//! Immutable request generations over one listener and shared machine state.
//! Only generation-owned monitoring tasks are retired; inference tasks and bodies
//! retain their original gateway, clients and settings until they complete.
use crate::{
    dashboard_settings::{self as settings, Specification},
    engine::Gateway,
};
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures::future::BoxFuture;
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex, RwLock},
};
use tokio::task::{AbortHandle, JoinHandle};
use tower::ServiceExt;

tokio::task_local! {
    static ENVIRONMENT: Arc<BTreeMap<String, String>>;
    static BACKGROUND: Arc<BackgroundTasks>;
}

/// Read an immutable generation environment, never mutate the process environment.
pub fn var(name: &str) -> Result<String, std::env::VarError> {
    ENVIRONMENT
        .try_with(|env| env.get(name).cloned().ok_or(std::env::VarError::NotPresent))
        .unwrap_or_else(|_| std::env::var(name))
}
pub(crate) fn background_is_staged() -> bool {
    BACKGROUND
        .try_with(|tasks| !*tasks.active.borrow())
        .unwrap_or(false)
}
pub fn current_environment() -> Arc<BTreeMap<String, String>> {
    ENVIRONMENT
        .try_with(Arc::clone)
        .unwrap_or_else(|_| Arc::new(std::env::vars().collect()))
}
pub fn with_environment<T>(spec: &Specification, run: impl FnOnce() -> T) -> T {
    ENVIRONMENT.sync_scope(environment(spec, 0), run)
}
pub fn environment(spec: &Specification, revision: u64) -> Arc<BTreeMap<String, String>> {
    let referenced = settings::referenced_environment(&spec.document);
    let mut values: BTreeMap<_, _> = std::env::vars()
        .filter(|(name, _)| !settings::managed_env(name) && !referenced.contains(name))
        .collect();
    values.extend(spec.environment.clone());
    values.insert("LLMCONDUIT_MANAGED_REVISION".into(), revision.to_string());
    Arc::new(values)
}
pub async fn scope<F: Future>(
    env: Arc<BTreeMap<String, String>>,
    tasks: Arc<BackgroundTasks>,
    future: F,
) -> F::Output {
    ENVIRONMENT
        .scope(env, BACKGROUND.scope(tasks, future))
        .await
}
/// Startup constructors open local policy/capture resources. Prepare them on
/// the blocking pool, preserving the generation scopes and runtime spawn context.
pub async fn prepare<T: Send + 'static>(
    run: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let env = current_environment();
    let background = BACKGROUND.try_with(Arc::clone).ok();
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _runtime = handle.enter();
        ENVIRONMENT.sync_scope(env, || match background {
            Some(tasks) => BACKGROUND.sync_scope(tasks, run),
            None => run(),
        })
    })
    .await
    .map_err(|_| "Runtime resource preparation failed".to_owned())?
}

pub struct BackgroundTasks {
    handles: Mutex<Vec<AbortHandle>>,
    active: tokio::sync::watch::Sender<bool>,
}
impl Default for BackgroundTasks {
    fn default() -> Self {
        Self {
            handles: Mutex::new(Vec::new()),
            active: tokio::sync::watch::channel(false).0,
        }
    }
}
impl BackgroundTasks {
    pub fn activate(&self) {
        self.active.send_replace(true);
    }
    pub fn stop(&self) {
        for task in self.handles.lock().expect("background tasks").drain(..) {
            task.abort();
        }
    }
}
impl Drop for BackgroundTasks {
    fn drop(&mut self) {
        self.stop();
    }
}
/// Use only for periodic generation-owned work, never inference or mesh transport.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let env = ENVIRONMENT.try_with(Arc::clone).ok();
    let mut active = BACKGROUND.try_with(|tasks| tasks.active.subscribe()).ok();
    let task = tokio::spawn(async move {
        if let Some(active) = &mut active {
            while !*active.borrow_and_update() {
                if active.changed().await.is_err() {
                    return std::future::pending().await;
                }
            }
        }
        match env {
            Some(env) => ENVIRONMENT.scope(env, future).await,
            None => future.await,
        }
    });
    let _ = BACKGROUND.try_with(|tasks| {
        tasks
            .handles
            .lock()
            .expect("background tasks")
            .push(task.abort_handle())
    });
    task
}

pub struct Generation {
    pub router: Router,
    pub gateway: Arc<Gateway>,
    pub environment: Arc<BTreeMap<String, String>>,
    pub tasks: Arc<BackgroundTasks>,
    pub retention_days: u64,
}
type LoggingReloader = dyn Fn(&str) + Send + Sync;
static LOGGING: std::sync::OnceLock<Box<LoggingReloader>> = std::sync::OnceLock::new();
pub fn set_logging_reloader(reload: Box<LoggingReloader>) {
    let _ = LOGGING.set(reload);
}
impl Generation {
    pub fn activate(&self) {
        self.gateway.apply_live_mesh_policy();
        if let Some(queue) = self.gateway.persistence_queue() {
            queue.set_retention_days(self.retention_days);
        }
        if let Some(reload) = LOGGING.get() {
            reload(
                self.environment
                    .get("RUST_LOG")
                    .map(String::as_str)
                    .unwrap_or("info"),
            );
        }
        self.tasks.activate();
    }
}
pub type Factory = dyn Fn(Arc<Gateway>, Specification, u64) -> BoxFuture<'static, Result<Generation, String>>
    + Send
    + Sync;
pub struct LiveRuntime {
    current: RwLock<Generation>,
    applying: tokio::sync::Mutex<()>,
    factory: Arc<Factory>,
}
impl LiveRuntime {
    pub fn new(initial: Generation, factory: Arc<Factory>) -> Arc<Self> {
        Arc::new(Self {
            current: RwLock::new(initial),
            applying: tokio::sync::Mutex::new(()),
            factory,
        })
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/internal/configuration-apply", axum::routing::post(apply))
            .fallback(dispatch)
            .with_state(self.clone())
    }
    async fn apply(&self, revision: u64) -> Result<(), String> {
        let _guard = self.applying.lock().await;
        let store = settings::owner().await?;
        let (raw, mut state) = settings::read(store.as_ref()).await?;
        if !state.pending && state.applied_revision == revision {
            return Ok(());
        }
        if !state.pending || state.revision != revision {
            return Err("Configuration changed during apply".into());
        }
        if !restart_reasons(&state.active, &state.draft).is_empty() {
            return Err("This change requires the supervised restart path".into());
        }
        let previous = self
            .current
            .read()
            .expect("runtime generation")
            .gateway
            .clone();
        let candidate = (self.factory)(previous, state.draft.clone(), revision).await;
        let failure = match candidate {
            Ok(candidate) => {
                state.active = state.draft.clone();
                state.applied_revision = revision;
                state.pending = false;
                state.status = "applied".into();
                state.error = None;
                if !settings::write(store.as_ref(), Some(&raw), &state).await? {
                    return Err("Configuration changed during apply".into());
                }
                let old = std::mem::replace(
                    &mut *self.current.write().expect("runtime generation"),
                    candidate,
                );
                old.tasks.stop();
                self.current.read().expect("runtime generation").activate();
                tracing::info!(
                    revision,
                    "Applied live configuration without draining requests"
                );
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(revision, %error, "Live configuration preparation failed");
                "Live configuration failed preparation; previous configuration remains active"
                    .to_owned()
            }
        };
        state.pending = false;
        state.status = "rolled_back".into();
        state.error = Some(failure.clone());
        if !settings::write(store.as_ref(), Some(&raw), &state).await? {
            return Err("Configuration changed during apply".into());
        }
        Err(failure)
    }
}
async fn dispatch(State(runtime): State<Arc<LiveRuntime>>, request: Request) -> Response {
    // Clone once, before any body/auth/routing work. Locks never span request work.
    let (router, env) = {
        let generation = runtime.current.read().expect("runtime generation");
        (generation.router.clone(), generation.environment.clone())
    };
    ENVIRONMENT
        .scope(env, async move {
            router
                .oneshot(request)
                .await
                .unwrap_or_else(|never| match never {})
        })
        .await
}
#[utoipa::path(post,path="/internal/configuration-apply",tag="internal",request_body=settings::Apply,responses((status=200,body=serde_json::Value),(status=404),(status=409),(status=503)))]
pub async fn unavailable(headers: HeaderMap) -> Response {
    crate::dashboard_api::json_no_store(
        if settings::internal_authorized(&headers) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::NOT_FOUND
        },
        &serde_json::json!({"error":"Managed runtime unavailable"}),
    )
}
async fn apply(
    State(runtime): State<Arc<LiveRuntime>>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<settings::Apply>,
) -> Response {
    if !settings::internal_authorized(&headers) {
        return crate::dashboard_api::json_no_store(
            StatusCode::NOT_FOUND,
            &serde_json::json!({"error":"Not found"}),
        );
    }
    match runtime.apply(body.revision).await {
        Ok(()) => crate::dashboard_api::json_no_store(
            StatusCode::OK,
            &serde_json::json!({"revision":body.revision}),
        ),
        Err(_) => crate::dashboard_api::json_no_store(
            StatusCode::CONFLICT,
            &serde_json::json!({"error":"Live apply failed; inspect settings status"}),
        ),
    }
}

/// Compare effective values: removing an override or spelling out an existing
/// default must not accidentally restart the service.
pub fn restart_reasons(old: &Specification, new: &Specification) -> Vec<&'static str> {
    fn resources(
        spec: &Specification,
    ) -> Result<BTreeMap<&'static str, serde_json::Value>, String> {
        with_environment(spec, || {
            let document = crate::control_plane::ControlPlaneConfig::from_yaml_str(
                &serde_yaml::to_string(&spec.document).map_err(|_| "Invalid configuration")?,
            )?;
            let materialized = document.materialize()?;
            let config =
                crate::config::Config::from_persisted_env_and_routes(materialized.gateway, &[])?;
            let storage = &document.control_plane().storage;
            let read = |key: &str| {
                var(key)
                    .ok()
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
            };
            let backend = read("LLMCONDUIT_STORAGE_BACKEND")
                .map(|value| {
                    let value = value.to_ascii_lowercase();
                    if value == "postgresql" {
                        "postgres".to_owned()
                    } else {
                        value
                    }
                })
                .unwrap_or(
                    serde_json::to_value(storage.backend)
                        .map_err(|_| "Invalid storage")?
                        .as_str()
                        .unwrap_or("none")
                        .to_owned(),
                );
            let controller = &config.mesh.controller;
            let worker = &config.mesh.worker;
            Ok(BTreeMap::from([
                ("listener", serde_json::json!(config.bind_addr.to_string())),
                (
                    "database connection",
                    serde_json::json!(read("LLMCONDUIT_DATABASE_URL").or(storage.url.clone())),
                ),
                (
                    "storage",
                    serde_json::json!([
                        backend,
                        read("LLMCONDUIT_STORAGE_JSONL_DIR").or(storage.jsonl_dir.clone()),
                        read("LLMCONDUIT_PERSISTENCE_QUEUE_CAPACITY")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(storage.queue_capacity),
                        config.auth.store_path,
                        controller.state_path
                    ]),
                ),
                (
                    "mesh transport",
                    serde_json::json!([
                        controller.enabled,
                        controller.bind_addr,
                        controller.identity_path,
                        worker.controller_addr,
                        worker.controller_endpoint_id,
                        worker.identity_path
                    ]),
                ),
            ]))
        })
    }
    match (resources(old), resources(new)) {
        (Ok(old), Ok(new)) => [
            "listener",
            "database connection",
            "storage",
            "mesh transport",
        ]
        .into_iter()
        .filter(|key| old.get(key) != new.get(key))
        .collect(),
        _ => vec!["invalid configuration"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn spec(document: serde_json::Value) -> Specification {
        Specification {
            document,
            environment: BTreeMap::new(),
        }
    }
    #[test]
    fn routine_changes_and_explicit_defaults_are_live() {
        let old = spec(json!({}));
        let new = spec(
            json!({"request_timeout_secs":120,"connect_timeout_secs":5,"max_request_body_bytes":12345,"system_prompt_prefix":"new", "auth":{"mode":"enforce"}, "mesh":{"controller":{"enabled":false,"heartbeat_timeout_secs":60,"model_allowlist":{}}}, "control_plane":{"storage":{"retention_days":7,"keep_media":false},"metrics":{"enabled":false},"vision_probe":{"enabled":false}}}),
        );
        assert!(restart_reasons(&old, &new).is_empty());
    }
    #[test]
    fn restart_categories_and_shadowed_environment_are_precise() {
        let mut old = spec(json!({}));
        for (document, reason) in [
            (json!({"bind_addr":"0.0.0.0:9999"}), "listener"),
            (
                json!({"control_plane":{"storage":{"url":"sqlite:/tmp/new.db"}}}),
                "database connection",
            ),
            (json!({"auth":{"store_path":"/tmp/new-auth.db"}}), "storage"),
            (
                json!({"mesh":{"controller":{"bind_addr":"0.0.0.0:9998"}}}),
                "mesh transport",
            ),
        ] {
            assert_eq!(restart_reasons(&old, &spec(document)), vec![reason]);
        }
        old.environment
            .insert("LLMCONDUIT_BIND_ADDR".into(), "0.0.0.0:4000".into());
        let mut new = old.clone();
        new.document["bind_addr"] = json!("0.0.0.0:5000");
        assert!(restart_reasons(&old, &new).is_empty());
        new.environment.clear();
        assert_eq!(restart_reasons(&old, &new), vec!["listener"]);
    }
    #[tokio::test]
    async fn environment_isolated_and_retired_background_does_not_cancel_inference() {
        let original = std::env::var("LLMCONDUIT_REQUEST_TIMEOUT_SECS").ok();
        let mut spec = spec(json!({}));
        spec.environment
            .insert("LLMCONDUIT_REQUEST_TIMEOUT_SECS".into(), "19".into());
        let tasks = Arc::new(BackgroundTasks::default());
        let (timer, inference) = scope(environment(&spec, 3), tasks.clone(), async {
            prepare(|| {
                assert_eq!(var("LLMCONDUIT_REQUEST_TIMEOUT_SECS").unwrap(), "19");
                Ok((
                    spawn(async { std::future::pending::<()>().await }),
                    tokio::spawn(async {
                        tokio::task::yield_now().await;
                        42
                    }),
                ))
            })
            .await
            .unwrap()
        })
        .await;
        tasks.activate();
        tasks.stop();
        assert!(timer.await.unwrap_err().is_cancelled());
        assert_eq!(inference.await.unwrap(), 42);
        assert_eq!(
            std::env::var("LLMCONDUIT_REQUEST_TIMEOUT_SECS").ok(),
            original
        );
    }
}

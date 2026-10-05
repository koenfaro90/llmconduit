//! Durable benchmark orchestration. Fleet owns execution; SQL stores run provenance
//! and bounded live/partial reports. Polling survives browser/controller reconnects.
use crate::{
    control_plane_store::PersistenceStore,
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_management::ManagedCommand,
    dashboard_mesh::{authorize_mesh_admin_read, authorize_mesh_mutation},
    engine::Gateway,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use utoipa::ToSchema;
const KEY: &str = "fleet_benchmarks_v1";
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BenchmarkCommand {
    Start { request: Value },
    Get { id: String },
    Cancel { id: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkOptions {
    pub concurrency: Vec<u32>,
    pub contexts: Vec<u32>,
    pub duration_seconds: u32,
    pub max_tokens: u32,
    pub prefill: bool,
}
impl BenchmarkOptions {
    fn valid(&self) -> bool {
        !self.concurrency.is_empty()
            && self.concurrency.len() <= 6
            && !self.contexts.is_empty()
            && self.contexts.len() <= 4
            && self.concurrency.len() * self.contexts.len() <= 24
            && (5..=60).contains(&self.duration_seconds)
            && (32..=2048).contains(&self.max_tokens)
            && self.concurrency.iter().all(|v| (1..=32).contains(v))
            && self.contexts.iter().all(|v| *v <= 131072)
            && self
                .concurrency
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.concurrency.len()
            && self
                .contexts
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.contexts.len()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateBenchmark {
    #[schema(value_type = String)]
    pub id: uuid::Uuid,
    pub deployment_id: String,
    pub model: String,
    pub options: BenchmarkOptions,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BenchmarkRun {
    pub id: String,
    pub deployment_id: String,
    pub endpoint_id: String,
    pub host: String,
    pub recipe_id: String,
    pub revision_digest: String,
    pub generation: u64,
    pub model: String,
    pub options: BenchmarkOptions,
    pub created_at_ms: i64,
    pub cancel_requested: bool,
    pub report: Option<Value>,
    pub error: Option<String>,
}
#[derive(Default, Clone, Serialize, Deserialize)]
struct Catalog {
    runs: BTreeMap<String, BenchmarkRun>,
}
fn error(status: StatusCode, message: &str) -> Response {
    json_no_store(status, &json!({"error":message}))
}
async fn read(store: &dyn PersistenceStore) -> Result<(Option<String>, Catalog), String> {
    let raw = store.get_setting(KEY).await?;
    let state = raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| "invalid benchmark state".to_string())?
        .unwrap_or_default();
    Ok((raw, state))
}
async fn commit(
    store: &dyn PersistenceStore,
    expected: Option<&str>,
    catalog: &Catalog,
) -> Result<bool, String> {
    let raw = serde_json::to_string(catalog).map_err(|_| "invalid benchmark state")?;
    if raw.len() > 4 * 1024 * 1024 {
        return Err("benchmark history exceeds bounds".into());
    };
    store.compare_exchange_setting(KEY, expected, &raw).await
}
fn terminal(run: &BenchmarkRun) -> bool {
    run.report
        .as_ref()
        .and_then(|r| r["status"].as_str())
        .is_some_and(|s| matches!(s, "complete" | "failed" | "cancelled" | "interrupted"))
}
#[utoipa::path(get,path="/dashboard/api/benchmarks",tag="dashboard",responses((status=200,body=Vec<BenchmarkRun>)))]
pub async fn list(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    };
    let Some(store) = gateway.persistence_store() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "SQL store required");
    };
    match read(store.as_ref()).await {
        Ok((_, catalog)) => {
            let mut runs: Vec<_> = catalog.runs.into_values().collect();
            runs.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
            for r in &mut runs {
                if let Some(report) = r.report.as_mut() {
                    report["cells"] = json!([]);
                    report["prefill"] = json!([]);
                    report["samples"] = json!([]);
                    report["live"] = Value::Null;
                    report["events"] = json!([])
                }
            }
            json_no_store(StatusCode::OK, &runs)
        }
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "benchmark state unavailable",
        ),
    }
}
#[utoipa::path(get,path="/dashboard/api/benchmarks/{id}",tag="dashboard",params(("id"=String,Path)),responses((status=200,body=BenchmarkRun)))]
pub async fn get(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    };
    let Some(store) = gateway.persistence_store() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "SQL store required");
    };
    match read(store.as_ref()).await {
        Ok((_, catalog)) => match catalog.runs.get(&id) {
            Some(run) => json_no_store(StatusCode::OK, run),
            None => error(StatusCode::NOT_FOUND, "benchmark not found"),
        },
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "benchmark state unavailable",
        ),
    }
}
#[utoipa::path(post,path="/dashboard/api/benchmarks",tag="dashboard",request_body=CreateBenchmark,responses((status=202,body=BenchmarkRun)))]
pub async fn create(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<CreateBenchmark>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    };
    if !body.options.valid()
        || body.deployment_id.len() > 40
        || body.model.is_empty()
        || body.model.len() > 512
        || body.model.chars().any(char::is_control)
    {
        return error(StatusCode::BAD_REQUEST, "invalid benchmark configuration");
    }
    let Some(store) = gateway.persistence_store() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "SQL store required");
    };
    let Some(admin) = gateway.mesh_admin() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "mesh controller required");
    };
    let Ok((_, state)) = crate::dashboard_management::read(store.as_ref()).await else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "deployment catalog unavailable",
        );
    };
    let Some(d) = state.deployments.get(&body.deployment_id) else {
        return error(StatusCode::NOT_FOUND, "deployment not found");
    };
    if d.desired_state != "ready"
        || d.observed
            .as_ref()
            .and_then(|o| o["status"]["phase"].as_str())
            != Some("ready")
    {
        return error(StatusCode::CONFLICT, "deployment must be ready");
    };
    if !admin
        .store()
        .is_node_authorized(&d.endpoint_id)
        .await
        .unwrap_or(false)
    {
        return error(StatusCode::FORBIDDEN, "host is not authorized");
    };
    let Ok(endpoint) = d.endpoint_id.parse() else {
        return error(StatusCode::BAD_REQUEST, "invalid host");
    };
    if !admin.registry().managed_deployments_available(endpoint) {
        return error(StatusCode::CONFLICT, "host is unavailable");
    };
    let Some(revision) = state
        .recipes
        .get(&d.recipe_id)
        .and_then(|rs| rs.iter().find(|r| r.digest == d.revision_digest))
    else {
        return error(StatusCode::CONFLICT, "pinned recipe unavailable");
    };
    let Ok(runtime) =
        crate::dashboard_deployment_groups::render(&revision.model_spec, d.placement.as_ref())
    else {
        return error(StatusCode::CONFLICT, "invalid distributed placement");
    };
    if runtime["readiness"]["url"].as_str().is_none() {
        return error(
            StatusCode::CONFLICT,
            "headless workers cannot run an HTTP benchmark",
        );
    };
    let host = admin
        .registry()
        .host_snapshot(endpoint)
        .map(|s| s.hostname)
        .unwrap_or_else(|| d.endpoint_id[..12.min(d.endpoint_id.len())].to_string());
    let run = BenchmarkRun {
        id: body.id.to_string(),
        deployment_id: body.deployment_id,
        endpoint_id: d.endpoint_id.clone(),
        host,
        recipe_id: d.recipe_id.clone(),
        revision_digest: d.revision_digest.clone(),
        generation: d.generation,
        model: body.model,
        options: body.options,
        created_at_ms: crate::mesh::store::now_ms(),
        cancel_requested: false,
        report: None,
        error: None,
    };
    for _ in 0..4 {
        let Ok((raw, mut catalog)) = read(store.as_ref()).await else {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "benchmark state unavailable",
            );
        };
        if let Some(old) = catalog.runs.get(&run.id) {
            if old.deployment_id != run.deployment_id
                || old.options != run.options
                || old.model != run.model
            {
                return error(StatusCode::CONFLICT, "run ID belongs to another request");
            };
            return json_no_store(StatusCode::ACCEPTED, old);
        };
        if catalog
            .runs
            .values()
            .any(|r| r.endpoint_id == run.endpoint_id && !terminal(r))
        {
            return error(
                StatusCode::CONFLICT,
                "this host already has an active benchmark",
            );
        };
        if catalog.runs.len() >= 64 {
            let Some(old) = catalog
                .runs
                .values()
                .filter(|r| terminal(r))
                .min_by_key(|r| r.created_at_ms)
                .map(|r| r.id.clone())
            else {
                return error(StatusCode::CONFLICT, "benchmark history full");
            };
            catalog.runs.remove(&old);
        };
        if let Err(e) = gateway
            .traffic()
            .reserve_deployment(&run.id, &run.deployment_id, false)
        {
            return error(StatusCode::CONFLICT, &e.client_message);
        }
        struct ReservationGuard {
            traffic: crate::traffic::TrafficController,
            id: String,
            committed: bool,
        }
        impl Drop for ReservationGuard {
            fn drop(&mut self) {
                if !self.committed {
                    self.traffic.unreserve(&self.id);
                }
            }
        }
        let mut guard = ReservationGuard {
            traffic: gateway.traffic(),
            id: run.id.clone(),
            committed: false,
        };
        catalog.runs.insert(run.id.clone(), run.clone());
        match commit(store.as_ref(), raw.as_deref(), &catalog).await {
            Ok(true) => {
                guard.committed = true;
                return json_no_store(StatusCode::ACCEPTED, &run);
            }
            Ok(false) => {
                gateway.traffic().unreserve(&run.id);
                continue;
            }
            Err(_) => {
                gateway.traffic().unreserve(&run.id);
                return error(StatusCode::SERVICE_UNAVAILABLE, "cannot persist benchmark");
            }
        }
    }
    error(StatusCode::CONFLICT, "benchmark catalog changed; retry")
}
#[utoipa::path(post,path="/dashboard/api/benchmarks/{id}/cancel",tag="dashboard",params(("id"=String,Path)),responses((status=202,body=BenchmarkRun)))]
pub async fn cancel(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    };
    let Some(store) = gateway.persistence_store() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "SQL store required");
    };
    for _ in 0..4 {
        let Ok((raw, mut catalog)) = read(store.as_ref()).await else {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "benchmark state unavailable",
            );
        };
        let Some(run) = catalog.runs.get_mut(&id) else {
            return error(StatusCode::NOT_FOUND, "benchmark not found");
        };
        run.cancel_requested = true;
        let response = run.clone();
        match commit(store.as_ref(), raw.as_deref(), &catalog).await {
            Ok(true) => return json_no_store(StatusCode::ACCEPTED, &response),
            Ok(false) => continue,
            Err(_) => break,
        }
    }
    error(
        StatusCode::CONFLICT,
        "could not record benchmark cancellation",
    )
}

pub(crate) fn start_reconciler(gateway: Arc<Gateway>) {
    if gateway.mesh_admin().is_none() || gateway.persistence_store().is_none() {
        return;
    };
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(2));
        loop {
            timer.tick().await;
            reconcile(&gateway).await
        }
    });
}
async fn reconcile(gateway: &Gateway) {
    use futures::StreamExt;
    let Some(store) = gateway.persistence_store() else {
        return;
    };
    let Ok((_, catalog)) = read(store.as_ref()).await else {
        return;
    };
    for run in catalog.runs.values() {
        if terminal(run) {
            crate::dashboard_recipe_evidence::record_benchmark(gateway, run).await;
            gateway.traffic().unreserve(&run.id);
        }
    }
    futures::stream::iter(catalog.runs.values().filter(|r| !terminal(r)))
        .for_each_concurrent(Some(4), |run| reconcile_run(gateway, run))
        .await;
}
async fn reconcile_run(gateway: &Gateway, run: &BenchmarkRun) {
    if gateway
        .traffic()
        .reserve_deployment(&run.id, &run.deployment_id, run.report.is_some())
        .is_err()
    {
        return;
    }
    let Some(admin) = gateway.mesh_admin() else {
        return;
    };
    let Some(store) = gateway.persistence_store() else {
        return;
    };
    if !admin
        .store()
        .is_node_authorized(&run.endpoint_id)
        .await
        .unwrap_or(false)
    {
        return;
    };
    let Ok(endpoint) = run.endpoint_id.parse() else {
        return;
    };
    let command = if run.cancel_requested {
        BenchmarkCommand::Cancel { id: run.id.clone() }
    } else if run.report.is_none() {
        BenchmarkCommand::Start {
            request: json!({"id":run.id,"deployment_id":run.deployment_id,"generation":run.generation,"recipe_digest":run.revision_digest,"model":run.model,"options":run.options}),
        }
    } else {
        BenchmarkCommand::Get { id: run.id.clone() }
    };
    let result = admin
        .registry()
        .manage_fleet(
            endpoint,
            ManagedCommand {
                settings: None,
                benchmark: Some(command),
                deployment_id: None,
                desired: None,
            },
        )
        .await;
    let (report, error) = match result {
        Ok(reply) => {
            if let Some(report) = reply.result {
                if valid_report(&report, &run.id) {
                    (Some(report), None)
                } else {
                    (None, Some("invalid bounded benchmark report".to_string()))
                }
            } else if reply
                .error_status
                .is_some_and(|s| matches!(s, 404 | 409 | 400))
            {
                let cancelled = run.cancel_requested && reply.error_status == Some(404);
                let now = crate::mesh::store::now_ms();
                (
                    Some(
                        json!({"id":run.id,"status":if cancelled{"cancelled"}else{"failed"},"started_at_ms":run.created_at_ms,"updated_at_ms":now,"finished_at_ms":now,"image":"","tool":{"version":"unavailable","revision":"","repository":""},"message":if cancelled{"Cancelled before execution"}else{"Fleet rejected admission; deployment changed, is busy, or needs benchmark support"},"cells":[],"prefill":[],"events":[],"sequence":1}),
                    ),
                    None,
                )
            } else {
                (None, reply.error)
            }
        }
        Err(_) => (
            None,
            Some("host unavailable; waiting to reconnect".to_string()),
        ),
    };
    for _ in 0..4 {
        let Ok((raw, mut latest)) = read(store.as_ref()).await else {
            return;
        };
        let Some(current) = latest.runs.get_mut(&run.id) else {
            return;
        };
        if let Some(report) = report.as_ref() {
            current.report = Some(report.clone())
        };
        current.error = error.clone();
        match commit(store.as_ref(), raw.as_deref(), &latest).await {
            Ok(false) => continue,
            _ => return,
        }
    }
}
fn valid_report(report: &Value, id: &str) -> bool {
    report["id"].as_str() == Some(id)
        && report["status"].as_str().is_some_and(|s| {
            matches!(
                s,
                "running" | "cancelling" | "complete" | "failed" | "cancelled" | "interrupted"
            )
        })
        && report["cells"].as_array().is_some_and(|c| c.len() <= 24)
        && report["prefill"].as_array().is_some_and(|c| c.len() <= 8)
        && report["events"].as_array().is_some_and(|c| c.len() <= 40)
        && serde_json::to_vec(report).is_ok_and(|r| r.len() <= 48 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_and_report_contract() {
        let mut o = BenchmarkOptions {
            concurrency: vec![1, 2],
            contexts: vec![0, 8192],
            duration_seconds: 5,
            max_tokens: 256,
            prefill: true,
        };
        assert!(o.valid());
        o.concurrency = vec![1, 1];
        assert!(!o.valid());
        o.concurrency = vec![33];
        assert!(!o.valid());
        assert!(!valid_report(
            &json!({"id":"x","status":"complete","cells":[],"prefill":[],"events":[]}),
            "other"
        ));
        assert!(valid_report(
            &json!({"id":"x","status":"complete","cells":[],"prefill":[],"events":[]}),
            "x"
        ));
    }
    #[tokio::test]
    async fn durable_catalog_cas() {
        let store = crate::control_plane_store::SqlStore::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        let (raw, state) = read(&store).await.unwrap();
        assert!(commit(&store, raw.as_deref(), &state).await.unwrap());
        assert!(!commit(&store, None, &state).await.unwrap());
    }
    #[tokio::test]
    async fn benchmark_handlers_require_admin_and_csrf() {
        use crate::dashboard_auth::{AuthSessionKind, CSRF_COOKIE, CSRF_HEADER, DashboardEnv};
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let auth = DashboardAuth::from_env(
            "127.0.0.1:0".parse().unwrap(),
            &DashboardEnv {
                token: Some("benchmark-test".into()),
                session_key_b64: Some(STANDARD.encode([8u8; 32])),
                allow_mutations: true,
                ..Default::default()
            },
        )
        .unwrap()
        .auth;
        let session = |is_admin| AuthSession {
            exp: u64::MAX,
            user: Some(crate::accounts::SessionUser {
                id: "operator".into(),
                username: "operator".into(),
                is_admin,
            }),
            kind: AuthSessionKind::User,
        };
        let (_, gateway) = crate::build_app_with_gateway(
            crate::config::Config::from_persisted(&serde_json::from_value(json!({})).unwrap())
                .unwrap(),
        );
        let request = CreateBenchmark {
            id: uuid::Uuid::new_v4(),
            deployment_id: "dep".into(),
            model: "model".into(),
            options: BenchmarkOptions {
                concurrency: vec![1, 2],
                contexts: vec![0],
                duration_seconds: 5,
                max_tokens: 64,
                prefill: false,
            },
        };
        for admin in [false, true] {
            assert_eq!(
                create(
                    State(gateway.clone()),
                    Extension(auth.clone()),
                    Extension(session(admin)),
                    HeaderMap::new(),
                    Json(request.clone())
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            list(
                State(gateway.clone()),
                Extension(auth.clone()),
                Extension(session(false)),
                HeaderMap::new()
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            cancel(
                State(gateway.clone()),
                Extension(auth.clone()),
                Extension(session(true)),
                HeaderMap::new(),
                Path("run".into())
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let csrf = auth.issue_csrf_token();
        let mut headers = HeaderMap::new();
        headers.insert(CSRF_HEADER, csrf.parse().unwrap());
        headers.insert(
            axum::http::header::COOKIE,
            format!("{CSRF_COOKIE}={csrf}").parse().unwrap(),
        );
        let mut invalid = request;
        invalid.options.concurrency = vec![1, 1];
        assert_eq!(
            create(
                State(gateway),
                Extension(auth),
                Extension(session(true)),
                headers,
                Json(invalid)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
}

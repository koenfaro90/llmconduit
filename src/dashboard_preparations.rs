//! Durable recipe preparation intent. Fleet owns image pulls, cache writes and verification.
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
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use utoipa::ToSchema;
const KEY: &str = "fleet_preparations_v1";
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub repository: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subfolder: String,
}
impl Checkpoint {
    pub(crate) fn valid(&self) -> bool {
        regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}/[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$")
            .unwrap()
            .is_match(&self.repository)
            && !self.repository.contains("..")
            && self.revision.len() == 40
            && self
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && (self.subfolder.is_empty()
                || (self.subfolder.len() <= 256
                    && !self.subfolder.contains("..")
                    && regex::Regex::new(
                        r"^[A-Za-z0-9][A-Za-z0-9_.-]*(/[A-Za-z0-9][A-Za-z0-9_.-]*)*$",
                    )
                    .unwrap()
                    .is_match(&self.subfolder)))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreparationCommand {
    Start { request: Value },
    Get { id: String },
    Cancel { id: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePreparation {
    #[schema(value_type=String)]
    pub id: uuid::Uuid,
    pub endpoint_id: String,
    pub recipe_id: String,
    pub revision_digest: String,
    #[serde(default)]
    pub rank: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PreparationRun {
    pub cache_directory: String,
    pub id: String,
    pub endpoint_id: String,
    pub recipe_id: String,
    pub revision_digest: String,
    pub rank: usize,
    pub image: String,
    pub checkpoints: Vec<Checkpoint>,
    pub created_at_ms: i64,
    pub cancel_requested: bool,
    pub report: Option<Value>,
    pub error: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Catalog {
    runs: BTreeMap<String, PreparationRun>,
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
        .map_err(|_| "invalid preparation catalog".to_string())?
        .unwrap_or_default();
    Ok((raw, state))
}
async fn commit(
    store: &dyn PersistenceStore,
    raw: Option<&str>,
    state: &Catalog,
) -> Result<bool, String> {
    let next =
        serde_json::to_string(state).map_err(|_| "invalid preparation catalog".to_string())?;
    if next.len() > 2 * 1024 * 1024 {
        return Err("preparation history exceeds bounds".into());
    };
    store.compare_exchange_setting(KEY, raw, &next).await
}
fn terminal(run: &PreparationRun) -> bool {
    run.report
        .as_ref()
        .and_then(|r| r["status"].as_str())
        .is_some_and(|s| matches!(s, "complete" | "failed" | "cancelled" | "interrupted"))
}
#[utoipa::path(get,path="/dashboard/api/preparations",tag="dashboard",responses((status=200,body=Vec<PreparationRun>)))]
pub async fn list(
    State(g): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    };
    let Some(store) = g.persistence_store() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "SQL store required");
    };
    match read(store.as_ref()).await {
        Ok((_, state)) => {
            let mut rows: Vec<_> = state.runs.into_values().collect();
            rows.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
            json_no_store(StatusCode::OK, &rows)
        }
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "preparation history unavailable",
        ),
    }
}
#[utoipa::path(post,path="/dashboard/api/preparations",tag="dashboard",request_body=CreatePreparation,responses((status=202,body=PreparationRun),(status=400),(status=403),(status=409)))]
pub async fn create(
    State(g): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<CreatePreparation>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    };
    let Some(store) = g.persistence_store() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "SQL store required");
    };
    let Some(admin) = g.mesh_admin() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "mesh controller required");
    };
    if !admin
        .store()
        .is_node_authorized(&body.endpoint_id)
        .await
        .unwrap_or(false)
    {
        return error(StatusCode::BAD_REQUEST, "select an enrolled enabled host");
    }
    let Ok((_, management)) = crate::dashboard_management::read(store.as_ref()).await else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "recipe catalog unavailable",
        );
    };
    let Some(revision) = management
        .recipes
        .get(&body.recipe_id)
        .and_then(|rows| rows.iter().find(|r| r.digest == body.revision_digest))
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "select an immutable recipe revision",
        );
    };
    let spec = revision
        .model_spec
        .get("distributed")
        .and_then(Value::as_array)
        .and_then(|r| r.get(body.rank))
        .unwrap_or(&revision.model_spec);
    if revision.model_spec.get("distributed").is_none() && body.rank != 0 {
        return error(StatusCode::BAD_REQUEST, "invalid recipe rank");
    };
    let Some(image) = spec
        .get("image")
        .and_then(Value::as_str)
        .filter(|s| s.starts_with("sha256:") || s.contains("@sha256:"))
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "preparation requires a digest-pinned executable image",
        );
    };
    let checkpoints: Vec<Checkpoint> = match serde_json::from_value(
        revision
            .model_spec
            .pointer("/metadata/checkpoints")
            .cloned()
            .unwrap_or(json!([])),
    ) {
        Ok(c) => c,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid checkpoint requirements"),
    };
    if checkpoints.len() > 8 || checkpoints.iter().any(|c| !c.valid()) {
        return error(
            StatusCode::BAD_REQUEST,
            "checkpoint requirements must pin immutable revisions",
        );
    }
    let cache_directory = if checkpoints.is_empty() {
        String::new()
    } else {
        match recipe_cache_directory(spec) {
            Ok(path) => path,
            Err(message) => return error(StatusCode::BAD_REQUEST, &message),
        }
    };
    let run = PreparationRun {
        cache_directory,
        id: body.id.to_string(),
        endpoint_id: body.endpoint_id,
        recipe_id: body.recipe_id,
        revision_digest: body.revision_digest,
        rank: body.rank,
        image: image.into(),
        checkpoints,
        created_at_ms: crate::mesh::store::now_ms(),
        cancel_requested: false,
        report: None,
        error: None,
    };
    for _ in 0..4 {
        let Ok((raw, mut state)) = read(store.as_ref()).await else {
            break;
        };
        if let Some(old) = state.runs.get(&run.id) {
            if old.endpoint_id == run.endpoint_id
                && old.recipe_id == run.recipe_id
                && old.revision_digest == run.revision_digest
                && old.rank == run.rank
            {
                return json_no_store(StatusCode::ACCEPTED, old);
            };
            return error(
                StatusCode::CONFLICT,
                "preparation ID belongs to another request",
            );
        };
        if state.runs.len() >= 256 {
            let oldest = state
                .runs
                .values()
                .filter(|r| terminal(r))
                .min_by_key(|r| r.created_at_ms)
                .map(|r| r.id.clone());
            if let Some(id) = oldest {
                state.runs.remove(&id);
            } else {
                return error(
                    StatusCode::CONFLICT,
                    "preparation history is full of active jobs",
                );
            }
        };
        state.runs.insert(run.id.clone(), run.clone());
        match commit(store.as_ref(), raw.as_deref(), &state).await {
            Ok(true) => return json_no_store(StatusCode::ACCEPTED, &run),
            Ok(false) => continue,
            Err(_) => break,
        }
    }
    error(
        StatusCode::CONFLICT,
        "could not persist preparation; reload",
    )
}
#[utoipa::path(post,path="/dashboard/api/preparations/{id}/cancel",tag="dashboard",params(("id"=String,Path)),responses((status=202,body=PreparationRun),(status=404)))]
pub async fn cancel(
    State(g): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    };
    let Some(store) = g.persistence_store() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "SQL store required");
    };
    for _ in 0..4 {
        let Ok((raw, mut state)) = read(store.as_ref()).await else {
            break;
        };
        let Some(run) = state.runs.get_mut(&id) else {
            return error(StatusCode::NOT_FOUND, "preparation not found");
        };
        run.cancel_requested = true;
        let result = run.clone();
        match commit(store.as_ref(), raw.as_deref(), &state).await {
            Ok(true) => return json_no_store(StatusCode::ACCEPTED, &result),
            Ok(false) => continue,
            Err(_) => break,
        }
    }
    error(StatusCode::CONFLICT, "could not persist cancellation")
}
pub(crate) fn start_reconciler(g: Arc<Gateway>) {
    if g.mesh_admin().is_none() || g.persistence_store().is_none() {
        return;
    };
    crate::runtime_configuration::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            reconcile(&g).await
        }
    });
}
async fn reconcile(g: &Gateway) {
    use futures::StreamExt;
    let Some(store) = g.persistence_store() else {
        return;
    };
    let Ok((_, state)) = read(store.as_ref()).await else {
        return;
    };
    let mut pending: Vec<_> = state.runs.values().filter(|r| !terminal(r)).collect();
    pending.sort_by_key(|r| (r.created_at_ms, r.id.clone()));
    let mut hosts = BTreeSet::new();
    let work: Vec<_> = pending
        .into_iter()
        .filter(|r| r.cancel_requested || hosts.insert(r.endpoint_id.clone()))
        .collect();
    futures::stream::iter(work)
        .for_each_concurrent(Some(4), |r| reconcile_run(g, r))
        .await;
}
fn failed(run: &PreparationRun, status: &str, message: &str) -> Value {
    json!({"id":run.id,"recipe_digest":run.revision_digest,"image":run.image,"cache_directory":run.cache_directory,"checkpoints":run.checkpoints,"status":status,"phase":status,"message":message,"total_bytes":0,"completed_bytes":0,"downloaded_bytes":0,"verified_files":0,"total_files":0,"sequence":0})
}
fn valid_report(report: &Value, run: &PreparationRun) -> bool {
    report["id"] == run.id
        && report["recipe_digest"] == run.revision_digest
        && report["image"] == run.image
        && report["cache_directory"].as_str().unwrap_or_default() == run.cache_directory
        && report["checkpoints"] == json!(run.checkpoints)
        && report["status"].as_str().is_some_and(|s| {
            matches!(
                s,
                "running" | "cancelling" | "complete" | "failed" | "cancelled" | "interrupted"
            )
        })
        && [
            "total_bytes",
            "completed_bytes",
            "downloaded_bytes",
            "verified_files",
            "total_files",
            "sequence",
        ]
        .iter()
        .all(|key| report[*key].as_u64().is_some())
        && report["completed_bytes"].as_u64() <= report["total_bytes"].as_u64()
        && report["verified_files"].as_u64() <= report["total_files"].as_u64()
        && (report["status"] != "complete"
            || (report["completed_bytes"] == report["total_bytes"]
                && report["verified_files"] == report["total_files"]
                && (run.checkpoints.is_empty()
                    || report["total_files"].as_u64().unwrap_or_default() > 0)))
        && serde_json::to_vec(report).is_ok_and(|raw| raw.len() <= 32768)
}
async fn reconcile_run(g: &Gateway, run: &PreparationRun) {
    let Some(admin) = g.mesh_admin() else { return };
    let Some(store) = g.persistence_store() else {
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
        PreparationCommand::Cancel { id: run.id.clone() }
    } else if run.report.is_none() {
        PreparationCommand::Start {
            request: json!({"id":run.id,"recipe_digest":run.revision_digest,"image":run.image,"cache_directory":run.cache_directory,"checkpoints":run.checkpoints}),
        }
    } else {
        PreparationCommand::Get { id: run.id.clone() }
    };
    let result = admin
        .registry()
        .manage_fleet(
            endpoint,
            ManagedCommand {
                preparation: Some(command),
                settings: None,
                engine_metrics: None,
                benchmark: None,
                deployment_id: None,
                desired: None,
            },
        )
        .await;
    let (report, error) = match result {
        Ok(reply) => {
            if let Some(report) = reply.result {
                if valid_report(&report, run) {
                    (Some(report), None)
                } else {
                    (
                        None,
                        Some("Fleet returned an invalid preparation receipt".into()),
                    )
                }
            } else if reply.error_status == Some(423) {
                (
                    None,
                    Some("Host preparation is busy; queued for retry".into()),
                )
            } else if reply
                .error_status
                .is_some_and(|s| matches!(s, 400 | 403 | 404 | 409))
            {
                let report = if run.cancel_requested && reply.error_status == Some(404) {
                    failed(run, "cancelled", "Cancelled before Fleet accepted the job")
                } else {
                    failed(
                        run,
                        "failed",
                        "Fleet rejected preparation; check its version, checkpoint-cache and image policy",
                    )
                };
                (Some(report), None)
            } else {
                (None, Some("Fleet is unavailable; reconnecting".into()))
            }
        }
        Err(_) => (
            None,
            Some("Host disconnected; durable preparation intent retained".into()),
        ),
    };
    for _ in 0..4 {
        let Ok((raw, mut state)) = read(store.as_ref()).await else {
            return;
        };
        let Some(current) = state.runs.get_mut(&run.id) else {
            return;
        };
        if terminal(current) {
            return;
        };
        if let Some(report) = &report {
            let sequence = report["sequence"].as_u64().unwrap_or_default();
            if current
                .report
                .as_ref()
                .and_then(|v| v["sequence"].as_u64())
                .is_none_or(|old| sequence >= old)
            {
                current.report = Some(report.clone())
            }
        };
        current.error = error.clone();
        match commit(store.as_ref(), raw.as_deref(), &state).await {
            Ok(true) => return,
            Ok(false) => continue,
            Err(_) => return,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_subfolder_round_trips_without_changing_legacy_wire() {
        let old = json!({"repository":"owner/model","revision":"a".repeat(40)});
        let legacy: Checkpoint = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(legacy).unwrap(), old);
        let selected =
            json!({"repository":"owner/model","revision":"a".repeat(40),"subfolder":"FL2VA"});
        let checkpoint: Checkpoint = serde_json::from_value(selected.clone()).unwrap();
        assert!(checkpoint.valid());
        assert_eq!(serde_json::to_value(&checkpoint).unwrap(), selected);
        for subfolder in [
            "/FL2VA",
            "../FL2VA",
            "FL2VA/../Ref2VA",
            "FL2VA/",
            "FL2VA//weights",
        ] {
            assert!(
                !Checkpoint {
                    subfolder: subfolder.into(),
                    ..checkpoint.clone()
                }
                .valid()
            );
        }
    }
    #[test]
    fn checkpoint_refs_require_immutable_commits() {
        assert!(
            Checkpoint {
                repository: "deepseek-ai/DeepSeek-V4.1-Flash-1017".into(),
                revision: "a".repeat(40),
                subfolder: String::new(),
            }
            .valid()
        );
        for repository in [
            "../private",
            "owner/../../file",
            "https://host/repo",
            "owner/a..b",
        ] {
            assert!(
                !Checkpoint {
                    repository: repository.into(),
                    revision: "a".repeat(40),
                    subfolder: String::new(),
                }
                .valid()
            )
        }
        assert!(
            !Checkpoint {
                repository: "owner/model".into(),
                revision: "main".into(),
                subfolder: String::new(),
            }
            .valid()
        )
    }
}

/// New starts with declared artifacts wait for the same preparation jobs exposed in
/// Deployment. Preparing is independent of placement admission and existing models.
pub(crate) async fn ensure_prepared(
    g: &Gateway,
    deployment: &crate::dashboard_management::DesiredDeployment,
    spec: &Value,
) -> Result<bool, String> {
    let checkpoints: Vec<Checkpoint> = serde_json::from_value(
        spec.pointer("/metadata/checkpoints")
            .cloned()
            .unwrap_or(json!([])),
    )
    .map_err(|_| "invalid checkpoint requirements")?;
    if checkpoints.is_empty() {
        return Ok(true);
    };
    let store = g
        .persistence_store()
        .ok_or("SQL store required for checkpoint preparation")?;
    let rank = deployment.placement.as_ref().map_or(0, |p| p.rank as usize);
    let model = spec
        .get("distributed")
        .and_then(Value::as_array)
        .and_then(|r| r.get(rank))
        .unwrap_or(spec);
    let image = model
        .get("image")
        .and_then(Value::as_str)
        .ok_or("recipe image missing")?;
    let cache_directory = recipe_cache_directory(model)?;
    for _ in 0..4 {
        let (raw, mut state) = read(store.as_ref()).await?;
        // Execution-only edits (for example GPU utilization) do not change
        // downloaded artifacts. Keep the original receipt and its recipe pin;
        // never fabricate a new Fleet verification for the edited revision.
        if state.runs.values().any(|run| {
            completed_artifacts_match(run, deployment, image, &checkpoints, &cache_directory)
        }) {
            return Ok(true);
        }
        let matching: Vec<_> = state
            .runs
            .values()
            .filter(|r| {
                r.endpoint_id == deployment.endpoint_id
                    && r.revision_digest == deployment.revision_digest
                    && r.rank == rank
            })
            .collect();
        if let Some(latest) = matching.into_iter().max_by_key(|r| r.created_at_ms) {
            if terminal(latest) {
                return Err(
                    "Checkpoint preparation failed or was cancelled; prepare again in Deployment"
                        .into(),
                );
            };
            return Ok(false);
        }
        if state.runs.len() >= 256 {
            if let Some(id) = state
                .runs
                .values()
                .filter(|r| terminal(r))
                .min_by_key(|r| r.created_at_ms)
                .map(|r| r.id.clone())
            {
                state.runs.remove(&id);
            } else {
                return Err("Preparation job history is full".into());
            }
        }
        let run = PreparationRun {
            cache_directory: cache_directory.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            endpoint_id: deployment.endpoint_id.clone(),
            recipe_id: deployment.recipe_id.clone(),
            revision_digest: deployment.revision_digest.clone(),
            rank,
            image: image.into(),
            checkpoints: checkpoints.clone(),
            created_at_ms: crate::mesh::store::now_ms(),
            cancel_requested: false,
            report: None,
            error: None,
        };
        state.runs.insert(run.id.clone(), run);
        if commit(store.as_ref(), raw.as_deref(), &state).await? {
            return Ok(false);
        }
    }
    Err("Preparation catalog changed; retrying".into())
}

fn completed_artifacts_match(
    run: &PreparationRun,
    deployment: &crate::dashboard_management::DesiredDeployment,
    image: &str,
    checkpoints: &[Checkpoint],
    cache_directory: &str,
) -> bool {
    run.endpoint_id == deployment.endpoint_id
        && run.rank == deployment.placement.as_ref().map_or(0, |p| p.rank as usize)
        && run.image == image
        && run.checkpoints == checkpoints
        && run.cache_directory == cache_directory
        && run
            .report
            .as_ref()
            .is_some_and(|report| report["status"] == "complete" && valid_report(report, run))
}

#[cfg(test)]
mod artifact_reuse_tests {
    use super::*;

    #[test]
    fn reuse_keeps_original_provenance_and_checks_every_artifact_boundary() {
        let run = PreparationRun {
            cache_directory: "/hf".into(),
            id: uuid::Uuid::new_v4().to_string(),
            endpoint_id: "host-a".into(),
            recipe_id: "original".into(),
            revision_digest: "a".repeat(64),
            rank: 0,
            image: format!("image@sha256:{}", "b".repeat(64)),
            checkpoints: vec![Checkpoint {
                repository: "owner/model".into(),
                revision: "c".repeat(40),
                subfolder: String::new(),
            }],
            created_at_ms: 1,
            cancel_requested: false,
            report: None,
            error: None,
        };
        let mut run = PreparationRun {
            report: Some(
                json!({"id":run.id,"recipe_digest":run.revision_digest,"image":run.image,"cache_directory":run.cache_directory,"checkpoints":run.checkpoints,"status":"complete","total_bytes":42,"completed_bytes":42,"total_files":1,"verified_files":1,"downloaded_bytes":42,"sequence":3}),
            ),
            ..run
        };
        let mut deployment: crate::dashboard_management::DesiredDeployment = serde_json::from_value(json!({"generation":2,"endpoint_id":"host-a","recipe_id":"edited","revision_digest":"d".repeat(64),"desired_state":"ready","updated_at_ms":2})).unwrap();
        assert!(completed_artifacts_match(
            &run,
            &deployment,
            &run.image,
            &run.checkpoints,
            "/hf"
        ));
        assert_eq!(run.recipe_id, "original");
        assert_eq!(
            run.report.as_ref().unwrap()["recipe_digest"],
            "a".repeat(64)
        );
        deployment.endpoint_id = "host-b".into();
        assert!(!completed_artifacts_match(
            &run,
            &deployment,
            &run.image,
            &run.checkpoints,
            "/hf"
        ));
        deployment.endpoint_id = "host-a".into();
        assert!(!completed_artifacts_match(
            &run,
            &deployment,
            "other-image",
            &run.checkpoints,
            "/hf"
        ));
        assert!(!completed_artifacts_match(
            &run,
            &deployment,
            &run.image,
            &run.checkpoints,
            "/other-cache"
        ));
        let mut changed = run.checkpoints.clone();
        changed[0].revision = "e".repeat(40);
        assert!(!completed_artifacts_match(
            &run,
            &deployment,
            &run.image,
            &changed,
            "/hf"
        ));
        run.rank = 1;
        assert!(!completed_artifacts_match(
            &run,
            &deployment,
            &run.image,
            &run.checkpoints,
            "/hf"
        ));
        run.rank = 0;
        run.report.as_mut().unwrap()["recipe_digest"] = json!("d".repeat(64));
        assert!(!completed_artifacts_match(
            &run,
            &deployment,
            &run.image,
            &run.checkpoints,
            "/hf"
        ));
        run.report.as_mut().unwrap()["recipe_digest"] = json!("a".repeat(64));
        run.report.as_mut().unwrap()["completed_bytes"] = json!(41);
        assert!(!completed_artifacts_match(
            &run,
            &deployment,
            &run.image,
            &run.checkpoints,
            "/hf"
        ));
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    #[test]
    fn preparation_command_round_trips_and_rejects_extra_execution_fields() {
        let command = ManagedCommand {
            settings: None,
            engine_metrics: None,
            benchmark: None,
            preparation: Some(PreparationCommand::Start {
                request: json!({"id":uuid::Uuid::new_v4(),"recipe_digest":"a".repeat(64),"image":format!("image@sha256:{}","b".repeat(64)),"checkpoints":[{"repository":"owner/model","revision":"c".repeat(40)}]}),
            }),
            deployment_id: None,
            desired: None,
        };
        let raw = serde_json::to_value(&command).unwrap();
        assert_eq!(
            serde_json::from_value::<ManagedCommand>(raw).unwrap(),
            command
        );
        assert!(serde_json::from_value::<CreatePreparation>(json!({"id":uuid::Uuid::new_v4(),"endpoint_id":"host","recipe_id":"recipe","revision_digest":"a".repeat(64),"command":["unsafe"]})).is_err());
    }
}

/// Map the serving cache through its actual writable bind, so a preparation
/// receipt cannot describe files that the selected inference recipe cannot see.
fn recipe_cache_directory(spec: &Value) -> Result<String, String> {
    let home = spec
        .pointer("/environment/HF_HOME")
        .and_then(Value::as_str)
        .unwrap_or("/root/.cache/huggingface")
        .trim_end_matches('/');
    let default_hub = format!("{home}/hub");
    let hub = spec
        .pointer("/environment/HF_HUB_CACHE")
        .and_then(Value::as_str)
        .unwrap_or(&default_hub)
        .trim_end_matches('/');
    if !hub.starts_with('/') || !hub.ends_with("/hub") {
        return Err(
            "Preparation requires a standard HF_HOME/hub cache bound into the serving container"
                .into(),
        );
    }
    let mounts = spec
        .get("mounts")
        .and_then(Value::as_array)
        .ok_or("Recipe has no HF cache bind mount")?;
    let best = mounts
        .iter()
        .filter_map(|m| {
            let target = m["target"].as_str()?.trim_end_matches('/');
            if hub != target
                && !hub
                    .strip_prefix(target)
                    .is_some_and(|tail| tail.starts_with('/'))
            {
                return None;
            };
            Some((target.len(), target, m))
        })
        .max_by_key(|(len, _, _)| *len)
        .ok_or("Recipe HF cache has no matching host bind")?;
    if best.2["read_only"].as_bool().unwrap_or(false)
        || best.2["per_model"].as_bool().unwrap_or(false)
    {
        return Err("Preparation requires a writable shared HF cache bind".into());
    };
    let source = best.2["source"]
        .as_str()
        .filter(|p| p.starts_with('/'))
        .ok_or("Recipe HF cache must use an absolute host path")?
        .trim_end_matches('/');
    let host_hub = format!("{}{}", source, &hub[best.1.len()..]);
    host_hub
        .strip_suffix("/hub")
        .map(str::to_string)
        .ok_or("Recipe HF cache must map to the host's standard hub directory".into())
}
#[cfg(test)]
mod cache_binding_tests {
    use super::*;
    #[test]
    fn checkpoint_cache_matches_actual_recipe_mount() {
        assert_eq!(
            recipe_cache_directory(
                &json!({"environment":{"HF_HOME":"/hf"},"mounts":[{"source":"/hf","target":"/hf"}]})
            )
            .unwrap(),
            "/hf"
        );
        assert_eq!(recipe_cache_directory(&json!({"mounts":[{"source":"/home/koen/.cache/huggingface","target":"/root/.cache/huggingface"}]})).unwrap(),"/home/koen/.cache/huggingface");
        assert!(recipe_cache_directory(&json!({"environment":{"HF_HOME":"/different"},"mounts":[{"source":"/hf","target":"/hf"}]})).is_err());
        assert!(recipe_cache_directory(&json!({"environment":{"HF_HOME":"/hf"},"mounts":[{"source":"/hf","target":"/hf","read_only":true}]})).is_err())
    }
}

//! Central immutable recipes and durable desired deployments. The host executes
//! commands through its authenticated mesh worker; local policy remains in Fleet.
use crate::control_plane_store::PersistenceStore;
use crate::dashboard_api::json_no_store;
use crate::dashboard_auth::{AuthSession, DashboardAuth};
use crate::dashboard_mesh::{authorize_mesh_admin_read, authorize_mesh_mutation};
use crate::engine::Gateway;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use utoipa::ToSchema;
const KEY: &str = "fleet_management_v1";

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ManagementState {
    pub recipes: BTreeMap<String, Vec<RecipeRevision>>,
    pub deployments: BTreeMap<String, DesiredDeployment>,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RecipeRevision {
    pub digest: String,
    pub revision: u64,
    pub description: String,
    pub model_spec: Value,
    pub created_at_ms: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRecipeRevision {
    pub description: String,
    pub model_spec: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectDeployment {
    pub expected_generation: u64,
    pub endpoint_id: String,
    pub recipe_id: String,
    pub revision_digest: String,
    pub desired_state: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DesiredDeployment {
    pub generation: u64,
    pub endpoint_id: String,
    pub recipe_id: String,
    pub revision_digest: String,
    pub desired_state: String,
    pub updated_at_ms: i64,
    pub observed_at_ms: Option<i64>,
    pub observed: Option<Value>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManagedCommand {
    pub deployment_id: Option<String>,
    pub desired: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedReply {
    pub request_id: uuid::Uuid,
    pub result: Option<Value>,
    pub error: Option<String>,
}
fn error(status: StatusCode, message: &str) -> Response {
    json_no_store(status, &serde_json::json!({"error":message}))
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && (b == b'-' || b == b'_'))
        })
}
fn validate_recipe(body: &CreateRecipeRevision) -> Result<String, &'static str> {
    if body.description.len() > 256 {
        return Err("description exceeds 256 bytes");
    }
    let Some(spec) = body.model_spec.as_object() else {
        return Err("model_spec must be an object");
    };
    let allowed = [
        "description",
        "image",
        "command",
        "environment",
        "mounts",
        "ports",
        "gpus",
        "shm_size",
        "ipc",
        "entrypoint",
        "network_mode",
        "memory_limit",
        "memory_swap_limit",
        "security_opt",
        "ulimits",
        "restart",
        "init",
        "user",
        "read_only",
        "stop_timeout",
        "readiness",
    ];
    if spec.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("unknown or host-owned model field");
    }
    let Some(image) = spec.get("image").and_then(Value::as_str) else {
        return Err("an image is required");
    };
    if image.is_empty()
        || image.len() > 512
        || image.starts_with('-')
        || image.chars().any(char::is_whitespace)
        || !image.rsplit_once("@sha256:").is_some_and(|(_, digest)| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return Err("image must be pinned by its sha256 digest");
    }
    if let Some(env) = spec.get("environment") {
        let Some(env) = env.as_object() else {
            return Err("environment must be an object");
        };
        if env.iter().any(|(key, value)| {
            let upper = key.to_ascii_uppercase();
            !value.is_string()
                || ["TOKEN", "SECRET", "PASSWORD", "API_KEY"]
                    .iter()
                    .any(|s| upper.contains(s))
        }) {
            return Err("inline credentials are not permitted; use a host-local mount");
        }
    }
    for key in [
        "description",
        "gpus",
        "shm_size",
        "ipc",
        "entrypoint",
        "network_mode",
        "memory_limit",
        "memory_swap_limit",
        "restart",
        "user",
        "stop_timeout",
    ] {
        if spec.get(key).is_some_and(|value| !value.is_string()) {
            return Err("recipe string field has an invalid type");
        }
    }
    for key in ["init", "read_only"] {
        if spec.get(key).is_some_and(|value| !value.is_boolean()) {
            return Err("recipe boolean field has an invalid type");
        }
    }
    for key in ["command", "security_opt", "ulimits"] {
        if let Some(value) = spec.get(key)
            && !value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string))
        {
            return Err("recipe argument list must contain strings");
        }
    }
    for key in ["memory_limit", "user"] {
        if !spec
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            return Err("memory_limit and user are required");
        }
    }
    for key in ["mounts", "ports"] {
        if let Some(value) = spec.get(key)
            && !value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_object))
        {
            return Err("mounts and ports must contain objects");
        }
    }
    if spec
        .get("readiness")
        .is_some_and(|value| !value.is_object())
    {
        return Err("readiness must be an object");
    }
    let bytes = serde_json::to_vec(spec).map_err(|_| "invalid model specification")?;
    if bytes.len() > 32 * 1024 {
        return Err("model specification exceeds 32 KiB");
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
async fn read(store: &dyn PersistenceStore) -> Result<(Option<String>, ManagementState), String> {
    let raw = store.get_setting(KEY).await?;
    let state = match &raw {
        Some(raw) => serde_json::from_str(raw).map_err(|_| "invalid stored management state")?,
        None => ManagementState::default(),
    };
    Ok((raw, state))
}
async fn commit(
    store: &dyn PersistenceStore,
    expected: Option<&str>,
    state: &ManagementState,
) -> Result<bool, String> {
    let raw = serde_json::to_string(state).map_err(|_| "could not encode management state")?;
    if raw.len() > 4 * 1024 * 1024 {
        return Err("management catalog exceeds capacity".into());
    }
    store.compare_exchange_setting(KEY, expected, &raw).await
}

#[utoipa::path(get,path="/dashboard/api/management",responses((status=200,body=ManagementState),(status=401),(status=403),(status=503)),tag="dashboard")]
pub async fn management_state(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return response;
    }
    let Some(store) = gateway.persistence_store() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SQL persistence is required",
        );
    };
    match read(store.as_ref()).await {
        Ok((_, state)) => json_no_store(StatusCode::OK, &state),
        Err(err) => {
            tracing::error!(error=%err,"read management catalog");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "management state unavailable",
            )
        }
    }
}
#[utoipa::path(post,path="/dashboard/api/management/recipes/{id}/revisions",params(("id"=String,Path)),request_body=CreateRecipeRevision,responses((status=201,body=RecipeRevision),(status=400),(status=401),(status=403),(status=409),(status=503)),tag="dashboard")]
pub async fn create_revision(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateRecipeRevision>,
) -> Response {
    if let Some(response) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return response;
    }
    if !valid_id(&id) {
        return error(StatusCode::BAD_REQUEST, "invalid recipe id");
    }
    let digest = match validate_recipe(&body) {
        Ok(digest) => digest,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
    };
    let Some(store) = gateway.persistence_store() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SQL persistence is required",
        );
    };
    for _ in 0..4 {
        let (raw, mut state) = match read(store.as_ref()).await {
            Ok(value) => value,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "management state unavailable",
                );
            }
        };
        if state.recipes.len() >= 64 && !state.recipes.contains_key(&id) {
            return error(StatusCode::CONFLICT, "recipe limit reached");
        }
        let revisions = state.recipes.entry(id.clone()).or_default();
        if let Some(revision) = revisions.iter().find(|revision| revision.digest == digest) {
            return json_no_store(StatusCode::OK, revision);
        }
        if revisions.len() >= 32 {
            return error(StatusCode::CONFLICT, "revision limit reached");
        }
        let revision = RecipeRevision {
            digest: digest.clone(),
            revision: revisions.len() as u64 + 1,
            description: body.description.clone(),
            model_spec: body.model_spec.clone(),
            created_at_ms: crate::mesh::store::now_ms(),
        };
        revisions.push(revision.clone());
        match commit(store.as_ref(), raw.as_deref(), &state).await {
            Ok(true) => return json_no_store(StatusCode::CREATED, &revision),
            Ok(false) => continue,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not persist revision",
                );
            }
        }
    }
    error(StatusCode::CONFLICT, "catalog changed; retry the request")
}
#[utoipa::path(put,path="/dashboard/api/management/deployments/{id}",params(("id"=String,Path)),request_body=SelectDeployment,responses((status=202,body=DesiredDeployment),(status=400),(status=401),(status=403),(status=409),(status=503)),tag="dashboard")]
pub async fn select_deployment(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SelectDeployment>,
) -> Response {
    if let Some(response) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return response;
    }
    if !valid_id(&id)
        || !valid_id(&body.recipe_id)
        || !matches!(body.desired_state.as_str(), "ready" | "unloaded")
        || body.endpoint_id.parse::<iroh::EndpointId>().is_err()
    {
        return error(StatusCode::BAD_REQUEST, "invalid deployment selection");
    }
    let Some(store) = gateway.persistence_store() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SQL persistence is required",
        );
    };
    let Some(admin) = gateway.mesh_admin() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "mesh controller is required",
        );
    };
    if admin.initialize().await.is_err() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "mesh state unavailable");
    }
    let nodes = match admin.store().list_nodes().await {
        Ok(nodes) => nodes,
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "mesh state unavailable"),
    };
    if !nodes
        .iter()
        .any(|node| node.endpoint_id == body.endpoint_id && node.enabled)
    {
        return error(StatusCode::BAD_REQUEST, "select an enrolled, enabled host");
    }
    for _ in 0..4 {
        let (raw, mut state) = match read(store.as_ref()).await {
            Ok(value) => value,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "management state unavailable",
                );
            }
        };
        if !state
            .recipes
            .get(&body.recipe_id)
            .is_some_and(|revisions| revisions.iter().any(|r| r.digest == body.revision_digest))
        {
            return error(StatusCode::BAD_REQUEST, "recipe revision does not exist");
        }
        if state.deployments.len() >= 64 && !state.deployments.contains_key(&id) {
            return error(StatusCode::CONFLICT, "deployment limit reached");
        }
        let current = state.deployments.get(&id);
        if current.map_or(0, |d| d.generation) != body.expected_generation {
            return error(StatusCode::CONFLICT, "deployment generation changed");
        }
        if let Some(current) = current {
            if current.endpoint_id != body.endpoint_id {
                return error(
                    StatusCode::CONFLICT,
                    "a deployment cannot move hosts; unload it and create a new deployment",
                );
            }
            if current.revision_digest != body.revision_digest
                && (current.desired_state != "unloaded"
                    || current
                        .observed
                        .as_ref()
                        .and_then(|v| v.pointer("/status/phase"))
                        .and_then(Value::as_str)
                        != Some("unloaded"))
            {
                return error(
                    StatusCode::CONFLICT,
                    "wait for the host to unload before selecting a different recipe revision",
                );
            }
        }
        let Some(generation) = body.expected_generation.checked_add(1) else {
            return error(StatusCode::CONFLICT, "generation exhausted");
        };
        let deployment = DesiredDeployment {
            generation,
            endpoint_id: body.endpoint_id.clone(),
            recipe_id: body.recipe_id.clone(),
            revision_digest: body.revision_digest.clone(),
            desired_state: body.desired_state.clone(),
            updated_at_ms: crate::mesh::store::now_ms(),
            observed_at_ms: None,
            observed: None,
            error: None,
        };
        state.deployments.insert(id.clone(), deployment.clone());
        match commit(store.as_ref(), raw.as_deref(), &state).await {
            Ok(true) => return json_no_store(StatusCode::ACCEPTED, &deployment),
            Ok(false) => continue,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not persist deployment",
                );
            }
        }
    }
    error(StatusCode::CONFLICT, "catalog changed; retry the request")
}
pub(crate) fn start_reconciler(gateway: Arc<Gateway>) {
    if gateway.mesh_admin().is_none() || gateway.persistence_store().is_none() {
        return;
    }
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(5));
        loop {
            timer.tick().await;
            reconcile(&gateway).await;
        }
    });
}
async fn reconcile(gateway: &Gateway) {
    let Some(store) = gateway.persistence_store() else {
        return;
    };
    let Ok((_, state)) = read(store.as_ref()).await else {
        return;
    };
    futures::stream::iter(&state.deployments)
        .for_each_concurrent(Some(4), |(id, deployment)| {
            reconcile_deployment(gateway, &state, id, deployment)
        })
        .await;
}

async fn reconcile_deployment(
    gateway: &Gateway,
    state: &ManagementState,
    id: &str,
    deployment: &DesiredDeployment,
) {
    let Some(store) = gateway.persistence_store() else {
        return;
    };
    let Some(admin) = gateway.mesh_admin() else {
        return;
    };

    let Some(revision) = state
        .recipes
        .get(&deployment.recipe_id)
        .and_then(|revs| revs.iter().find(|r| r.digest == deployment.revision_digest))
    else {
        return;
    };
    if !admin
        .store()
        .is_node_authorized(&deployment.endpoint_id)
        .await
        .unwrap_or(false)
    {
        return;
    }
    let Ok(endpoint) = deployment.endpoint_id.parse() else {
        return;
    };
    let mut model = revision.model_spec.clone();
    model["id"] = Value::String(format!("managed-{id}"));
    let desired = serde_json::json!({"generation":deployment.generation,"recipe_digest":deployment.revision_digest,"desired_state":deployment.desired_state,"model":model});
    let result = admin
        .registry()
        .manage_fleet(
            endpoint,
            ManagedCommand {
                deployment_id: Some(id.to_string()),
                desired: Some(desired),
            },
        )
        .await;
    // A timed-out or rejected command is retried with the same generation. The
    // node retains accepted state even while its controller is disconnected.
    let (observed, message) = match result {
        Ok(reply) => (reply.result, reply.error),
        Err(_) => (
            None,
            Some("host is unavailable or does not support managed deployments".to_string()),
        ),
    };
    for _ in 0..3 {
        let Ok((raw, mut latest)) = read(store.as_ref()).await else {
            break;
        };
        let Some(current) = latest.deployments.get_mut(id) else {
            break;
        };
        if current.generation != deployment.generation {
            break;
        }
        current.observed = observed.clone();
        current.error = message.clone();
        current.observed_at_ms = Some(crate::mesh::store::now_ms());
        match commit(store.as_ref(), raw.as_deref(), &latest).await {
            Ok(false) => continue,
            _ => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn management_compare_exchange_preserves_concurrent_updates() {
        let store = crate::control_plane_store::SqlStore::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        assert!(
            store
                .compare_exchange_setting(KEY, None, "first")
                .await
                .unwrap()
        );
        assert!(
            !store
                .compare_exchange_setting(KEY, None, "lost")
                .await
                .unwrap()
        );
        assert!(
            !store
                .compare_exchange_setting(KEY, Some("wrong"), "lost")
                .await
                .unwrap()
        );
        let (one, two) = tokio::join!(
            store.compare_exchange_setting(KEY, Some("first"), "second"),
            store.compare_exchange_setting(KEY, Some("first"), "third")
        );
        assert_ne!(one.unwrap(), two.unwrap());
        let current = store.get_setting(KEY).await.unwrap().unwrap();
        assert!(current == "second" || current == "third");
    }
    #[test]
    fn recipe_digest_is_canonical_and_credentials_are_rejected() {
        let mut body = CreateRecipeRevision {
            description: "fixture".into(),
            model_spec: serde_json::json!({"image":"example@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","command":["serve"],"user":"1000:1000","memory_limit":"128m"}),
        };
        let digest = validate_recipe(&body).unwrap();
        let encoded = serde_json::to_string(&body).unwrap();
        let decoded: CreateRecipeRevision = serde_json::from_str(&encoded).unwrap();
        assert_eq!(validate_recipe(&decoded).unwrap(), digest);
        body.model_spec["environment"] = serde_json::json!({"HF_TOKEN":"private"});
        assert!(validate_recipe(&body).is_err());
        body.model_spec = serde_json::json!({"image":"example","privileged":true});
        assert!(validate_recipe(&body).is_err());
    }
    #[test]
    fn management_wire_fields_round_trip() {
        let command = ManagedCommand {
            deployment_id: Some("fixture".into()),
            desired: Some(serde_json::json!({"generation":1,"model":{"image":"fixture"}})),
        };
        assert_eq!(
            serde_json::from_value::<ManagedCommand>(serde_json::to_value(&command).unwrap())
                .unwrap(),
            command
        );
        let reply = ManagedReply {
            request_id: uuid::Uuid::new_v4(),
            result: Some(serde_json::json!({"status":{"phase":"ready"}})),
            error: None,
        };
        assert_eq!(
            serde_json::from_value::<ManagedReply>(serde_json::to_value(&reply).unwrap()).unwrap(),
            reply
        );
    }
}

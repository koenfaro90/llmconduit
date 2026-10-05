//! Central immutable recipes and durable desired deployments. The host executes
//! commands through its authenticated mesh worker; local policy remains in Fleet.
use crate::control_plane_store::PersistenceStore;
use crate::dashboard_api::json_no_store;
use crate::dashboard_auth::{AuthSession, DashboardAuth};
use crate::dashboard_mesh::{authorize_mesh_admin_read, authorize_mesh_mutation};
use crate::engine::Gateway;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
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
    #[serde(default)]
    pub archived_recipes: Vec<String>,
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
    #[serde(default)]
    pub size: Option<String>,
    pub expected_generation: u64,
    pub endpoint_id: String,
    pub recipe_id: String,
    pub revision_digest: String,
    pub desired_state: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DesiredDeployment {
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<crate::dashboard_deployment_groups::Placement>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<crate::dashboard_benchmarks::BenchmarkCommand>,
    pub deployment_id: Option<String>,
    pub desired: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedReply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_status: Option<u16>,
    pub request_id: uuid::Uuid,
    pub result: Option<Value>,
    pub error: Option<String>,
}
fn error(status: StatusCode, message: &str) -> Response {
    json_no_store(status, &serde_json::json!({"error":message}))
}
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && (b == b'-' || b == b'_'))
        })
}
pub(crate) fn credential_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key == "TOKEN"
        || key.ends_with("_TOKEN")
        || ["SECRET", "PASSWORD", "API_KEY"]
            .iter()
            .any(|part| key.contains(part))
}
pub(crate) fn source_has_credentials(value: &serde_yaml::Value) -> bool {
    fn literal(value: &serde_yaml::Value) -> bool {
        !value.is_null()
            && !value.as_str().is_some_and(|v| {
                v.is_empty()
                    || v == "[redacted]"
                    || regex::Regex::new(r"^\$\{[A-Za-z_][A-Za-z0-9_]*(?::?\?[^}]*)?(?::?-)?\}$")
                        .unwrap()
                        .is_match(v)
            })
    }
    match value {
        serde_yaml::Value::Mapping(map) => map.iter().any(|(key, value)| {
            key.as_str()
                .is_some_and(|key| credential_key(key) && literal(value))
                || source_has_credentials(value)
        }),
        serde_yaml::Value::Sequence(items) => items.iter().any(source_has_credentials),
        serde_yaml::Value::Tagged(value) => source_has_credentials(&value.value),
        serde_yaml::Value::String(text) => text.split_once('=').is_some_and(|(key, value)| {
            credential_key(key) && literal(&serde_yaml::Value::String(value.to_string()))
        }),
        _ => false,
    }
}
/// Catalog annotations are included in the revision identity, but stripped before
/// execution. Image provenance must refer to an image present in this recipe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecipeMetadata {
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub size_name: Option<String>,
    #[serde(default)]
    pub size_source_digest: Option<String>,
    #[serde(default)]
    pub memory_estimate_note: Option<String>,
    #[serde(default)]
    pub gpu_families: Vec<String>,
    #[serde(default)]
    pub image_sources: BTreeMap<String, String>,
    #[serde(default)]
    pub image_source_urls: BTreeMap<String, String>,
}
pub(crate) fn matches_hardware(
    spec: &Value,
    snapshot: Option<&crate::dashboard_fleet_host::FleetHostSnapshot>,
) -> bool {
    let families = spec
        .pointer("/metadata/gpu_families")
        .and_then(Value::as_array);
    let Some(families) = families.filter(|families| !families.is_empty()) else {
        return true;
    };
    snapshot.is_some_and(|snapshot| {
        snapshot.gpus.iter().any(|gpu| {
            let upper = gpu.name.to_ascii_uppercase();
            let family = if upper.contains("GB10") || gpu.unified_memory {
                "spark"
            } else if upper.contains("RTX")
                && upper.contains("PRO")
                && upper.contains("6000")
                && upper.contains("BLACKWELL")
            {
                "rtx-pro-6000-blackwell"
            } else {
                gpu.name.as_str()
            };
            families.iter().any(|value| value.as_str() == Some(family))
        })
    })
}
fn validate_metadata(spec: &serde_json::Map<String, Value>) -> Result<(), &'static str> {
    let Some(value) = spec.get("metadata") else {
        return Ok(());
    };
    let metadata: RecipeMetadata =
        serde_json::from_value(value.clone()).map_err(|_| "invalid recipe metadata")?;
    if metadata
        .source_url
        .as_ref()
        .is_some_and(|v| crate::recipe_import::lil_source_url(v).is_err())
    {
        return Err("invalid recipe source URL");
    }
    if metadata.size_name.as_ref().is_some_and(|v| !valid_id(v))
        || metadata
            .size_source_digest
            .as_ref()
            .is_some_and(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
        || metadata
            .memory_estimate_note
            .as_ref()
            .is_some_and(|v| v.len() > 1024 || v.chars().any(char::is_control))
    {
        return Err("invalid size/memory metadata");
    }
    if metadata.gpu_families.len() > 8
        || metadata
            .gpu_families
            .iter()
            .any(|v| v.is_empty() || v.len() > 128 || v.chars().any(char::is_control))
    {
        return Err("GPU families must be bounded, nonempty names");
    }
    let mut images = Vec::new();
    if let Some(ranks) = spec.get("distributed").and_then(Value::as_array) {
        images.extend(
            ranks
                .iter()
                .filter_map(|rank| rank.get("image").and_then(Value::as_str)),
        );
    } else if let Some(image) = spec.get("image").and_then(Value::as_str) {
        images.push(image);
    }
    if metadata.image_sources.len() > 8 || metadata.image_source_urls.len() > 8 {
        return Err("too many image sources");
    }
    for (image, source) in &metadata.image_sources {
        if !images.contains(&image.as_str())
            || source.is_empty()
            || source.len() > 512
            || source.chars().any(char::is_whitespace)
            || source.contains('@')
        {
            return Err(
                "image source must be a readable, credential-free image name present in the recipe",
            );
        }
    }
    for (image, source) in &metadata.image_source_urls {
        let url = url::Url::parse(source).map_err(|_| "image source URL must be HTTP(S)")?;
        if !images.contains(&image.as_str())
            || source.len() > 1024
            || !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "image source URL must be a credential-free HTTP(S) source for a recipe image",
            );
        }
    }
    Ok(())
}
pub(crate) fn validate_recipe(body: &CreateRecipeRevision) -> Result<String, &'static str> {
    if body.description.len() > 256 {
        return Err("description exceeds 256 bytes");
    }
    let Some(spec) = body.model_spec.as_object() else {
        return Err("model_spec must be an object");
    };
    validate_metadata(spec)?;
    crate::dashboard_recipe_sizes::validate(&body.model_spec)?;
    if let Some(ranks) = spec.get("distributed") {
        let Some(ranks) = ranks.as_array() else {
            return Err("distributed must be an array of rank specifications");
        };
        if spec
            .keys()
            .any(|key| !matches!(key.as_str(), "distributed" | "metadata" | "sizes"))
            || !(2..=8).contains(&ranks.len())
            || ranks
                .iter()
                .any(|r| r.get("distributed").is_some() || r.get("source_format").is_some())
        {
            return Err("distributed recipes require 2..8 executable ranks");
        }
        for rank in ranks {
            validate_recipe(&CreateRecipeRevision {
                description: String::new(),
                model_spec: rank.clone(),
            })?;
        }
        let bytes = serde_json::to_vec(spec).map_err(|_| "invalid distributed recipe")?;
        return Ok(format!("{:x}", Sha256::digest(bytes)));
    }
    // Historical sources stay editable centrally without pretending that an
    // unrendered Compose/RDMA recipe is an executable deployment.
    if let Some(format) = spec.get("source_format") {
        if !matches!(format.as_str(), Some("compose" | "sparkrun"))
            || spec.len() != 2
            || !spec
                .get("source_content")
                .and_then(Value::as_str)
                .is_some_and(|content| !content.is_empty() && content.len() <= 32 * 1024)
        {
            return Err("source recipes require a format and bounded source_content");
        }
        let content = spec["source_content"].as_str().unwrap();
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(content).map_err(|_| "source content must be valid YAML")?;
        if source_has_credentials(&yaml)
            || regex::Regex::new(r"hf_[A-Za-z0-9]{16,}")
                .unwrap()
                .is_match(content)
        {
            return Err("source credentials must be replaced by host-local references");
        }
        let bytes = serde_json::to_vec(spec).map_err(|_| "invalid source recipe")?;
        return Ok(format!("{:x}", Sha256::digest(bytes)));
    }
    let allowed = [
        "metadata",
        "sizes",
        "gpu_memory",
        "description",
        "image",
        "command",
        "environment",
        "environment_files",
        "devices",
        "group_add",
        "privileged",
        "cap_add",
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
    if let Some(memory) = spec.get("gpu_memory") {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Memory {
            mode: String,
            #[serde(default)]
            mib_per_device: u64,
            #[serde(default)]
            fraction: f64,
        }
        let memory: Memory =
            serde_json::from_value(memory.clone()).map_err(|_| "invalid GPU memory reservation")?;
        if !matches!(memory.mode.as_str(), "exclusive" | "shared")
            || memory.mib_per_device > 1 << 30
            || !memory.fraction.is_finite()
            || !(0.0..=1.0).contains(&memory.fraction)
            || memory.mode == "shared" && memory.mib_per_device == 0 && memory.fraction == 0.0
        {
            return Err("invalid GPU memory reservation");
        }
    }
    let Some(image) = spec.get("image").and_then(Value::as_str) else {
        return Err("an image is required");
    };
    if image.is_empty()
        || image.len() > 512
        || image.starts_with('-')
        || image.chars().any(char::is_whitespace)
        || !image
            .rsplit_once("@sha256:")
            .map(|(_, digest)| digest)
            .or_else(|| image.strip_prefix("sha256:"))
            .is_some_and(|digest| {
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
        if env
            .iter()
            .any(|(key, value)| !value.is_string() || credential_key(key))
        {
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
    for key in ["init", "read_only", "privileged"] {
        if spec.get(key).is_some_and(|value| !value.is_boolean()) {
            return Err("recipe boolean field has an invalid type");
        }
    }
    for key in [
        "command",
        "security_opt",
        "ulimits",
        "environment_files",
        "devices",
        "group_add",
        "cap_add",
    ] {
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
#[utoipa::path(post,path="/dashboard/api/management/recipes/convert",request_body=crate::recipe_import::ConvertRecipeRequest,responses((status=200,body=crate::recipe_import::ConvertedRecipe),(status=400),(status=401),(status=403)),tag="dashboard")]
pub async fn convert_recipe(
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<crate::recipe_import::ConvertRecipeRequest>,
) -> Response {
    // Preview is side-effect-free, but uses the same admin/CSRF boundary as imports.
    if let Some(response) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return response;
    }
    match crate::recipe_import::convert_portable(&body) {
        Ok(mut preview) => {
            if preview.required_inputs.is_empty() {
                let revision = CreateRecipeRevision {
                    description: String::new(),
                    model_spec: preview.model_spec.clone(),
                };
                if let Err(message) = validate_recipe(&revision) {
                    preview.required_inputs.push(message.to_string());
                }
            }
            json_no_store(StatusCode::OK, &preview)
        }
        Err(message) => error(StatusCode::BAD_REQUEST, &message),
    }
}

#[utoipa::path(post,path="/dashboard/api/management/recipes/source",request_body=crate::recipe_import::FetchRecipeSource,responses((status=200,body=crate::recipe_import::ImportedRecipeSource),(status=400),(status=401),(status=403),(status=502)),tag="dashboard")]
pub async fn fetch_recipe_source(
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<crate::recipe_import::FetchRecipeSource>,
) -> Response {
    if let Some(response) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return response;
    }
    if let Err(message) = crate::recipe_import::lil_source_url(&body.url) {
        return error(StatusCode::BAD_REQUEST, &message);
    }
    match crate::recipe_import::fetch_lil_source(&body.url).await {
        Ok(source) => json_no_store(StatusCode::OK, &source),
        Err(message) => error(StatusCode::BAD_GATEWAY, &message),
    }
}

pub(crate) async fn read(
    store: &dyn PersistenceStore,
) -> Result<(Option<String>, ManagementState), String> {
    let raw = store.get_setting(KEY).await?;
    let state = match &raw {
        Some(raw) => serde_json::from_str(raw).map_err(|_| "invalid stored management state")?,
        None => ManagementState::default(),
    };
    Ok((raw, state))
}
pub(crate) async fn commit(
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
#[utoipa::path(get,path="/dashboard/api/management/model-activity",responses((status=200,body=Vec<crate::control_plane_store::ModelActivity>),(status=401),(status=403),(status=503)),tag="dashboard")]
pub async fn model_activity(
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
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        store.model_activity(crate::mesh::store::now_ms()),
    )
    .await
    {
        Ok(Ok(rows)) => json_no_store(StatusCode::OK, &rows),
        _ => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "model request activity unavailable",
        ),
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
        if state.recipes.len() >= 256 && !state.recipes.contains_key(&id) {
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
        if !state.recipes[&body.recipe_id]
            .iter()
            .find(|r| r.digest == body.revision_digest)
            .is_some_and(|r| r.model_spec.get("image").is_some())
        {
            return error(
                StatusCode::BAD_REQUEST,
                "render the source recipe into an executable revision before deployment",
            );
        }
        let revision = match crate::dashboard_recipe_sizes::pin(
            &mut state,
            &body.recipe_id,
            &body.revision_digest,
            body.size.as_deref(),
        ) {
            Ok(revision) => revision,
            Err(message) => return error(StatusCode::BAD_REQUEST, message),
        };
        if body.desired_state == "ready"
            && !matches_hardware(
                &revision.model_spec,
                body.endpoint_id
                    .parse()
                    .ok()
                    .and_then(|endpoint| admin.registry().host_snapshot(endpoint))
                    .as_ref(),
            )
        {
            return error(
                StatusCode::CONFLICT,
                "recipe GPU families do not match the host's detected hardware, or the host observation is unavailable",
            );
        }
        if state.deployments.len() >= 64 && !state.deployments.contains_key(&id) {
            return error(StatusCode::CONFLICT, "deployment limit reached");
        }
        let current = state.deployments.get(&id);
        if current.map_or(0, |d| d.generation) != body.expected_generation {
            return error(StatusCode::CONFLICT, "deployment generation changed");
        }
        if let Some(current) = current
            && (current.revision_digest != revision.digest
                || current.endpoint_id != body.endpoint_id)
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
                "wait for the host to unload before selecting a different recipe revision or host",
            );
        }
        let Some(generation) = body.expected_generation.checked_add(1) else {
            return error(StatusCode::CONFLICT, "generation exhausted");
        };
        let deployment = DesiredDeployment {
            generation,
            placement: current.and_then(|d| d.placement.clone()),
            endpoint_id: body.endpoint_id.clone(),
            recipe_id: body.recipe_id.clone(),
            revision_digest: revision.digest.clone(),
            desired_state: body.desired_state.clone(),
            updated_at_ms: crate::mesh::store::now_ms(),
            observed_at_ms: None,
            observed: None,
            error: None,
        };
        if let Err(message) = crate::dashboard_resource_admission::check(
            &state,
            &id,
            &deployment,
            body.endpoint_id
                .parse()
                .ok()
                .and_then(|endpoint| admin.registry().host_snapshot(endpoint))
                .as_ref(),
        ) {
            return error(StatusCode::CONFLICT, &message);
        }
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
    let mut model = match crate::dashboard_deployment_groups::render(
        &revision.model_spec,
        deployment.placement.as_ref(),
    ) {
        Ok(model) => model,
        Err(error) => {
            tracing::warn!(%error, deployment_id=id,"invalid deployment placement");
            return;
        }
    };
    model["id"] = Value::String(format!("managed-{id}"));
    let desired = serde_json::json!({"generation":deployment.generation,"recipe_digest":deployment.revision_digest,"desired_state":deployment.desired_state,"model":model});
    let result = admin
        .registry()
        .manage_fleet(
            endpoint,
            ManagedCommand {
                settings: None,
                benchmark: None,
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
    if observed
        .as_ref()
        .and_then(|v| v.pointer("/status/phase"))
        .and_then(Value::as_str)
        == Some("ready")
    {
        let record = crate::dashboard_recipe_evidence::Record {
            id: format!("ready:{id}:{}", deployment.generation),
            recipe_id: deployment.recipe_id.clone(),
            revision_digest: deployment.revision_digest.clone(),
            endpoint_id: deployment.endpoint_id.clone(),
            kind: "ready".into(),
            recorded_at_ms: chrono::Utc::now().timestamp_millis(),
            data: serde_json::json!({"deployment_id":id,"generation":deployment.generation,"image":revision.model_spec.get("image"),"phase":"ready","inference_tested":false}),
        };
        if let Err(error) = store.record_recipe_evidence(record).await {
            tracing::warn!(%error,"could not store recipe readiness evidence");
        }
    }
    // Four hosts reconcile concurrently against this one catalog row. Three
    // CAS attempts can starve the fourth writer and leave healthy hosts stale.
    persist_observation(store.as_ref(), id, deployment.generation, observed, message).await;
}

async fn persist_observation(
    store: &dyn PersistenceStore,
    id: &str,
    generation: u64,
    observed: Option<Value>,
    message: Option<String>,
) {
    for _ in 0..8 {
        let Ok((raw, mut latest)) = read(store).await else {
            break;
        };
        let Some(current) = latest.deployments.get_mut(id) else {
            break;
        };
        if current.generation != generation {
            break;
        }
        current.observed = observed.clone();
        current.error = message.clone();
        current.observed_at_ms = Some(crate::mesh::store::now_ms());
        match commit(store, raw.as_deref(), &latest).await {
            Ok(false) => continue,
            _ => break,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn catalog_adoption_preserves_execution_and_rejects_changes_atomically() {
        let mut state:ManagementState=serde_json::from_value(serde_json::json!({"recipes":{"old":[{"digest":"old","revision":1,"description":"old","model_spec":{"image":"same","command":["--host","10.0.0.1"]},"created_at_ms":1}],"portable":[{"digest":"new","revision":1,"description":"new","model_spec":{"image":"same","command":["--host","{{host_address}}"]},"created_at_ms":1}]},"deployments":{"dep":{"endpoint_id":"host","recipe_id":"old","revision_digest":"old","generation":1,"desired_state":"ready","updated_at_ms":1,"observed_at_ms":1,"observed":null,"error":null}}})).unwrap();
        let body:CatalogAdoption=serde_json::from_value(serde_json::json!({"deployments":{"dep":{"expected_generation":1,"recipe_id":"portable","revision_digest":"new","placement":{"group_id":"group","rank":0,"addresses":["10.0.0.1"]}}},"archive_recipes":["old"]})).unwrap();
        adopt_catalog(&mut state, &body).unwrap();
        assert_eq!(state.deployments["dep"].generation, 2);
        assert_eq!(state.archived_recipes, vec!["old"]);
        let round_trip: ManagementState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(round_trip.archived_recipes, vec!["old"]);
        assert_eq!(
            round_trip.deployments["dep"].placement,
            state.deployments["dep"].placement
        );
        let before = serde_json::to_value(&state).unwrap();
        assert!(adopt_catalog(&mut state, &body).is_err());
        assert_eq!(before, serde_json::to_value(&state).unwrap());
    }

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
    #[tokio::test]
    async fn concurrent_observations_retain_every_host_and_ignore_old_generations() {
        let store = crate::control_plane_store::SqlStore::connect_sqlite("sqlite::memory:")
            .await
            .unwrap();
        let mut state = ManagementState::default();
        for id in ["a", "b", "c", "d"] {
            state.deployments.insert(
                id.into(),
                serde_json::from_value(serde_json::json!({
                    "generation": 2, "endpoint_id": id, "recipe_id": "test",
                    "revision_digest": "digest", "desired_state": "ready", "updated_at_ms": 1
                }))
                .unwrap(),
            );
        }
        assert!(commit(&store, None, &state).await.unwrap());
        futures::future::join_all(["a", "b", "c", "d"].into_iter().map(|id| {
            persist_observation(&store, id, 2, Some(serde_json::json!({"host": id})), None)
        }))
        .await;
        persist_observation(&store, "a", 1, None, Some("obsolete".into())).await;
        let (_, state) = read(&store).await.unwrap();
        for (id, deployment) in state.deployments {
            assert_eq!(deployment.observed, Some(serde_json::json!({"host": id})));
            assert!(deployment.observed_at_ms.is_some());
            assert!(deployment.error.is_none());
        }
    }
    #[test]
    fn declared_hardware_requires_detected_gpu_family() {
        let spec = serde_json::json!({"metadata":{"gpu_families":["spark"]}});
        assert!(!matches_hardware(&spec, None));
        assert!(matches_hardware(
            &serde_json::json!({"image":"pinned"}),
            None
        ));
        let mut snapshot: crate::dashboard_fleet_host::FleetHostSnapshot = serde_json::from_value(serde_json::json!({
            "schema_version":1,"sampled_at":"2026-10-04T00:00:00Z","hostname":"host","architecture":"arm64","uptime_seconds":1,"cpu":null,"memory":null,"disk":null,"network":[],"containers":[],"errors":[],
            "gpus":[{"index":0,"uuid":"gpu","name":"NVIDIA GB10","unified_memory":true,"memory_total_mib":null,"memory_used_mib":null,"utilization_percent":0,"temperature_celsius":40,"power_watts":null}]
        })).unwrap();
        assert!(matches_hardware(&spec, Some(&snapshot)));
        snapshot.gpus[0].name = "NVIDIA RTX PRO 6000 Blackwell Server Edition".into();
        snapshot.gpus[0].unified_memory = false;
        assert!(!matches_hardware(&spec, Some(&snapshot)));
        assert!(matches_hardware(
            &serde_json::json!({"metadata":{"gpu_families":["rtx-pro-6000-blackwell"]}}),
            Some(&snapshot)
        ));
    }
    #[test]
    fn catalog_annotations_round_trip_and_never_reach_fleet() {
        let image = format!("example@sha256:{}", "a".repeat(64));
        let body = CreateRecipeRevision {
            description: "portable annotated recipe".into(),
            model_spec: serde_json::json!({"image":image,"gpus":"device=0","user":"1000:1000","memory_limit":"128m","metadata":{"gpu_families":["spark"],"image_sources":{image.clone():"example:stable"},"image_source_urls":{image.clone():"https://example.org/build"}}}),
        };
        let digest = validate_recipe(&body).unwrap();
        let restored: CreateRecipeRevision =
            serde_json::from_str(&serde_json::to_string(&body).unwrap()).unwrap();
        assert_eq!(restored.model_spec, body.model_spec);
        assert_eq!(validate_recipe(&restored).unwrap(), digest);
        let rendered =
            crate::dashboard_deployment_groups::render(&restored.model_spec, None).unwrap();
        assert!(rendered.get("metadata").is_none());
        assert_eq!(rendered["image"], image);
        let distributed = CreateRecipeRevision {
            description: "distributed annotations".into(),
            model_spec: serde_json::json!({"distributed":[body.model_spec.clone(),body.model_spec.clone()],"metadata":body.model_spec["metadata"].clone()}),
        };
        let restored: CreateRecipeRevision =
            serde_json::from_str(&serde_json::to_string(&distributed).unwrap()).unwrap();
        assert_eq!(restored.model_spec, distributed.model_spec);
        assert_eq!(
            validate_recipe(&restored).unwrap(),
            validate_recipe(&distributed).unwrap()
        );
        let placement = crate::dashboard_deployment_groups::Placement {
            group_id: "pair".into(),
            rank: 1,
            addresses: vec!["host1".into(), "host2".into()],
            gpus: None,
            gpu_uuids: None,
        };
        assert_eq!(
            crate::dashboard_deployment_groups::render(&restored.model_spec, Some(&placement))
                .unwrap(),
            rendered
        );
        let mut invalid = body.clone();
        invalid.model_spec["metadata"]["image_source_urls"][&image] =
            Value::String("https://user:password@example.org/build".into());
        assert!(validate_recipe(&invalid).is_err());
        invalid.model_spec["metadata"] =
            serde_json::json!({"gpu_families":["spark"],"unexpected":"ignored"});
        assert!(validate_recipe(&invalid).is_err());
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
        let settings = ManagedCommand {
            settings: Some(
                serde_json::json!({"revision":7,"configuration":{"policy":{},"metrics":{"cpu":"100ms"}}}),
            ),
            benchmark: None,
            deployment_id: None,
            desired: None,
        };
        assert_eq!(
            serde_json::from_value::<ManagedCommand>(serde_json::to_value(&settings).unwrap())
                .unwrap(),
            settings
        );
        let command = ManagedCommand {
            settings: None,
            benchmark: None,
            deployment_id: Some("fixture".into()),
            desired: Some(serde_json::json!({"generation":1,"model":{"image":"fixture"}})),
        };
        assert_eq!(
            serde_json::from_value::<ManagedCommand>(serde_json::to_value(&command).unwrap())
                .unwrap(),
            command
        );
        let reply = ManagedReply {
            error_status: None,
            request_id: uuid::Uuid::new_v4(),
            result: Some(serde_json::json!({"status":{"phase":"ready"}})),
            error: None,
        };
        assert_eq!(
            serde_json::from_value::<ManagedReply>(serde_json::to_value(&reply).unwrap()).unwrap(),
            reply
        );
    }

    #[test]
    fn imported_runtime_options_and_local_image_ids_round_trip() {
        let body = CreateRecipeRevision {
            description: "Imported inference".into(),
            model_spec: serde_json::json!({"image":format!("sha256:{}", "a".repeat(64)),
                "user":"0:0", "memory_limit":"115g", "privileged":true,
                "devices":["/dev/infiniband:/dev/infiniband:rwm"],
                "environment_files":["/home/koen/compose/fleet-observer/.secrets/model.env"],
                "group_add":["44"]}),
        };
        let digest = validate_recipe(&body).unwrap();
        let restored: CreateRecipeRevision =
            serde_json::from_value(serde_json::to_value(&body).unwrap()).unwrap();
        assert_eq!(validate_recipe(&restored).unwrap(), digest);
    }

    #[test]
    fn source_catalog_is_bounded_and_round_trips() {
        let mut body = CreateRecipeRevision {
            description: "Archived Compose".into(),
            model_spec: serde_json::json!({"source_format":"compose","source_content":"services: {}\n"}),
        };
        let digest = validate_recipe(&body).unwrap();
        let restored: CreateRecipeRevision =
            serde_json::from_value(serde_json::to_value(&body).unwrap()).unwrap();
        assert_eq!(validate_recipe(&restored).unwrap(), digest);
        body.model_spec["source_content"] = Value::String("x".repeat(32769));
        assert!(validate_recipe(&body).is_err());
        body.model_spec["source_content"] = Value::String(
            "services:\n  model:\n    environment:\n      HF_TOKEN: private\n".into(),
        );
        assert!(validate_recipe(&body).is_err());
        body.model_spec["source_content"] = Value::String(
            "services:\n  model:\n    environment:\n      HF_TOKEN: ${HF_TOKEN}\n".into(),
        );
        assert!(validate_recipe(&body).is_ok());
    }
    #[tokio::test]
    async fn conversion_requires_admin_and_csrf_and_validates_the_preview() {
        use crate::dashboard_auth::{AuthSessionKind, CSRF_COOKIE, CSRF_HEADER, DashboardEnv};
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let auth = DashboardAuth::from_env(
            "127.0.0.1:0".parse().unwrap(),
            &DashboardEnv {
                token: Some("preview-test".into()),
                session_key_b64: Some(STANDARD.encode([7u8; 32])),
                allow_mutations: true,
                ..Default::default()
            },
        )
        .unwrap()
        .auth;
        let session = |is_admin| AuthSession {
            exp: u64::MAX,
            user: Some(crate::accounts::SessionUser {
                id: "operator-id".into(),
                username: "operator".into(),
                is_admin,
            }),
            kind: AuthSessionKind::User,
        };
        let request = crate::recipe_import::ConvertRecipeRequest {
            source_format: "compose".into(),
            source_content: "services: {x: {image: tagged}}".into(),
            options: Default::default(),
        };
        let source_request = crate::recipe_import::FetchRecipeSource {
            url: "https://localhost/".into(),
        };
        assert_eq!(
            fetch_recipe_source(
                Extension(auth.clone()),
                Extension(session(false)),
                HeaderMap::new(),
                Json(source_request.clone())
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fetch_recipe_source(
                Extension(auth.clone()),
                Extension(session(true)),
                HeaderMap::new(),
                Json(source_request.clone())
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            convert_recipe(
                Extension(auth.clone()),
                Extension(session(false)),
                HeaderMap::new(),
                Json(request.clone())
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            convert_recipe(
                Extension(auth.clone()),
                Extension(session(true)),
                HeaderMap::new(),
                Json(request.clone())
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
        assert_eq!(
            fetch_recipe_source(
                Extension(auth.clone()),
                Extension(session(true)),
                headers.clone(),
                Json(source_request)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        let response = convert_recipe(
            Extension(auth.clone()),
            Extension(session(true)),
            headers.clone(),
            Json(request.clone()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CACHE_CONTROL],
            "no-store"
        );
        let value: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(value["required_inputs"].as_array().unwrap().len() >= 3);
        let mut bad = request;
        bad.source_content = "services: {x: {environment: {PASSWORD: literal-secret}}}".into();
        let response = convert_recipe(
            Extension(auth),
            Extension(session(true)),
            headers,
            Json(bad),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let text = String::from_utf8(
            axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!text.contains("literal-secret"));
    }
}

/// Change catalog ownership of existing runtime specifications without restarting
/// containers. Equivalence is checked after rank/address rendering, atomically.
#[derive(utoipa::ToSchema, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogAdoption {
    pub deployments: BTreeMap<String, AdoptRevision>,
    #[serde(default)]
    pub archive_recipes: Vec<String>,
}
#[derive(utoipa::ToSchema, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptRevision {
    pub expected_generation: u64,
    pub recipe_id: String,
    pub revision_digest: String,
    pub placement: Option<crate::dashboard_deployment_groups::Placement>,
}
fn adopt_catalog(state: &mut ManagementState, body: &CatalogAdoption) -> Result<(), String> {
    if body.deployments.len() > 64 || body.archive_recipes.len() > 1024 {
        return Err("catalog adoption exceeds bounds".into());
    }
    let mut changes = Vec::new();
    for (id, b) in &body.deployments {
        let d = state.deployments.get(id).ok_or("deployment not found")?;
        if d.generation != b.expected_generation {
            return Err("generation changed".into());
        }
        let old = state
            .recipes
            .get(&d.recipe_id)
            .and_then(|r| r.iter().find(|r| r.digest == d.revision_digest))
            .ok_or("old recipe missing")?;
        let new = state
            .recipes
            .get(&b.recipe_id)
            .and_then(|r| r.iter().find(|r| r.digest == b.revision_digest))
            .ok_or("new recipe missing")?;
        let render = crate::dashboard_deployment_groups::render;
        if render(&old.model_spec, d.placement.as_ref())?
            != render(&new.model_spec, b.placement.as_ref())?
        {
            return Err("runtime specifications differ; unload before changing execution".into());
        }
        if let Some(p) = &b.placement
            && (!valid_id(&p.group_id) || p.group_id.len() > 32)
        {
            return Err("invalid placement group".into());
        }
        let mut d = d.clone();
        d.generation = d.generation.checked_add(1).ok_or("generation exhausted")?;
        d.observed_at_ms = None;
        d.observed = None;
        d.error = None;
        d.recipe_id = b.recipe_id.clone();
        d.revision_digest = b.revision_digest.clone();
        d.placement = b.placement.clone();
        changes.push((id.clone(), d));
    }
    let mut next = state.clone();
    for (id, d) in changes {
        next.deployments.insert(id, d);
    }
    for id in &body.archive_recipes {
        if !next.recipes.contains_key(id) || next.deployments.values().any(|d| d.recipe_id == *id) {
            return Err("cannot archive a referenced or missing recipe".into());
        }
        if !next.archived_recipes.contains(id) {
            next.archived_recipes.push(id.clone());
        }
    }
    for (id, d) in &next.deployments {
        if let Some(p) = &d.placement {
            let peers = next
                .deployments
                .values()
                .filter(|x| {
                    x.placement
                        .as_ref()
                        .is_some_and(|v| v.group_id == p.group_id)
                })
                .collect::<Vec<_>>();
            if peers.len() != p.addresses.len()
                || peers.iter().any(|x| {
                    x.recipe_id != d.recipe_id
                        || x.revision_digest != d.revision_digest
                        || x.placement.as_ref().unwrap().addresses != p.addresses
                })
                || peers
                    .iter()
                    .filter(|x| x.placement.as_ref().unwrap().rank == p.rank)
                    .count()
                    != 1
            {
                return Err(format!("incomplete or inconsistent placement for {id}"));
            }
        }
    }
    *state = next;
    Ok(())
}
#[utoipa::path(post,path="/dashboard/api/management/adopt",tag="dashboard",request_body=CatalogAdoption,responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn adopt(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<CatalogAdoption>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    }
    let Some(store) = gateway.persistence_store() else {
        return json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({"error":"SQL required"}),
        );
    };
    for _ in 0..4 {
        let Ok((raw, mut state)) = read(store.as_ref()).await else {
            return json_no_store(
                StatusCode::SERVICE_UNAVAILABLE,
                &serde_json::json!({"error":"catalog unavailable"}),
            );
        };
        if let Err(error) = adopt_catalog(&mut state, &body) {
            return json_no_store(StatusCode::CONFLICT, &serde_json::json!({"error":error}));
        }
        match commit(store.as_ref(), raw.as_deref(), &state).await {
            Ok(true) => return json_no_store(StatusCode::OK, &state),
            Ok(false) => continue,
            Err(_) => break,
        }
    }
    json_no_store(
        StatusCode::CONFLICT,
        &serde_json::json!({"error":"catalog changed; retry"}),
    )
}

/// Retire an observed-unloaded single-host placement while retaining recipe evidence.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveDeployment {
    pub expected_generation: u64,
}
fn remove_unloaded(
    state: &mut ManagementState,
    id: &str,
    generation: u64,
) -> Result<(), &'static str> {
    let d = state.deployments.get(id).ok_or("deployment not found")?;
    if d.generation != generation {
        return Err("generation changed");
    }
    if d.desired_state != "unloaded"
        || d.observed
            .as_ref()
            .and_then(|v| v.pointer("/status/phase"))
            .and_then(Value::as_str)
            != Some("unloaded")
        || d.observed
            .as_ref()
            .and_then(|v| v.pointer("/desired/generation"))
            .and_then(Value::as_u64)
            != Some(generation)
    {
        return Err("wait for the current generation to be observed unloaded");
    }
    if d.placement.as_ref().is_some_and(|p| {
        p.addresses.len() != 1
            || state
                .deployments
                .values()
                .filter(|x| {
                    x.placement
                        .as_ref()
                        .is_some_and(|q| q.group_id == p.group_id)
                })
                .count()
                != 1
    }) {
        return Err("removing distributed placements is unsupported");
    }
    state.deployments.remove(id);
    Ok(())
}
#[utoipa::path(delete,path="/dashboard/api/management/deployments/{id}",params(("id"=String,Path),("expected_generation"=u64,Query)),responses((status=200),(status=403),(status=409),(status=503)),tag="dashboard")]
pub async fn remove_deployment(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    Path(id): Path<String>,
    Query(body): Query<RemoveDeployment>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return response;
    }
    let Some(store) = gateway.persistence_store() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SQL persistence is required",
        );
    };
    for _ in 0..4 {
        let Ok((raw, mut state)) = read(store.as_ref()).await else {
            return error(StatusCode::SERVICE_UNAVAILABLE, "catalog unavailable");
        };
        if let Err(message) = remove_unloaded(&mut state, &id, body.expected_generation) {
            return error(StatusCode::CONFLICT, message);
        }
        match commit(store.as_ref(), raw.as_deref(), &state).await {
            Ok(true) => return json_no_store(StatusCode::OK, &serde_json::json!({"removed":id})),
            Ok(false) => continue,
            Err(_) => {
                return error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "could not retire deployment",
                );
            }
        }
    }
    error(StatusCode::CONFLICT, "catalog changed; retry")
}
#[cfg(test)]
mod retirement_tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> ManagementState {
        let mut state = ManagementState::default();
        state.recipes.insert("recipe".into(), vec![]);
        state.deployments.insert(
            "diagnostic".into(),
            DesiredDeployment {
                generation: 4,
                placement: None,
                endpoint_id: "host".into(),
                recipe_id: "recipe".into(),
                revision_digest: "pinned".into(),
                desired_state: "unloaded".into(),
                updated_at_ms: 0,
                observed_at_ms: Some(1),
                observed: Some(json!({"desired":{"generation":4},"status":{"phase":"unloaded"}})),
                error: None,
            },
        );
        state
    }
    #[test]
    fn retirement_preserves_catalog_and_rejects_active_or_stale_observations() {
        let mut state = fixture();
        assert!(remove_unloaded(&mut state, "diagnostic", 3).is_err());
        state
            .deployments
            .get_mut("diagnostic")
            .unwrap()
            .desired_state = "ready".into();
        assert!(remove_unloaded(&mut state, "diagnostic", 4).is_err());
        state
            .deployments
            .get_mut("diagnostic")
            .unwrap()
            .desired_state = "unloaded".into();
        state.deployments.get_mut("diagnostic").unwrap().observed =
            Some(json!({"desired":{"generation":3},"status":{"phase":"unloaded"}}));
        assert!(remove_unloaded(&mut state, "diagnostic", 4).is_err());
        assert_eq!(state.deployments.len(), 1);
        state = fixture();
        remove_unloaded(&mut state, "diagnostic", 4).unwrap();
        assert!(state.deployments.is_empty());
        assert!(state.recipes.contains_key("recipe"));
    }
    #[test]
    fn retirement_does_not_remove_part_of_a_distributed_group_and_query_round_trips() {
        let mut state = fixture();
        state.deployments.get_mut("diagnostic").unwrap().placement =
            Some(crate::dashboard_deployment_groups::Placement {
                group_id: "group".into(),
                rank: 0,
                addresses: vec!["one".into(), "two".into()],
                gpus: None,
                gpu_uuids: None,
            });
        assert!(remove_unloaded(&mut state, "diagnostic", 4).is_err());
        assert_eq!(state.deployments.len(), 1);
        let input = json!({"expected_generation":4});
        let value: RemoveDeployment = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(value).unwrap(), input);
    }
}

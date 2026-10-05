//! Durable, revisioned startup settings. A supervisor applies staged revisions;
//! request handlers never mutate the process environment or partially rebuild a gateway.
use crate::{
    control_plane::ControlPlaneConfig,
    control_plane_store::{PersistenceStore, SqlStore},
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_mesh::authorize_mesh_admin_read,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::OnceCell;
pub const KNOWN_ENVIRONMENT: &[&str] = &[
    "BRAVE_SEARCH_API_KEY",
    "LLMCONDUIT_ADMIN_PASSWORD",
    "LLMCONDUIT_ADMIN_USERNAME",
    "LLMCONDUIT_ALLOW_INSECURE_DASHBOARD",
    "LLMCONDUIT_AUTH_BOOTSTRAP_KEY",
    "LLMCONDUIT_AUTH_MIGRATE_LEGACY_KEYS",
    "LLMCONDUIT_AUTH_PEPPER",
    "LLMCONDUIT_BIND_ADDR",
    "LLMCONDUIT_BRAVE_BASE_URL",
    "LLMCONDUIT_BRAVE_MAX_RESULTS",
    "LLMCONDUIT_CONNECT_TIMEOUT_SECS",
    "LLMCONDUIT_CONVERSATION_ID_HEADER",
    "LLMCONDUIT_DASHBOARD_ALLOW_MUTATIONS",
    "LLMCONDUIT_DASHBOARD_CAPTURE_UPSTREAM_RESPONSE",
    "LLMCONDUIT_DASHBOARD_CLIENT_HEADER",
    "LLMCONDUIT_DASHBOARD_PUBLIC_ORIGIN",
    "LLMCONDUIT_DASHBOARD_RUNTIME_DIR",
    "LLMCONDUIT_DASHBOARD_SESSION_KEY",
    "LLMCONDUIT_DASHBOARD_TOKEN",
    "LLMCONDUIT_DATABASE_URL",
    "LLMCONDUIT_DEBUG_LOG_MAX_AGE_HOURS",
    "LLMCONDUIT_FLATTEN_CONTENT",
    "LLMCONDUIT_FLEET_TOKEN",
    "LLMCONDUIT_FLEET_TOKEN_FILE",
    "LLMCONDUIT_FLEET_URL",
    "LLMCONDUIT_GITHUB_ADMIN_USERS",
    "LLMCONDUIT_GITHUB_ALLOWED_USERS",
    "LLMCONDUIT_GITHUB_CALLBACK_URL",
    "LLMCONDUIT_GITHUB_CLIENT_ID",
    "LLMCONDUIT_GITHUB_CLIENT_SECRET",
    "LLMCONDUIT_IMAGE_AGENT_ENABLED",
    "LLMCONDUIT_IMAGE_CACHE_MAX_SIZE",
    "LLMCONDUIT_IMAGE_CACHE_TTL_SECS",
    "LLMCONDUIT_MAX_REPLAY_ENTRIES",
    "LLMCONDUIT_MAX_REQUEST_BODY_BYTES",
    "LLMCONDUIT_MAX_SSE_FRAME_BYTES",
    "LLMCONDUIT_MAX_WEB_SEARCH_ROUNDS",
    "LLMCONDUIT_MIN_COMPLETION_TOKENS",
    "LLMCONDUIT_PERSISTENCE_QUEUE_CAPACITY",
    "LLMCONDUIT_PERSISTENCE_RETENTION_DAYS",
    "LLMCONDUIT_PRICE_TABLE_JSON",
    "LLMCONDUIT_REQUEST_TIMEOUT_SECS",
    "LLMCONDUIT_REQUIRE_AUTH",
    "LLMCONDUIT_STORAGE_BACKEND",
    "LLMCONDUIT_STORAGE_JSONL_DIR",
    "LLMCONDUIT_SYSTEM_PROMPT_PREFIX",
    "LLMCONDUIT_TEMPLATE_FAMILY",
    "LLMCONDUIT_TURN_CAPTURE_DIR",
    "LLMCONDUIT_UNSUPPORTED_IMAGE_POLICY",
    "LLMCONDUIT_UPSTREAM_API_KEY",
    "LLMCONDUIT_UPSTREAM_BASE_URL",
    "LLMCONDUIT_UPSTREAM_CHAT_KWARGS_JSON",
    "LLMCONDUIT_UPSTREAM_FAILURE_COOLDOWN_SECS",
    "LLMCONDUIT_UPSTREAM_MODEL",
    "LLMCONDUIT_UPSTREAM_REQUEST_LOG_PATH",
    "LLMCONDUIT_VISION_MODEL",
    "LLMCONDUIT_VISION_URL",
    "OPENAI_API_KEY",
    "RUST_LOG",
];
pub const KEY: &str = "gateway_settings_v1";
pub const SECRET: &str = "[keep existing secret]";
pub const INTERNAL_PREFIX: &str = "LLMCONDUIT_MANAGED_";
#[derive(Clone, Serialize, Deserialize)]
pub struct Specification {
    pub document: Value,
    pub environment: BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub revision: u64,
    pub applied_revision: u64,
    pub status: String,
    pub pending: bool,
    pub draft: Specification,
    pub active: Specification,
    pub error: Option<String>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub revision: u64,
    pub values: Value,
    pub environment: BTreeMap<String, Option<String>>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Apply {
    pub revision: u64,
}
static OWNER: OnceCell<Arc<SqlStore>> = OnceCell::const_new();
pub fn managed_env(name: &str) -> bool {
    ((name.starts_with("LLMCONDUIT_")
        && !name.starts_with(INTERNAL_PREFIX)
        && !name.starts_with("LLMCONDUIT_BUILD_")
        && name != "LLMCONDUIT_DASHBOARD_DIST")
        || matches!(name, "OPENAI_API_KEY" | "BRAVE_SEARCH_API_KEY" | "RUST_LOG"))
        && !name.contains("MESH_JOIN_KEY")
}
pub fn referenced_environment(document: &Value) -> std::collections::BTreeSet<String> {
    fn walk(value: &Value, names: &mut std::collections::BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if key == "api_key_env" {
                        if let Some(name) = value.as_str() {
                            names.insert(name.to_owned());
                        }
                    } else {
                        walk(value, names);
                    }
                }
            }
            Value::Array(list) => {
                for value in list {
                    walk(value, names);
                }
            }
            _ => {}
        }
    }
    let mut names = std::collections::BTreeSet::new();
    walk(document, &mut names);
    names
}
fn environment_scope<'a>(spec: &Specification, name: &'a str) -> &'a str {
    if referenced_environment(&spec.document).contains(name) {
        "providers"
    } else {
        env_scope(name)
    }
}
pub async fn connect(backend: &str, url: &str) -> Result<Arc<SqlStore>, String> {
    Ok(Arc::new(match backend {
        "sqlite" => SqlStore::connect_sqlite(url).await?,
        "postgres" => SqlStore::connect_postgres(url).await?,
        _ => return Err("Managed settings require SQL persistence".into()),
    }))
}
async fn owner() -> Result<Arc<SqlStore>, String> {
    OWNER
        .get_or_try_init(|| async {
            let url = std::env::var("LLMCONDUIT_MANAGED_STORE_URL")
                .map_err(|_| "Configuration supervisor is required")?;
            let backend = std::env::var("LLMCONDUIT_MANAGED_STORE_BACKEND")
                .map_err(|_| "Configuration supervisor is required")?;
            connect(&backend, &url).await
        })
        .await
        .cloned()
}
pub async fn read(store: &dyn PersistenceStore) -> Result<(String, Settings), String> {
    let raw = store
        .get_setting(KEY)
        .await?
        .ok_or("Managed settings have not been initialized")?;
    let settings =
        serde_json::from_str(&raw).map_err(|_| "Managed settings document is invalid")?;
    Ok((raw, settings))
}
pub async fn write(
    store: &dyn PersistenceStore,
    expected: Option<&str>,
    settings: &Settings,
) -> Result<bool, String> {
    store
        .compare_exchange_setting(
            KEY,
            expected,
            &serde_json::to_string(settings).map_err(|_| "Could not serialize settings")?,
        )
        .await
}
/// Each page owns explicit paths. Full documents/unknown fields survive untouched.
pub fn paths(scope: &str) -> Option<Vec<&'static str>> {
    Some(match scope {
        "gateway" => vec![
            "/request_timeout_secs",
            "/connect_timeout_secs",
            "/upstream_failure_cooldown_secs",
            "/max_request_body_bytes",
            "/max_sse_frame_bytes",
            "/max_web_search_rounds",
            "/flatten_content",
            "/min_completion_tokens",
            "/max_replay_entries",
            "/image_cache_max_size",
            "/image_cache_ttl_secs",
            "/unsupported_image_policy",
            "/system_prompt_prefix",
            "/upstream_chat_kwargs",
            "/price_table",
        ],
        "profiles" => vec![
            "/control_plane/operational/model_profiles",
            "/control_plane/operational/unknown_model_policy",
            "/model_profiles",
            "/model_profile_templates",
            "/model_routes",
            "/template_family",
        ],
        "providers" => vec![
            "/control_plane/operational/backends",
            "/upstreams",
            "/fallback_upstreams",
            "/upstream_base_url",
            "/upstream_api_key",
            "/upstream_model",
        ],
        "integrations" => vec![
            "/brave_base_url",
            "/brave_api_key",
            "/brave_max_results",
            "/image_agent_enabled",
            "/vision_url",
            "/vision_model",
        ],
        "authentication" => vec!["/control_plane/auth"],
        "access" => vec!["/auth"],
        "observability" => vec![
            "/control_plane/metrics",
            "/control_plane/vision_probe",
            "/control_plane/sessions",
            "/upstream_metrics_url",
            "/upstream_metrics_source",
            "/upstream_request_log_path",
            "/turn_capture_dir",
            "/debug_log_max_age_hours",
            "/control_plane/storage/keep_media",
            "/control_plane/storage/retention_days",
        ],
        "service" => vec![
            "/bind_addr",
            "/mesh",
            "/control_plane/storage/backend",
            "/control_plane/storage/url",
            "/control_plane/storage/jsonl_dir",
            "/control_plane/storage/queue_capacity",
        ],
        _ => return None,
    })
}
fn env_scope(name: &str) -> &str {
    if name.contains("GITHUB")
        || name.starts_with("LLMCONDUIT_ADMIN_")
        || name == "LLMCONDUIT_REQUIRE_AUTH"
        || name == "LLMCONDUIT_CONVERSATION_ID_HEADER"
    {
        return "authentication";
    }
    if name.contains("AUTH_PEPPER")
        || name.contains("AUTH_BOOTSTRAP")
        || name.contains("AUTH_MIGRATE")
    {
        "access"
    } else if name.contains("DASHBOARD")
        && !name.contains("CAPTURE")
        && !name.contains("CLIENT_HEADER")
    {
        "authentication"
    } else if name.contains("BRAVE")
        || name.contains("IMAGE_AGENT")
        || name.contains("VISION_URL")
        || name.contains("VISION_MODEL")
    {
        "integrations"
    } else if name.contains("STORAGE")
        || name == "LLMCONDUIT_DATABASE_URL"
        || name == "LLMCONDUIT_PERSISTENCE_QUEUE_CAPACITY"
        || name.contains("BIND_ADDR")
        || name.contains("MESH")
    {
        "service"
    } else if name.contains("CAPTURE")
        || name == "LLMCONDUIT_PERSISTENCE_RETENTION_DAYS"
        || name.contains("LOG")
        || name.contains("METRIC")
        || name.contains("CLIENT_HEADER")
    {
        "observability"
    } else if name.contains("UPSTREAM_BASE")
        || name.contains("API_KEY")
        || name == "LLMCONDUIT_UPSTREAM_MODEL"
    {
        "providers"
    } else if name == "LLMCONDUIT_TEMPLATE_FAMILY" {
        "profiles"
    } else {
        "gateway"
    }
}
fn secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    if k.ends_with("_env") {
        return false;
    }
    k.contains("password")
        || k.contains("secret")
        || k.contains("api_key")
        || k.contains("pepper")
        || (k == "token"
            || k.ends_with("_token")
            || k.ends_with("_key") && !k.ends_with("_model_key"))
        || k.contains("session_key")
        || k == "key"
        || k.ends_with("_hash")
}
fn credential_url(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        url::Url::parse(s).is_ok_and(|u| !u.username().is_empty() || u.password().is_some())
            || url::Url::parse(s).is_ok_and(|u| u.query_pairs().any(|(k, _)| secret_key(&k)))
    })
}
pub fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                if (secret_key(key) || credential_url(v)) && !v.is_null() && v != "" {
                    *v = Value::String(SECRET.into())
                } else {
                    redact(v)
                }
            }
        }
        Value::Array(v) => {
            for x in v {
                redact(x)
            }
        }
        _ => {}
    }
}
fn restore(value: &mut Value, old: &Value) -> Result<(), String> {
    if value == SECRET {
        if old.is_null() {
            return Err("A new secret needs a value".into());
        }
        *value = old.clone();
        return Ok(());
    }
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                restore(v, old.get(key).unwrap_or(&Value::Null))?
            }
        }
        Value::Array(list) => {
            for (index, v) in list.iter_mut().enumerate() {
                let reference = v.get("id").or_else(|| v.get("name"));
                let prior = if let Some(reference) = reference {
                    old.as_array().and_then(|items| {
                        items.iter().find(|item| {
                            item.get("id").or_else(|| item.get("name")) == Some(reference)
                        })
                    })
                } else {
                    old.get(index)
                };
                restore(v, prior.unwrap_or(&Value::Null))?
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn set(document: &mut Value, path: &str, value: Value) {
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    let mut object = document;
    for part in &parts[..parts.len() - 1] {
        if !object.get(part).is_some_and(Value::is_object) {
            object[*part] = json!({})
        }
        object = &mut object[*part]
    }
    object[parts[parts.len() - 1]] = value;
}
pub fn edit(settings: &mut Settings, scope: &str, body: Edit) -> Result<(), String> {
    let allowed = paths(scope).ok_or("Unknown settings page")?;
    if body.revision != settings.revision || settings.pending {
        return Err("Configuration changed or is applying; reload before editing".into());
    }
    let object = body
        .values
        .as_object()
        .ok_or("Settings must be an object")?;
    for (path, value) in object {
        if !allowed.contains(&path.as_str()) {
            return Err("Setting does not belong to this page".into());
        }
        let mut value = value.clone();
        restore(
            &mut value,
            settings
                .draft
                .document
                .pointer(path)
                .unwrap_or(&Value::Null),
        )?;
        set(&mut settings.draft.document, path, value)
    }
    for (name, value) in body.environment {
        if !(managed_env(&name) || referenced_environment(&settings.draft.document).contains(&name))
            || environment_scope(&settings.draft, &name) != scope
            || (!KNOWN_ENVIRONMENT.contains(&name.as_str())
                && !settings.active.environment.contains_key(&name)
                && !referenced_environment(&settings.draft.document).contains(&name))
        {
            return Err("Environment setting does not belong to this page".into());
        }
        match value {
            Some(value) if value == SECRET => {
                if !settings.draft.environment.contains_key(&name) {
                    return Err("A new secret needs a value".into());
                }
            }
            Some(value) => {
                settings.draft.environment.insert(name, value);
            }
            None => {
                settings.draft.environment.remove(&name);
            }
        }
    }
    if scope == "access"
        && settings
            .draft
            .document
            .pointer("/auth/mode")
            .and_then(Value::as_str)
            == Some("enforce")
        && settings
            .active
            .document
            .pointer("/auth/mode")
            .and_then(Value::as_str)
            != Some("enforce")
    {
        settings
            .draft
            .environment
            .entry("LLMCONDUIT_AUTH_PEPPER".into())
            .or_insert_with(|| format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()));
        settings
            .draft
            .environment
            .entry("LLMCONDUIT_AUTH_BOOTSTRAP_KEY".into())
            .or_insert_with(|| {
                crate::authz::keys::generate_api_key(
                    &crate::authz::keys::AuthPepper::new("bootstrap").unwrap(),
                )
                .expose_once()
            });
        settings
            .draft
            .environment
            .insert("LLMCONDUIT_AUTH_MIGRATE_LEGACY_KEYS".into(), "true".into());
    }
    if let Some(existing) = settings.active.environment.get("LLMCONDUIT_AUTH_PEPPER")
        && settings.draft.environment.get("LLMCONDUIT_AUTH_PEPPER") != Some(existing)
    {
        return Err("Changing the auth pepper would invalidate existing HMAC keys. Rotate/reissue keys before replacing the policy store".into());
    }
    validate(&settings.draft)?;
    settings.revision = settings
        .revision
        .checked_add(1)
        .ok_or("Revision exhausted")?;
    settings.status = "staged".into();
    settings.error = None;
    Ok(())
}
pub fn validate(spec: &Specification) -> Result<(), String> {
    if spec.document.to_string().len() > 1024 * 1024
        || spec.environment.len() > 128
        || spec
            .environment
            .values()
            .any(|s| s.len() > 65536 || s.contains('\0'))
    {
        return Err("Configuration exceeds limits".into());
    }
    let yaml = serde_yaml::to_string(&spec.document).map_err(|_| "Invalid document")?;
    let doc = ControlPlaneConfig::from_yaml_str(&yaml)
        .map_err(|_| "Invalid configuration schema or routing. Review the changed fields")?;
    let materialized = doc
        .materialize()
        .map_err(|_| "Invalid profiles, backends or routes")?;
    crate::config::Config::from_persisted(&materialized.gateway)
        .map_err(|_| "Invalid gateway settings")?;
    Ok(())
}
fn view(settings: &Settings, scope: &str) -> Option<Value> {
    let paths = paths(scope)?;
    let mut values = json!({});
    let mut active = json!({});
    for path in paths {
        values[path] = settings
            .draft
            .document
            .pointer(path)
            .cloned()
            .unwrap_or(Value::Null);
        active[path] = settings
            .active
            .document
            .pointer(path)
            .cloned()
            .unwrap_or(Value::Null)
    }
    redact(&mut values);
    redact(&mut active);
    let environment: BTreeMap<_, _> = settings
        .draft
        .environment
        .iter()
        .filter(|(name, _)| environment_scope(&settings.draft, name) == scope)
        .map(|(k, v)| {
            (
                k.clone(),
                if secret_key(k)
                    || referenced_environment(&settings.draft.document).contains(k)
                    || credential_url(&json!(v))
                {
                    SECRET.into()
                } else {
                    v.clone()
                },
            )
        })
        .collect();
    Some(
        json!({"revision":settings.revision,"applied_revision":settings.applied_revision,"status":settings.status,"error":settings.error,"values":values,"active_values":active,"environment":environment,"impact":"Gateway restart; existing streams drain for up to 35 seconds. Failed readiness rolls back. Aliases and database records are retained.","source":"SQL managed settings","available_environment":KNOWN_ENVIRONMENT.iter().map(|s|s.to_string()).chain(referenced_environment(&settings.draft.document)).filter(|name|environment_scope(&settings.draft,name)==scope).collect::<std::collections::BTreeSet<_>>()}),
    )
}
fn error(status: StatusCode, message: &str) -> Response {
    json_no_store(status, &json!({"error":message}))
}
fn effective(gateway: &crate::engine::Gateway, scope: &str) -> Value {
    let config = gateway.config();
    let all = json!({
 "/bind_addr":config.bind_addr.to_string(),"/request_timeout_secs":config.request_timeout.as_secs(),"/connect_timeout_secs":config.connect_timeout_secs,"/max_request_body_bytes":config.max_request_body_bytes,"/max_sse_frame_bytes":config.max_sse_frame_bytes,"/min_completion_tokens":config.min_completion_tokens,"/max_replay_entries":config.max_replay_entries,"/max_web_search_rounds":config.max_web_search_rounds,"/flatten_content":config.flatten_content,"/upstream_failure_cooldown_secs":config.upstream_failure_cooldown_secs,"/image_cache_max_size":config.image_cache_max_size,"/image_cache_ttl_secs":config.image_cache_ttl_secs,"/image_agent_enabled":config.image_agent_enabled,"/brave_max_results":config.brave_max_results,"/brave_base_url":config.brave_base_url.as_str(),"/debug_log_max_age_hours":config.debug_log_max_age_hours,"/upstream_base_url":config.upstream_base_url.as_str(),"/upstream_model":config.upstream_model,"/system_prompt_prefix":config.system_prompt_prefix});
    let mut result = json!({});
    for path in paths(scope).unwrap_or_default() {
        if let Some(v) = all.get(path) {
            result[path] = v.clone()
        }
    }
    redact(&mut result);
    result
}
fn authorize_configuration(
    auth: &DashboardAuth,
    session: &AuthSession,
    headers: &HeaderMap,
) -> Option<Response> {
    if let Some(r) = authorize_mesh_admin_read(auth, session, headers) {
        return Some(r);
    }
    if !auth.verify_csrf(headers) {
        return Some(error(
            StatusCode::FORBIDDEN,
            "Missing or invalid CSRF token",
        ));
    }
    // The opt-in supervisor remains manageable when ordinary mutations are off.
    None
}
#[utoipa::path(get,path="/dashboard/api/settings/{scope}",params(("scope"=String,Path)),tag="dashboard",responses((status=200,body=Value),(status=403),(status=503)))]
pub async fn get(
    State(gateway): State<Arc<crate::engine::Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(scope): Path<String>,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    match owner().await {
        Ok(store) => match read(store.as_ref()).await {
            Ok((_, s)) => view(&s, &scope)
                .map(|mut v| {
                    v["effective_values"] = effective(&gateway, &scope);
                    json_no_store(StatusCode::OK, &v)
                })
                .unwrap_or_else(|| error(StatusCode::NOT_FOUND, "Unknown settings page")),
            Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Configuration unavailable"),
        },
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Configuration supervisor is required",
        ),
    }
}
#[utoipa::path(post,path="/dashboard/api/settings/{scope}",params(("scope"=String,Path)),request_body=Edit,tag="dashboard",responses((status=200,body=Value),(status=400),(status=403),(status=409),(status=503)))]
pub async fn stage(
    State(gateway): State<Arc<crate::engine::Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(scope): Path<String>,
    Json(body): Json<Edit>,
) -> Response {
    if let Some(r) = authorize_configuration(&auth, &session, &headers) {
        return r;
    }
    let Ok(store) = owner().await else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Configuration supervisor is required",
        );
    };
    let Ok((raw, mut settings)) = read(store.as_ref()).await else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Configuration unavailable");
    };
    if body.revision != settings.revision || settings.pending {
        return error(
            StatusCode::CONFLICT,
            "Configuration changed or is applying; reload",
        );
    }
    // SQL aliases own current references; a bootstrap alias must not block a
    // legitimate profile retirement after its live mapping was removed.
    if let Some(data) = gateway.persistence_store()
        && let Ok(Some(raw_aliases)) = data.get_setting(crate::managed_aliases::STORE_KEY).await
        && let Ok(aliases) = serde_json::from_str::<Value>(&raw_aliases)
        && let Some(rows) = aliases.get("aliases").filter(|v| v.is_array())
    {
        set(
            &mut settings.draft.document,
            "/control_plane/operational/aliases",
            rows.clone(),
        );
    }
    if let Err(message) = edit(&mut settings, &scope, body) {
        return error(StatusCode::BAD_REQUEST, &message);
    }
    let backend = std::env::var("LLMCONDUIT_MANAGED_STORE_BACKEND").unwrap_or_default();
    let url = std::env::var("LLMCONDUIT_MANAGED_STORE_URL").unwrap_or_default();
    let dir = std::env::var("LLMCONDUIT_MANAGED_STATE_DIR")
        .unwrap_or_else(|_| "/data/managed-settings".into());
    if let Err(message) = crate::configuration_supervisor::preflight(
        &settings.draft,
        std::path::Path::new(&dir),
        &backend,
        &url,
    )
    .await
    {
        return error(StatusCode::BAD_REQUEST, &message);
    }
    match write(store.as_ref(), Some(&raw), &settings).await {
        Ok(true) => json_no_store(StatusCode::OK, &view(&settings, &scope)),
        Ok(false) => error(StatusCode::CONFLICT, "Configuration changed; reload"),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Could not persist configuration",
        ),
    }
}
#[utoipa::path(post,path="/dashboard/api/settings/apply",request_body=Apply,tag="dashboard",responses((status=202,body=Value),(status=403),(status=409),(status=503)))]
pub async fn apply(
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<Apply>,
) -> Response {
    if let Some(r) = authorize_configuration(&auth, &session, &headers) {
        return r;
    }
    let Ok(store) = owner().await else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Configuration supervisor is required",
        );
    };
    let Ok((raw, mut settings)) = read(store.as_ref()).await else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Configuration unavailable");
    };
    if settings.revision != body.revision || settings.pending {
        return error(
            StatusCode::CONFLICT,
            "Configuration changed or is applying; reload",
        );
    };
    settings.pending = true;
    settings.status = "applying".into();
    match write(store.as_ref(), Some(&raw), &settings).await {
        Ok(true) => json_no_store(
            StatusCode::ACCEPTED,
            &json!({"revision":settings.revision,"status":"applying"}),
        ),
        Ok(false) => error(StatusCode::CONFLICT, "Configuration changed; reload"),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Could not schedule apply"),
    }
}
#[utoipa::path(get,path="/internal/configuration-ready",tag="internal",responses((status=200,body=Value),(status=404),(status=503)))]
pub async fn ready(
    State(gateway): State<Arc<crate::engine::Gateway>>,
    headers: HeaderMap,
) -> Response {
    let expected = std::env::var("LLMCONDUIT_MANAGED_READY_TOKEN").ok();
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    use subtle::ConstantTimeEq;
    if !expected
        .as_deref()
        .zip(provided)
        .is_some_and(|(e, p)| e.as_bytes().ct_eq(p.as_bytes()).into())
    {
        return error(StatusCode::NOT_FOUND, "Not found");
    };
    if std::env::var("LLMCONDUIT_MANAGED_REQUIRE_DASHBOARD").is_ok_and(|v| v == "true")
        && gateway.dashboard_auth().is_none()
    {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dashboard configuration failed readiness",
        );
    }
    let mut hosts = Vec::new();
    if let Some(admin) = gateway.mesh_admin() {
        if admin.initialize().await.is_err() {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Mesh state failed readiness",
            );
        }
        if let Ok(nodes) = admin.store().list_nodes().await {
            for node in nodes {
                if node.enabled
                    && node
                        .endpoint_id
                        .parse()
                        .ok()
                        .is_some_and(|id| admin.registry().host_snapshot(id).is_some())
                {
                    hosts.push(node.endpoint_id)
                }
            }
        }
    }
    json_no_store(
        StatusCode::OK,
        &json!({"revision":std::env::var("LLMCONDUIT_MANAGED_REVISION").ok(),"connected_hosts":hosts}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_and_restores_reordered_secrets() {
        let old = json!({"backends":[{"id":"a","api_key":"one"},{"id":"b","api_key":"two"}],"url":"postgres://user:password@localhost/db"});
        let mut redacted = old.clone();
        redact(&mut redacted);
        assert!(!redacted.to_string().contains("password@"));
        redacted["backends"].as_array_mut().unwrap().reverse();
        restore(&mut redacted, &old).unwrap();
        assert_eq!(redacted["backends"][0]["api_key"], "two");
        assert_eq!(redacted["url"], old["url"])
    }
    #[test]
    fn unknown_fields_and_explicit_clear_survive() {
        let spec = Specification {
            document: json!({"future":{"x":1},"upstream_base_url":"http://localhost:8080/v1","upstream_api_key":"secret"}),
            environment: BTreeMap::new(),
        };
        let mut s = Settings {
            revision: 0,
            applied_revision: 0,
            status: "applied".into(),
            pending: false,
            draft: spec.clone(),
            active: spec,
            error: None,
        };
        edit(
            &mut s,
            "providers",
            Edit {
                revision: 0,
                values: json!({"/upstream_api_key":""}),
                environment: BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(s.draft.document["future"]["x"], 1);
        assert_eq!(s.draft.document["upstream_api_key"], "");
        assert!(
            edit(
                &mut s,
                "gateway",
                Edit {
                    revision: 0,
                    values: json!({}),
                    environment: BTreeMap::new()
                }
            )
            .is_err()
        )
    }
    #[test]
    fn secret_references_and_nonsecret_limits_are_distinct() {
        let mut value = json!({"api_key_env":"EXTERNAL_CREDENTIAL","max_tokens":12,"connection":"postgres://localhost/db?sslpassword=private","url":"https://example.com/?api_key=private"});
        redact(&mut value);
        assert_eq!(value["api_key_env"], "EXTERNAL_CREDENTIAL");
        assert_eq!(value["max_tokens"], 12);
        assert_eq!(value["connection"], SECRET);
        assert_eq!(value["url"], SECRET);
        assert_eq!(
            env_scope("LLMCONDUIT_GITHUB_CLIENT_SECRET"),
            "authentication"
        );
    }

    #[test]
    fn wire_edit_roundtrip() {
        let value = json!({"revision":7,"values":{"/request_timeout_secs":120},"environment":{"LLMCONDUIT_REQUEST_TIMEOUT_SECS":null}});
        let edit: Edit = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(edit).unwrap(), value)
    }
}

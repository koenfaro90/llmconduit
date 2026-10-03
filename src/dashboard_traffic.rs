//! Authenticated policy editing and body-free live admission state.
use crate::{
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_mesh::{authorize_mesh_admin_read, authorize_mesh_mutation},
    engine::Gateway,
    traffic::{Identity, STORE_KEY, TrafficConfig, TrafficRequest},
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{sync::Arc, time::Duration};
#[derive(utoipa::ToSchema, Default, Clone, Serialize, Deserialize)]
pub struct StoredPolicy {
    pub revision: u64,
    pub config: TrafficConfig,
}
#[derive(utoipa::ToSchema, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavePolicy {
    pub revision: u64,
    pub config: TrafficConfig,
}
#[derive(utoipa::ToSchema, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    #[serde(default)]
    pub identity: Option<Identity>,
    #[serde(default)]
    pub key_id: Option<String>,
    pub model: String,
    pub backend: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    #[schema(value_type=Option<String>,format=DateTime)]
    pub at: Option<chrono::DateTime<chrono::Utc>>,
}
fn error(status: StatusCode, message: &str) -> Response {
    json_no_store(status, &json!({"error":message}))
}
#[utoipa::path(get,path="/dashboard/api/traffic/layout",tag="dashboard",responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn layout(State(gateway): State<Arc<Gateway>>) -> Response {
    json_no_store(
        StatusCode::OK,
        &json!({"mobile_compact":gateway.traffic().config().1.mobile_compact}),
    )
}
#[utoipa::path(post,path="/dashboard/api/traffic/requests/{id}/cancel",tag="dashboard",params(("id"=String,Path)),responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn cancel(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    }
    if gateway.traffic().cancel_queued(&id) {
        json_no_store(StatusCode::OK, &json!({"cancelled":true}))
    } else {
        error(StatusCode::NOT_FOUND, "request is no longer queued")
    }
}
#[utoipa::path(get,path="/dashboard/api/traffic/backends",tag="dashboard",responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn backends(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    match tokio::time::timeout(
        Duration::from_secs(5),
        gateway.upstream_client().provider_inventory(),
    )
    .await
    {
        Ok(Ok(entries)) => json_no_store(StatusCode::OK, &entries),
        _ => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend inventory unavailable",
        ),
    }
}
#[utoipa::path(get,path="/dashboard/api/traffic/engines",tag="dashboard",responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn engines(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    let Some(store) = gateway.persistence_store() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SQL engine observations unavailable",
        );
    };
    match store
        .backend_metrics_history_limited(chrono::Utc::now().timestamp_millis() - 120_000, 128)
        .await
    {
        Ok(samples) => {
            let mut latest = std::collections::BTreeMap::new();
            for sample in samples {
                if let Ok(value) = serde_json::from_str::<
                    crate::upstream_metrics::UpstreamMetricsSample,
                >(&sample.data)
                    && latest.get(&value.backend).is_none_or(
                        |old: &crate::upstream_metrics::UpstreamMetricsSample| {
                            old.scraped_at_ms < value.scraped_at_ms
                        },
                    )
                {
                    latest.insert(value.backend.clone(), value);
                }
            }
            json_no_store(StatusCode::OK, &latest.into_values().collect::<Vec<_>>())
        }
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "engine observations unavailable",
        ),
    }
}
#[utoipa::path(get,path="/dashboard/api/traffic",tag="dashboard",responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn get(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    let (revision, config) = gateway.traffic().config();
    json_no_store(
        StatusCode::OK,
        &json!({"revision":revision,"config":config,"runtime":gateway.traffic().snapshot(),"scope":"single_controller"}),
    )
}
#[utoipa::path(post,path="/dashboard/api/traffic",tag="dashboard",request_body=SavePolicy,responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn save(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<SavePolicy>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    }
    if let Err(e) = body.config.validate() {
        return error(StatusCode::BAD_REQUEST, &e);
    }
    let Some(store) = gateway.persistence_store() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SQL persistence is required for traffic policy",
        );
    };
    let raw = match store.get_setting(STORE_KEY).await {
        Ok(v) => v,
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "traffic storage unavailable",
            );
        }
    };
    let old: StoredPolicy = match raw.as_deref().map(serde_json::from_str).transpose() {
        Ok(v) => v.unwrap_or_default(),
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "traffic policy is invalid"),
    };
    if old.revision != body.revision {
        return error(StatusCode::CONFLICT, "policy changed; reload before saving");
    }
    let next = StoredPolicy {
        revision: old.revision + 1,
        config: body.config,
    };
    let encoded = serde_json::to_string(&next).unwrap();
    match store
        .compare_exchange_setting(STORE_KEY, raw.as_deref(), &encoded)
        .await
    {
        Ok(true) => {
            gateway
                .traffic()
                .install(next.config.clone(), next.revision)
                .unwrap();
            json_no_store(StatusCode::OK, &next)
        }
        Ok(false) => error(StatusCode::CONFLICT, "policy changed; reload before saving"),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "traffic storage unavailable",
        ),
    }
}
#[utoipa::path(post,path="/dashboard/api/traffic/preview",tag="dashboard",request_body=Preview,responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn preview(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<Preview>,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    let identity = if let Some(key) = body.key_id {
        match gateway.authz().traffic_identity_for_key(&key).or_else(|| {
            gateway
                .client_auth()
                .identity_for_key(&key)
                .map(|id| Identity {
                    key: id.key_id.clone(),
                    user: id.owner_id.unwrap_or(id.key_id),
                    groups: vec![],
                })
        }) {
            Some(identity) => identity,
            None => return error(StatusCode::NOT_FOUND, "key unavailable"),
        }
    } else {
        body.identity.unwrap_or_default()
    };
    let request = TrafficRequest::new(identity, body.model, None, 0);
    json_no_store(
        StatusCode::OK,
        &gateway.traffic().preview(
            &request,
            &body.backend,
            &body.provider,
            body.at.unwrap_or_else(chrono::Utc::now),
        ),
    )
}
pub fn start(gateway: Arc<Gateway>) {
    let Some(store) = gateway.persistence_store() else {
        return;
    };
    let traffic = gateway.traffic();
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let jobs_gateway = gateway.clone();
        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                crate::media_api::reconcile_traffic_jobs(&jobs_gateway).await;
            }
        });
        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                match store.get_setting(STORE_KEY).await {
                    Ok(raw) => {
                        let policy: Result<StoredPolicy, _> = raw
                            .as_deref()
                            .map(serde_json::from_str)
                            .transpose()
                            .map(Option::unwrap_or_default);
                        match policy {
                            Ok(policy) => {
                                let (revision, _) = traffic.config();
                                let ready = traffic.snapshot().ready;
                                if !ready || revision != policy.revision {
                                    if !ready
                                        && let Err(error) =
                                            crate::media_api::restore_traffic_jobs(&gateway).await
                                    {
                                        tracing::error!(%error,"video capacity restoration failed");
                                        continue;
                                    }
                                    if let Err(error) =
                                        traffic.install(policy.config, policy.revision)
                                    {
                                        tracing::error!(%error,"invalid traffic policy");
                                    }
                                }
                            }
                            Err(error) => tracing::error!(%error,"invalid traffic policy"),
                        }
                    }
                    Err(error) => tracing::warn!(%error,"traffic policy refresh unavailable"),
                }
            }
        });
    }
}

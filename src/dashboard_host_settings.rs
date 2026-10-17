//! Authenticated host configuration crosses the existing pinned mesh command channel.
use crate::{
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
use serde_json::{Value, json};
use std::sync::Arc;
fn error(status: StatusCode, message: &str) -> Response {
    json_no_store(status, &json!({"error":message}))
}
async fn forward(gateway: &Gateway, id: &str, settings: Value) -> Response {
    let Some(admin) = gateway.mesh_admin() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Mesh unavailable");
    };
    let Ok(endpoint) = id.parse::<iroh::EndpointId>() else {
        return error(StatusCode::BAD_REQUEST, "Invalid host");
    };
    if !admin.store().is_node_authorized(id).await.unwrap_or(false) {
        return error(StatusCode::FORBIDDEN, "Host is not authorized");
    }
    match admin
        .registry()
        .manage_fleet(
            endpoint,
            ManagedCommand {
                settings: Some(settings),
                engine_metrics: None,
                preparation: None,
                benchmark: None,
                deployment_id: None,
                desired: None,
            },
        )
        .await
    {
        Ok(reply) => {
            if let Some(result) = reply.result {
                if !result.get("revision").is_some_and(Value::is_u64)
                    || !result.get("configuration").is_some_and(Value::is_object)
                    || !result.get("enabled").is_some_and(Value::is_boolean)
                {
                    return error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Update the host agent to support settings",
                    );
                }
                if let Some(store) = gateway.persistence_store()
                    && store
                        .set_setting(&format!("host_settings:{id}"), &result.to_string())
                        .await
                        .is_err()
                {
                    return error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Host applied settings; central observation could not be persisted. Reload to reconcile",
                    );
                }
                json_no_store(StatusCode::OK, &result)
            } else {
                error(
                    reply
                        .error_status
                        .and_then(|s| StatusCode::from_u16(s).ok())
                        .unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                    "Fleet rejected settings; check host capacity, allowlists and configuration revision",
                )
            }
        }
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "Host is unavailable"),
    }
}
#[utoipa::path(get,path="/dashboard/api/hosts/{id}/settings",params(("id"=String,Path)),tag="dashboard",responses((status=200,body=Value),(status=403),(status=503)))]
pub async fn get(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    forward(&gateway, &id, json!({})).await
}
#[utoipa::path(put,path="/dashboard/api/hosts/{id}/settings",params(("id"=String,Path)),request_body=Value,tag="dashboard",responses((status=200,body=Value),(status=400),(status=403),(status=409),(status=503)))]
pub async fn save(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    }
    if body.to_string().len() > 65536
        || !body.get("revision").is_some_and(Value::is_u64)
        || !body.get("configuration").is_some_and(Value::is_object)
    {
        return error(StatusCode::BAD_REQUEST, "Invalid host configuration");
    }
    forward(&gateway, &id, body).await
}

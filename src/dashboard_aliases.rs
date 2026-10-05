//! Administrator-only alias editing, with SQL revisions and the existing CSRF guard.
use crate::{
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_mesh::{authorize_mesh_admin_read, authorize_mesh_mutation},
    engine::Gateway,
    managed_aliases::SaveAliases,
};
use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::Arc;
fn unavailable() -> Response {
    json_no_store(
        StatusCode::SERVICE_UNAVAILABLE,
        &serde_json::json!({"error":"Alias management unavailable"}),
    )
}
#[utoipa::path(get,path="/dashboard/api/aliases",tag="dashboard",responses((status=200,body=serde_json::Value),(status=403,description="Administrator required")))]
pub async fn get(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    let Some(registry) = gateway.alias_registry() else {
        return unavailable();
    };
    match registry.view().await {
        Ok(view) => json_no_store(StatusCode::OK, &view),
        Err(e) => e.into_response(),
    }
}
#[utoipa::path(post,path="/dashboard/api/aliases",tag="dashboard",request_body=SaveAliases,responses((status=200,body=serde_json::Value),(status=403,description="Administrator and CSRF required"),(status=409,description="Revision conflict")))]
pub async fn save(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<SaveAliases>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    }
    let Some(registry) = gateway.alias_registry() else {
        return unavailable();
    };
    match registry.save(body).await {
        Ok(view) => json_no_store(StatusCode::OK, &view),
        Err(e) => e.into_response(),
    }
}

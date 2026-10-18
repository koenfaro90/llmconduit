//! On-demand read-only Fleet artifact inventory over the authenticated mesh.
use crate::{
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_management::ManagedCommand,
    dashboard_mesh::authorize_mesh_admin_read,
    engine::Gateway,
};
use axum::{
    Extension,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use std::sync::Arc;
fn error(status: StatusCode, message: &str) -> Response {
    json_no_store(status, &json!({"error":message}))
}
#[utoipa::path(get,path="/dashboard/api/hosts/{id}/artifacts",params(("id"=String,Path)),tag="dashboard",responses((status=200,body=Value),(status=403),(status=503)))]
pub async fn get(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(response) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return response;
    }
    let Some(admin) = gateway.mesh_admin() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Mesh unavailable");
    };
    let Ok(endpoint) = id.parse::<iroh::EndpointId>() else {
        return error(StatusCode::BAD_REQUEST, "Invalid host");
    };
    if !admin.store().is_node_authorized(&id).await.unwrap_or(false) {
        return error(StatusCode::FORBIDDEN, "Host is not authorized");
    }
    match admin
        .registry()
        .manage_fleet(
            endpoint,
            ManagedCommand {
                inspect_artifacts: true,
                preparation: None,
                settings: None,
                benchmark: None,
                engine_metrics: None,
                deployment_id: None,
                desired: None,
            },
        )
        .await
    {
        Ok(reply) => match reply.result {
            Some(report) if valid_report(&report) => json_no_store(StatusCode::OK, &report),
            _ => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Artifact inspection unavailable; update this host's Fleet agent and mesh worker",
            ),
        },
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Host artifact inspection unavailable",
        ),
    }
}
fn valid_report(value: &Value) -> bool {
    value["sampled_at"]
        .as_str()
        .is_some_and(|v| chrono::DateTime::parse_from_rfc3339(v).is_ok())
        && value["images"].as_array().is_some_and(|v| v.len() <= 128)
        && value["snapshots"]
            .as_array()
            .is_some_and(|v| v.len() <= 128)
        && value["presence_only"] == true
        && value["cache_configured"].is_boolean()
        && value["truncated"].is_boolean()
        && value["errors"].as_array().is_some_and(|v| {
            v.len() <= 8 && v.iter().all(|s| s.as_str().is_some_and(|s| s.len() <= 256))
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspection_command_round_trips_and_old_commands_remain_unchanged() {
        let raw = json!({"inspect_artifacts":true,"deployment_id":null,"desired":null});
        let command: ManagedCommand = serde_json::from_value(raw.clone()).unwrap();
        assert!(command.inspect_artifacts);
        assert_eq!(serde_json::to_value(command).unwrap(), raw);
        let old = json!({"deployment_id":null,"desired":null});
        let command: ManagedCommand = serde_json::from_value(old.clone()).unwrap();
        assert!(!command.inspect_artifacts);
        assert_eq!(serde_json::to_value(command).unwrap(), old);
    }
    #[test]
    fn inspection_report_requires_explicit_presence_only_and_bounds() {
        let mut value = json!({"sampled_at":"2026-10-18T13:00:00Z","images":[],"snapshots":[],"presence_only":true,"cache_configured":true,"truncated":false,"errors":[]});
        assert!(valid_report(&value));
        value["presence_only"] = json!(false);
        assert!(!valid_report(&value));
        value["presence_only"] = json!(true);
        value["snapshots"] = json!(vec![json!({}); 129]);
        assert!(!valid_report(&value));
    }
}

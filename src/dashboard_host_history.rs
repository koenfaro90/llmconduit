//! Durable Fleet system observations; history does not depend on an open UI.
use crate::{
    control_plane_store::MetricSample,
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_mesh::authorize_mesh_admin_read,
    engine::Gateway,
};
use axum::{
    Extension,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
#[derive(Deserialize)]
pub struct HistoryQuery {
    pub since_ms: i64,
    pub until_ms: Option<i64>,
}
#[utoipa::path(get,path="/dashboard/api/hosts/{id}/history",tag="dashboard",params(("id"=String,Path)),responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn history(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Response {
    if let Some(response) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return response;
    }
    let now = chrono::Utc::now().timestamp_millis();
    let until = query.until_ms.unwrap_or(now);
    if id.parse::<iroh::EndpointId>().is_err()
        || query.since_ms < 0
        || until > now + 30_000
        || until <= query.since_ms
        || until - query.since_ms > 32 * 86_400_000
    {
        return json_no_store(
            StatusCode::BAD_REQUEST,
            &json!({"error":"select an enrolled host and a range of at most 32 days"}),
        );
    }
    let Some(store) = gateway.persistence_store() else {
        return json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error":"SQL host history unavailable"}),
        );
    };
    let bucket = ((until - query.since_ms) / 900).max(15_000);
    match tokio::time::timeout(
        Duration::from_secs(5),
        store.host_metrics_history(&id, query.since_ms, until, bucket),
    )
    .await
    {
        Ok(Ok(samples)) => {
            let samples: Vec<_> = samples
                .into_iter()
                .filter_map(|s| serde_json::from_str::<serde_json::Value>(&s.data).ok())
                .collect();
            json_no_store(
                StatusCode::OK,
                &json!({"samples":samples,"bucket_ms":bucket,"since_ms":query.since_ms,"until_ms":until,"sampling":"latest_observation_per_bucket"}),
            )
        }
        _ => json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error":"host history unavailable"}),
        ),
    }
}
pub fn start(gateway: Arc<Gateway>) {
    let Some(store) = gateway.persistence_store() else {
        return;
    };
    let Some(admin) = gateway.mesh_admin() else {
        return;
    };
    crate::runtime_configuration::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(15));
        loop {
            timer.tick().await;
            let Ok(nodes) = admin.store().list_nodes().await else {
                continue;
            };
            for node in nodes.into_iter().filter(|n| n.enabled) {
                let Ok(id) = node.endpoint_id.parse() else {
                    continue;
                };
                let Some(snapshot) = admin.registry().host_snapshot(id) else {
                    continue;
                };
                let Ok(time) = chrono::DateTime::parse_from_rfc3339(&snapshot.sampled_at) else {
                    continue;
                };
                let age = chrono::Utc::now().timestamp_millis() - time.timestamp_millis();
                if !snapshot.validate() || !(-30_000..=30_000).contains(&age) {
                    continue;
                }
                let Ok(data) = serde_json::to_string(&snapshot) else {
                    continue;
                };
                if let Err(error) = store
                    .record_host_metrics(MetricSample {
                        backend: node.endpoint_id,
                        ts_ms: time.timestamp_millis(),
                        data,
                    })
                    .await
                {
                    tracing::warn!(%error,"host history storage unavailable");
                }
            }
        }
    });
}

//! Immutable revision-specific startup, inference validation and benchmark evidence.
use crate::{
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_mesh::{authorize_mesh_admin_read, authorize_mesh_mutation},
    engine::Gateway,
};
use axum::{
    Extension, Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub recipe_id: String,
    pub revision_digest: String,
    pub endpoint_id: String,
    pub kind: String,
    pub recorded_at_ms: i64,
    pub data: Value,
}
#[derive(Deserialize)]
pub struct EvidenceQuery {
    pub recipe_id: Option<String>,
    pub revision_digest: Option<String>,
}
#[utoipa::path(get,path="/dashboard/api/management/recipe-evidence",tag="dashboard",responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn list(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Query(query): Query<EvidenceQuery>,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    let Some(store) = gateway.persistence_store() else {
        return json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error":"SQL store required"}),
        );
    };
    match store
        .recipe_evidence_history(query.recipe_id.as_deref(), query.revision_digest.as_deref())
        .await
    {
        Ok(samples) => json_no_store(
            StatusCode::OK,
            &samples
                .into_iter()
                .filter_map(|s| serde_json::from_str::<Record>(&s.data).ok())
                .collect::<Vec<_>>(),
        ),
        Err(_) => json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error":"recipe evidence unavailable"}),
        ),
    }
}
pub async fn record_benchmark(gateway: &Gateway, run: &crate::dashboard_benchmarks::BenchmarkRun) {
    let Some(store) = gateway.persistence_store() else {
        return;
    };
    let Some(report) = &run.report else {
        return;
    };
    let data = json!({"benchmark_run_id":run.id,"deployment_id":run.deployment_id,"generation":run.generation,"model":run.model,"options":run.options,"image":report.get("image"),"tool":report.get("tool"),"status":report.get("status"),"cells":report.get("cells"),"prefill":report.get("prefill")});
    let record = Record {
        id: format!("benchmark:{}", run.id),
        recipe_id: run.recipe_id.clone(),
        revision_digest: run.revision_digest.clone(),
        endpoint_id: run.endpoint_id.clone(),
        kind: "benchmark".into(),
        recorded_at_ms: report
            .get("finished_at_ms")
            .and_then(Value::as_i64)
            .unwrap_or(run.created_at_ms),
        data,
    };
    if let Err(error) = store.record_recipe_evidence(record).await {
        tracing::warn!(%error,"could not persist benchmark evidence");
    }
}
#[derive(utoipa::ToSchema, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryProfile {
    pub gpu_peak_mib: std::collections::BTreeMap<u32, u64>,
    pub container_ram_peak_mib: u64,
    pub unified_peak_mib: Option<u64>,
    pub sample_interval_ms: u32,
    pub duration_seconds: u32,
    pub context_tokens: u32,
    pub concurrency: u32,
    pub image_pixels: u32,
    pub gpu_name: String,
    pub margin_percent: u32,
    pub minimum_margin_mib: u64,
}
impl MemoryProfile {
    fn valid(&self) -> bool {
        !self.gpu_peak_mib.is_empty()
            && self.gpu_peak_mib.len() <= 64
            && self
                .gpu_peak_mib
                .iter()
                .all(|(i, m)| *i < 64 && *m > 0 && *m <= 1 << 30)
            && self.container_ram_peak_mib > 0
            && self.container_ram_peak_mib <= 1 << 30
            && self.unified_peak_mib.is_none_or(|m| m > 0 && m <= 1 << 30)
            && (100..=10000).contains(&self.sample_interval_ms)
            && (1..=86400).contains(&self.duration_seconds)
            && self.context_tokens <= 10_000_000
            && (1..=10000).contains(&self.concurrency)
            && self.image_pixels <= 100_000_000
            && (1..=100).contains(&self.margin_percent)
            && self.minimum_margin_mib <= 1 << 30
            && !self.gpu_name.is_empty()
            && self.gpu_name.len() <= 128
            && !self.gpu_name.chars().any(char::is_control)
    }
}
#[derive(utoipa::ToSchema, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Smoke {
    pub deployment_id: String,
    pub passed_requests: u32,
    pub failed_requests: u32,
    pub note: String,
    #[serde(default)]
    pub memory_profile: Option<MemoryProfile>,
}
#[utoipa::path(post,path="/dashboard/api/management/recipe-evidence",tag="dashboard",request_body=Smoke,responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn record_smoke(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<Smoke>,
) -> Response {
    if let Some(r) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return r;
    }
    if body.note.len() > 256
        || body.memory_profile.as_ref().is_some_and(|p| !p.valid())
        || body.passed_requests > 10000
        || body.failed_requests > 10000
        || body.passed_requests.saturating_add(body.failed_requests) < 2
    {
        return json_no_store(
            StatusCode::BAD_REQUEST,
            &json!({"error":"report at least two bounded test requests"}),
        );
    }
    let Some(store) = gateway.persistence_store() else {
        return json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error":"SQL store required"}),
        );
    };
    let Ok((_, state)) = crate::dashboard_management::read(store.as_ref()).await else {
        return json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error":"catalog unavailable"}),
        );
    };
    let Some(d) = state.deployments.get(&body.deployment_id) else {
        return json_no_store(
            StatusCode::NOT_FOUND,
            &json!({"error":"deployment does not exist"}),
        );
    };
    if d.observed
        .as_ref()
        .and_then(|o| o.get("status"))
        .and_then(|o| o.get("phase"))
        .and_then(Value::as_str)
        != Some("ready")
        || d.observed_at_ms
            .is_none_or(|t| chrono::Utc::now().timestamp_millis() - t > 20_000)
    {
        return json_no_store(
            StatusCode::CONFLICT,
            &json!({"error":"validation requires a currently ready deployment"}),
        );
    }
    if let Some(profile) = &body.memory_profile {
        let rendered = state
            .recipes
            .get(&d.recipe_id)
            .and_then(|rs| rs.iter().find(|r| r.digest == d.revision_digest))
            .and_then(|r| {
                crate::dashboard_deployment_groups::render(&r.model_spec, d.placement.as_ref()).ok()
            });
        let devices = rendered
            .as_ref()
            .and_then(|s| s.get("gpus"))
            .and_then(Value::as_str)
            .and_then(|s| s.strip_prefix("device="))
            .and_then(|s| {
                s.split(',')
                    .map(str::parse::<u32>)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()
            });
        if body.failed_requests > 0
            || body.passed_requests < 2
            || devices.is_none_or(|g| {
                g.len() != profile.gpu_peak_mib.len()
                    || g.iter().any(|i| !profile.gpu_peak_mib.contains_key(i))
            })
        {
            return json_no_store(
                StatusCode::BAD_REQUEST,
                &json!({"error":"memory validation needs successful requests and measurements for every assigned GPU"}),
            );
        }
    }
    let record = Record {
        id: format!("smoke:{}", uuid::Uuid::new_v4()),
        recipe_id: d.recipe_id.clone(),
        revision_digest: d.revision_digest.clone(),
        endpoint_id: d.endpoint_id.clone(),
        kind: "smoke".into(),
        recorded_at_ms: chrono::Utc::now().timestamp_millis(),
        data: json!({"deployment_id":body.deployment_id,"generation":d.generation,"placement":d.placement,"passed_requests":body.passed_requests,"failed_requests":body.failed_requests,"note":body.note,"memory_profile":body.memory_profile.as_ref().map(|p|json!({"gpu_peak_mib":p.gpu_peak_mib,"container_ram_peak_mib":p.container_ram_peak_mib,"unified_peak_mib":p.unified_peak_mib,"sample_interval_ms":p.sample_interval_ms,"duration_seconds":p.duration_seconds,"context_tokens":p.context_tokens,"concurrency":p.concurrency,"image_pixels":p.image_pixels,"gpu_name":p.gpu_name,"margin_percent":p.margin_percent,"minimum_margin_mib":p.minimum_margin_mib})),"source":"operator_report"}),
    };
    match store.record_recipe_evidence(record.clone()).await {
        Ok(()) => json_no_store(StatusCode::CREATED, &record),
        Err(_) => json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error":"could not store validation"}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measured_memory_is_bounded_and_round_trips_with_smoke_evidence() {
        let value = json!({"deployment_id":"model","passed_requests":16,"failed_requests":0,"note":"Measured startup and concurrent inference","memory_profile":{"gpu_peak_mib":{"2":18000},"container_ram_peak_mib":3000,"unified_peak_mib":21000,"sample_interval_ms":250,"duration_seconds":400,"context_tokens":8192,"concurrency":16,"image_pixels":0,"gpu_name":"NVIDIA GB10","margin_percent":15,"minimum_margin_mib":2048}});
        let report: Smoke = serde_json::from_value(value.clone()).unwrap();
        assert!(report.memory_profile.as_ref().unwrap().valid());
        assert_eq!(serde_json::to_value(report).unwrap(), value);
        let mut bad = value;
        bad["memory_profile"]["gpu_peak_mib"] = json!({"64":18000});
        assert!(
            !serde_json::from_value::<Smoke>(bad)
                .unwrap()
                .memory_profile
                .unwrap()
                .valid()
        );
    }
}

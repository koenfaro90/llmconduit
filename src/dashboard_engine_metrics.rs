//! Lossless one-second engine scrapes use their own bounded mesh batches and SQL
//! table. Machine heartbeats and inference request accounting stay independent.
use crate::{
    control_plane_store::MetricSample,
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_management::ManagedCommand,
    dashboard_mesh::authorize_mesh_admin_read,
    engine::Gateway,
};
use axum::{
    Extension,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, io::Read, sync::Arc, time::Duration};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngineMetricQuery {
    pub after: u64,
    pub stream: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub family: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub help: String,
    pub labels: BTreeMap<String, String>,
    pub value: Option<f64>,
    #[serde(default)]
    pub special: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub seq: u64,
    pub deployment_id: String,
    pub generation: u64,
    pub recipe_digest: String,
    pub image: String,
    pub sampled_at_ms: i64,
    pub duration_ms: f64,
    pub metrics: Vec<Metric>,
    #[serde(default)]
    pub error: String,
}
impl Sample {
    fn valid(&self) -> bool {
        crate::dashboard_management::valid_id(&self.deployment_id)
            && self.generation > 0
            && self.recipe_digest.len() == 64
            && self.recipe_digest.bytes().all(|b| b.is_ascii_hexdigit())
            && self.image.len() <= 512
            && self.metrics.len() <= 8192
            && self.error.len() <= 128
            && self.duration_ms.is_finite()
            && (0.0..=30000.0).contains(&self.duration_ms)
            && self.metrics.iter().all(|m| {
                m.name.len() <= 256
                    && m.family.len() <= 256
                    && m.kind.len() <= 32
                    && m.help.len() <= 65536
                    && m.labels.len() <= 64
                    && m.labels
                        .iter()
                        .all(|(k, v)| k.len() <= 128 && v.len() <= 4096)
                    && m.value.is_none_or(f64::is_finite)
                    && m.special.len() <= 16
            })
    }
}
fn decode(raw: &str) -> Result<Sample, String> {
    if raw.len() > 40000 {
        return Err("compressed sample too large".into());
    }
    let bytes = STANDARD
        .decode(raw)
        .map_err(|_| "invalid engine compression")?;
    let mut output = Vec::new();
    flate2::read::GzDecoder::new(bytes.as_slice())
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut output)
        .map_err(|_| "invalid engine archive")?;
    if output.len() > 8 * 1024 * 1024 {
        return Err("engine archive exceeds bounds".into());
    }
    let sample: Sample = serde_json::from_slice(&output).map_err(|_| "invalid engine sample")?;
    if !sample.valid() {
        return Err("engine fields exceed bounds".into());
    }
    Ok(sample)
}
#[derive(Deserialize)]
struct Batch {
    stream: String,
    cursor: u64,
    oldest_cursor: u64,
    lost_samples: bool,
    interval_ms: u64,
    samples_gzip_base64: Vec<String>,
}

pub fn start(gateway: Arc<Gateway>) {
    let Some(store) = gateway.persistence_store() else {
        return;
    };
    let Some(admin) = gateway.mesh_admin() else {
        return;
    };
    crate::runtime_configuration::spawn(async move {
        let mut cursors: BTreeMap<String, EngineMetricQuery> = store
            .get_setting("fleet_engine_metric_cursors_v1")
            .await
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        cursors.retain(|id, q| {
            id.parse::<iroh::EndpointId>().is_ok()
                && q.stream.len() == 24
                && q.stream.bytes().all(|b| b.is_ascii_hexdigit())
        });
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let Ok(nodes) = admin.store().list_nodes().await else {
                continue;
            };
            let nodes: Vec<_> = nodes.into_iter().filter(|n| n.enabled).collect();
            let replies = futures::future::join_all(nodes.iter().map(|node| {
                let mut query = cursors.get(&node.endpoint_id).cloned().unwrap_or_default();
                let registry = admin.registry();
                async move {
                    let endpoint = node.endpoint_id.parse().ok()?;
                    let snapshot = registry.host_snapshot(endpoint)?;
                    if snapshot
                        .collection
                        .as_ref()?
                        .metrics
                        .get("engine_metrics")?
                        .interval_ms
                        == 0
                    {
                        return None;
                    }
                    // A host can produce several samples per second. Drain bounded
                    // batches so replay never competes with ongoing capture.
                    let mut batches = Vec::new();
                    let deadline = tokio::time::Instant::now() + Duration::from_millis(900);
                    for _ in 0..8 {
                        let reply = tokio::time::timeout_at(
                            deadline,
                            registry.manage_fleet(
                                endpoint,
                                ManagedCommand {
                                    settings: None,
                                    benchmark: None,
                                    engine_metrics: Some(query.clone()),
                                    deployment_id: None,
                                    desired: None,
                                },
                            ),
                        )
                        .await;
                        let Ok(Ok(reply)) = reply else { break };
                        let Some(result) = reply.result else { break };
                        let Ok(batch) = serde_json::from_value::<Batch>(result) else {
                            break;
                        };
                        let done = batch.samples_gzip_base64.is_empty();
                        query = EngineMetricQuery {
                            after: batch.cursor,
                            stream: batch.stream.clone(),
                        };
                        batches.push((node.endpoint_id.clone(), batch));
                        if done {
                            break;
                        }
                    }
                    Some(batches)
                }
            }))
            .await;
            let mut changed = false;
            cursors.retain(|id, _| nodes.iter().any(|n| &n.endpoint_id == id));
            for (endpoint, batch) in replies.into_iter().flatten().flatten() {
                if batch.stream.len() != 24
                    || !batch.stream.bytes().all(|b| b.is_ascii_hexdigit())
                    || batch.samples_gzip_base64.len() > 256
                    || batch.interval_ms > 60000
                {
                    continue;
                }
                if batch.lost_samples {
                    tracing::warn!(%endpoint,oldest=batch.oldest_cursor,"engine telemetry buffer overrun; history has a gap")
                }
                let mut good = true;
                let mut last = 0;
                for raw in &batch.samples_gzip_base64 {
                    let Ok(sample) = decode(raw) else {
                        good = false;
                        break;
                    };
                    let now = chrono::Utc::now().timestamp_millis();
                    if sample.seq <= last
                        || sample.seq > batch.cursor
                        || sample.sampled_at_ms < now - 600000
                        || sample.sampled_at_ms > now + 30000
                    {
                        good = false;
                        break;
                    };
                    last = sample.seq;
                    if let Err(error) = store
                        .record_engine_metrics(
                            &endpoint,
                            MetricSample {
                                backend: sample.deployment_id,
                                ts_ms: sample.sampled_at_ms,
                                data: raw.clone(),
                            },
                        )
                        .await
                    {
                        tracing::warn!(%error,"engine metrics storage unavailable");
                        good = false;
                        break;
                    }
                }
                if good {
                    changed = true;
                    cursors.insert(
                        endpoint,
                        EngineMetricQuery {
                            after: batch.cursor,
                            stream: batch.stream,
                        },
                    );
                }
            }
            if changed
                && let Ok(raw) = serde_json::to_string(&cursors)
                && raw.len() <= 256 * 1024
                && let Err(error) = store
                    .set_setting("fleet_engine_metric_cursors_v1", &raw)
                    .await
            {
                tracing::warn!(%error,"engine telemetry cursor persistence unavailable");
            }
        }
    });
}
#[derive(Deserialize)]
pub struct HistoryQuery {
    pub deployment_id: String,
    pub since_ms: i64,
    pub until_ms: Option<i64>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub names: String,
}
#[utoipa::path(get,path="/dashboard/api/hosts/{id}/engine-metrics",tag="dashboard",responses((status=200,body=Value),(status=403),(status=400),(status=503)))]
pub async fn history(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Path(endpoint): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Response {
    if let Some(r) = authorize_mesh_admin_read(&auth, &session, &headers) {
        return r;
    }
    let now = chrono::Utc::now().timestamp_millis();
    let until = query.until_ms.unwrap_or(now);
    let limit = query.limit.unwrap_or(1);
    let names: Vec<_> = query.names.split(',').filter(|v| !v.is_empty()).collect();
    if endpoint.parse::<iroh::EndpointId>().is_err()
        || !crate::dashboard_management::valid_id(&query.deployment_id)
        || limit == 0
        || limit > 3600
        || query.since_ms < 0
        || until <= query.since_ms
        || until > now + 30000
        || until - query.since_ms > 32 * 86400000
        || query.names.len() > 4096
        || names.len() > 24
        || (limit > 1 && names.is_empty())
    {
        return json_no_store(
            StatusCode::BAD_REQUEST,
            &json!({"error":"select a deployment, up to 24 metric names, and a range of at most 32 days; history pages contain at most 3600 one-second samples"}),
        );
    }
    let Some(store) = gateway.persistence_store() else {
        return json_no_store(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({"error":"SQL engine history unavailable"}),
        );
    };
    let rows = match tokio::time::timeout(
        Duration::from_secs(5),
        store.engine_metrics_history(
            &endpoint,
            &query.deployment_id,
            query.since_ms,
            until,
            limit + 1,
        ),
    )
    .await
    {
        Ok(Ok(rows)) => rows,
        _ => {
            return json_no_store(
                StatusCode::SERVICE_UNAVAILABLE,
                &json!({"error":"engine history unavailable"}),
            );
        }
    };
    let truncated = rows.len() > limit;
    let mut samples = Vec::new();
    let mut bytes = 0;
    for row in rows.into_iter().take(limit) {
        let Ok(mut sample) = decode(&row.data) else {
            continue;
        };
        if !names.is_empty() {
            sample.metrics.retain(|m| names.contains(&m.name.as_str()))
        }
        bytes += serde_json::to_vec(&sample).map_or(0, |v| v.len());
        if bytes > 8 * 1024 * 1024 {
            return json_no_store(
                StatusCode::BAD_REQUEST,
                &json!({"error":"selected series exceed the response bound; narrow the time range or request smaller pages"}),
            );
        }
        samples.push(sample);
    }
    samples.reverse();
    let next = if truncated {
        samples.first().map(|s| s.sampled_at_ms - 1)
    } else {
        None
    };
    json_no_store(
        StatusCode::OK,
        &json!({"samples":samples,"next_until_ms":next,"resolution_ms":1000,"source":"fleet_local_prometheus","includes_direct_traffic":true}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn metrics_command_round_trips_and_old_commands_omit_the_extension() {
        let command = ManagedCommand {
            settings: None,
            benchmark: None,
            engine_metrics: Some(EngineMetricQuery {
                after: 42,
                stream: "a".repeat(24),
            }),
            deployment_id: None,
            desired: None,
        };
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(
            serde_json::from_value::<ManagedCommand>(encoded).unwrap(),
            command
        );
        let old: ManagedCommand = serde_json::from_value(json!({"settings":{}})).unwrap();
        assert!(old.engine_metrics.is_none());
        assert!(
            serde_json::to_value(old)
                .unwrap()
                .get("engine_metrics")
                .is_none()
        );
    }
    #[test]
    fn gzip_round_trip_keeps_histogram_labels_and_rejects_invalid_sample() {
        let sample = json!({"seq":2,"deployment_id":"gemma","generation":1,"recipe_digest":"a".repeat(64),"image":"image","sampled_at_ms":1000,"duration_ms":0.2,"metrics":[{"name":"vllm:latency_bucket","family":"vllm:latency","type":"histogram","help":"Queue time","labels":{"le":"+Inf","model_name":"Gemma"},"value":4}]});
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(serde_json::to_string(&sample).unwrap().as_bytes())
            .unwrap();
        let raw = STANDARD.encode(z.finish().unwrap());
        let decoded = decode(&raw).unwrap();
        assert_eq!(decoded.metrics[0].labels["le"], "+Inf");
        assert_eq!(decoded.metrics[0].value, Some(4.0));
        assert_eq!(
            serde_json::from_value::<Sample>(serde_json::to_value(&decoded).unwrap())
                .unwrap()
                .seq,
            2
        );
        assert!(decode("garbage").is_err());
    }
}

//! Bounded, credential-free Fleet host observations carried by mesh heartbeats.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct FleetHostSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_capacity: Option<ManagedCapacity>,
    pub schema_version: u32,
    pub sampled_at: String,
    pub hostname: String,
    pub architecture: String,
    pub uptime_seconds: Option<f64>,
    pub cpu: Option<HostCpu>,
    pub memory: Option<HostMemory>,
    pub disk: Option<HostMemory>,
    pub network: Vec<HostNetwork>,
    pub gpus: Vec<HostGpu>,
    pub containers: Vec<HostContainer>,
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub engine_observations: Vec<EngineObservation>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct ManagedCapacity {
    pub memory_budget_mib: u64,
    pub memory_reserved_mib: u64,
    pub gpu_sharing_enabled: bool,
    pub gpus: Vec<ManagedGpuCapacity>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct ManagedGpuCapacity {
    pub index: u32,
    pub total_mib: u64,
    pub budget_mib: u64,
    pub reserved_mib: u64,
    pub unified_memory: bool,
    #[serde(default)]
    pub exclusive_deployment: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HostCpu {
    pub logical_cores: u32,
    pub utilization_percent: Option<f64>,
    pub load_average: Vec<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HostMemory {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HostNetwork {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bytes_per_second: Option<f64>,
    pub tx_bytes_per_second: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HostGpu {
    pub index: u32,
    pub uuid: String,
    pub name: String,
    pub unified_memory: bool,
    pub memory_total_mib: Option<f64>,
    pub memory_used_mib: Option<f64>,
    pub utilization_percent: Option<f64>,
    pub temperature_celsius: Option<f64>,
    pub power_watts: Option<f64>,
    #[serde(default)]
    pub clock_mhz: Option<f64>,
    #[serde(default)]
    pub memory_clock_mhz: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HostContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<f64>,
    pub pids: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct EngineObservation {
    pub instance: String,
    pub model: String,
    pub engine: String,
    pub source: String,
    pub sampled_at: String,
    pub received_at: String,
    pub running: u32,
    pub waiting: u32,
    pub requests: Vec<EngineRequest>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct EngineRequest {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_request_id: Option<String>,
    pub state: String,
    pub prompt_tokens: u64,
    pub computed_tokens: u64,
    pub output_tokens: u64,
    pub preemptions: u64,
}
impl EngineObservation {
    fn validate(&self) -> bool {
        let text = |s: &str, max: usize| {
            !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
        };
        text(&self.instance, 64)
            && text(&self.model, 256)
            && self.engine == "vllm"
            && self.source == "scheduler"
            && chrono::DateTime::parse_from_rfc3339(&self.sampled_at).is_ok()
            && chrono::DateTime::parse_from_rfc3339(&self.received_at).is_ok()
            && self.running <= 1_000_000
            && self.waiting <= 1_000_000
            && self.requests.len() <= 8
            && self.requests.iter().all(|r| {
                r.id.len() == 32
                    && r.id.bytes().all(|b| b.is_ascii_hexdigit())
                    && r.gateway_request_id
                        .as_ref()
                        .is_none_or(|id| uuid::Uuid::parse_str(id).is_ok())
                    && matches!(
                        r.state.as_str(),
                        "queued" | "prefilling" | "decoding" | "recomputing" | "preempted"
                    )
                    && [
                        r.prompt_tokens,
                        r.computed_tokens,
                        r.output_tokens,
                        r.preemptions,
                    ]
                    .iter()
                    .all(|v| *v <= 1_000_000_000)
            })
    }
}
impl FleetHostSnapshot {
    pub fn validate(&self) -> bool {
        if self.managed_capacity.as_ref().is_some_and(|c| {
            c.memory_budget_mib > 1 << 30
                || c.memory_reserved_mib > c.memory_budget_mib
                || c.gpus.len() > 16
                || c.gpus.iter().any(|g| {
                    g.budget_mib > g.total_mib
                        || g.total_mib > 1 << 30
                        || g.reserved_mib > g.budget_mib
                        || g.exclusive_deployment.len() > 40
                        || !g.exclusive_deployment.bytes().all(|b| {
                            b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b)
                        })
                })
        }) {
            return false;
        }
        let text = |s: &str, max| s.len() <= max && !s.chars().any(char::is_control);
        let metric = |v: Option<f64>| v.is_none_or(|v| v.is_finite() && v >= 0.0);
        let percent = |v: Option<f64>| metric(v) && v.is_none_or(|v| v <= 100.0);
        self.engine_observations.len()<=4 && self.engine_observations.iter().all(EngineObservation::validate)
            && self.schema_version == 1
            && chrono::DateTime::parse_from_rfc3339(&self.sampled_at).is_ok()
            && !self.hostname.is_empty() && text(&self.hostname, 128)
            && text(&self.architecture, 32) && metric(self.uptime_seconds)
            && self.cpu.as_ref().is_none_or(|c| c.logical_cores > 0 && c.logical_cores <= 65536
                && percent(c.utilization_percent) && c.load_average.len() <= 3
                && c.load_average.iter().all(|v| metric(Some(*v))))
            && [&self.memory, &self.disk].iter().all(|m| m.as_ref().is_none_or(|m|
                m.available_bytes <= m.total_bytes && m.used_bytes <= m.total_bytes))
            && self.network.len() <= 32 && self.network.iter().all(|n| text(&n.name, 128)
                && metric(n.rx_bytes_per_second) && metric(n.tx_bytes_per_second))
            && self.gpus.len() <= 16 && self.gpus.iter().all(|g| text(&g.uuid, 128) && text(&g.name, 256)
                && metric(g.memory_total_mib) && metric(g.memory_used_mib)
                && percent(g.utilization_percent) && metric(g.temperature_celsius) && metric(g.power_watts) && metric(g.clock_mhz) && metric(g.memory_clock_mhz))
            && self.containers.len() <= 64 && self.containers.iter().all(|c|
                text(&c.id, 128) && text(&c.name, 256) && text(&c.image, 512)
                && text(&c.state, 64) && text(&c.status, 256)
                && metric(c.cpu_percent) && metric(c.memory_bytes) && metric(c.pids))
            && self.errors.len() <= 16 && self.errors.iter().all(|s| text(s, 64))
            // Leave room for resources and framing in the 64 KiB mesh control frame.
            && serde_json::to_vec(self).is_ok_and(|v| v.len() <= 32 * 1024)
    }
}

#[cfg(test)]
pub(crate) fn fixture() -> FleetHostSnapshot {
    serde_json::from_value(serde_json::json!({
        "schema_version":1,"sampled_at":"2026-10-03T12:00:00Z",
        "hostname":"spark-test","architecture":"arm64","uptime_seconds":100.0,
        "cpu":null,"memory":{"total_bytes":1000,"available_bytes":600,"used_bytes":400},
        "disk":null,"network":[],"containers":[],"errors":[],
        "gpus":[{"index":0,"uuid":"GPU-1","name":"NVIDIA GB10","unified_memory":true,
            "memory_total_mib":null,"memory_used_mib":null,"utilization_percent":5.0,
            "temperature_celsius":40.0,"power_watts":null}]
    }))
    .unwrap()
}
#[cfg(test)]
mod tests {
    #[test]
    fn engine_observations_and_gpu_clocks_survive_wire_round_trip() {
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["gpus"][0]["clock_mhz"] = serde_json::json!(1500.0);
        value["gpus"][0]["memory_clock_mhz"] = serde_json::json!(1200.0);
        value["engine_observations"] = serde_json::json!([{"instance":"gemma","model":"gemma","engine":"vllm","source":"scheduler","sampled_at":"2026-10-03T12:00:00Z","received_at":"2026-10-03T12:00:00Z","running":1,"waiting":0,"requests":[{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","gateway_request_id":"550e8400-e29b-41d4-a716-446655440000","state":"decoding","prompt_tokens":10,"computed_tokens":11,"output_tokens":1,"preemptions":0}]}]);
        let sample: super::FleetHostSnapshot = serde_json::from_value(value).unwrap();
        assert!(sample.validate());
        assert_eq!(
            sample,
            serde_json::from_slice(&serde_json::to_vec(&sample).unwrap()).unwrap()
        );
    }

    use super::*;
    #[test]
    fn fleet_host_round_trip_preserves_unavailable_unified_memory() {
        let mut shared = fixture();
        shared.managed_capacity = Some(ManagedCapacity {
            memory_budget_mib: 116000,
            memory_reserved_mib: 40960,
            gpu_sharing_enabled: true,
            gpus: vec![ManagedGpuCapacity {
                index: 0,
                total_mib: 124000,
                budget_mib: 116000,
                reserved_mib: 32768,
                unified_memory: true,
                exclusive_deployment: String::new(),
            }],
        });
        assert!(shared.validate());
        let roundtrip: FleetHostSnapshot =
            serde_json::from_value(serde_json::to_value(&shared).unwrap()).unwrap();
        assert_eq!(shared, roundtrip);
        shared.managed_capacity.as_mut().unwrap().gpus[0].reserved_mib = 200000;
        assert!(!shared.validate());
        let sample = fixture();
        assert!(sample.validate());
        let raw = serde_json::to_vec(&sample).unwrap();
        let decoded: FleetHostSnapshot = serde_json::from_slice(&raw).unwrap();
        assert_eq!(sample, decoded);
        assert!(decoded.gpus[0].unified_memory);
        assert_eq!(decoded.gpus[0].memory_total_mib, None);
    }
    #[test]
    fn fleet_host_rejects_unsupported_oversized_and_invalid_reports() {
        let mut sample = fixture();
        sample.schema_version = 2;
        assert!(!sample.validate());
        sample = fixture();
        sample.gpus[0].utilization_percent = Some(f64::NAN);
        assert!(!sample.validate());
        sample = fixture();
        sample.hostname = "bad\nhost".into();
        assert!(!sample.validate());
        sample = fixture();
        sample.network = vec![
            HostNetwork {
                name: "a".into(),
                rx_bytes: 0,
                tx_bytes: 0,
                rx_bytes_per_second: None,
                tx_bytes_per_second: None
            };
            33
        ];
        assert!(!sample.validate());
    }
    #[test]
    fn fleet_host_heartbeat_round_trip_and_legacy_compatibility() {
        use crate::mesh::protocol::{Heartbeat, validate_heartbeat};
        let heartbeat = Heartbeat {
            sequence: 1,
            resources: vec![],
            host_snapshot: Some(fixture()),
        };
        assert!(validate_heartbeat(&heartbeat).is_ok());
        assert_eq!(
            heartbeat,
            serde_json::from_slice::<Heartbeat>(&serde_json::to_vec(&heartbeat).unwrap()).unwrap()
        );
        let legacy: Heartbeat = serde_json::from_str(r#"{"sequence":1,"resources":[]}"#).unwrap();
        assert!(legacy.host_snapshot.is_none());
        assert_eq!(
            serde_json::to_value(legacy).unwrap(),
            serde_json::json!({"sequence":1,"resources":[]})
        );
    }
}

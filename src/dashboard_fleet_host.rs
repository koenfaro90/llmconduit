//! Bounded, credential-free Fleet host observations carried by mesh heartbeats.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct FleetHostSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<HostCollection>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filesystems: Vec<HostFilesystem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub block_devices: Vec<HostBlockDevice>,
    pub network: Vec<HostNetwork>,
    pub gpus: Vec<HostGpu>,
    pub containers: Vec<HostContainer>,
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub engine_observations: Vec<EngineObservation>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HostCollection {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub metrics: std::collections::BTreeMap<String, HostMetricCollection>,
    pub host_interval_ms: u64,
    pub gpu_interval_ms: u64,
    pub container_interval_ms: u64,
    pub gpu_backend: String,
    pub gpu_sampled_at: Option<String>,
    pub containers_sampled_at: Option<String>,
    pub gpu_duration_ms: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HostMetricCollection {
    pub interval_ms: u64,
    pub sampled_at: Option<String>,
}
impl HostMetricCollection {
    fn validate(&self, name: &str) -> bool {
        let minimum = match name {
            "cpu" | "memory" | "network" | "disk" | "disk_io" | "gpu_utilization"
            | "gpu_memory" | "gpu_temperature" | "gpu_power" | "gpu_clock" | "gpu_memory_clock"
            | "gpu_fan" | "pcie_rx" | "pcie_tx" | "pcie_link" | "pcie_errors" => 100,
            "container_inventory" | "container_cpu" | "container_memory" | "container_pids" => 1000,
            _ => return false,
        };
        (self.interval_ms == 0 || (minimum..=60000).contains(&self.interval_ms))
            && self.sampled_at.as_ref().is_none_or(|at| {
                self.interval_ms != 0 && chrono::DateTime::parse_from_rfc3339(at).is_ok()
            })
    }
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
pub struct HostFilesystem {
    pub mount_point: String,
    pub mount_points: Vec<String>,
    pub device: String,
    pub device_id: String,
    pub filesystem_type: String,
    pub read_only: bool,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub inodes_total: Option<u64>,
    pub inodes_free: Option<u64>,
    pub inodes_used: Option<u64>,
}
fn storage_device_id(value: &str) -> bool {
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u32>().is_ok_and(|v| v <= 1 << 20)
        })
}
impl HostFilesystem {
    fn validate(&self) -> bool {
        let path = |p: &str| {
            p.starts_with('/')
                && p.len() <= 512
                && !p.chars().any(char::is_control)
                && !p.split('/').any(|part| matches!(part, "." | ".."))
        };
        path(&self.mount_point)
            && (1..=8).contains(&self.mount_points.len())
            && self.mount_points.contains(&self.mount_point)
            && self.mount_points.iter().all(|p| path(p))
            && self.device.len() <= 128
            && !self.device.chars().any(char::is_control)
            && storage_device_id(&self.device_id)
            && matches!(
                self.filesystem_type.as_str(),
                "ext2"
                    | "ext3"
                    | "ext4"
                    | "xfs"
                    | "btrfs"
                    | "f2fs"
                    | "vfat"
                    | "exfat"
                    | "zfs"
                    | "bcachefs"
                    | "ntfs3"
                    | "overlay"
            )
            && self.free_bytes <= self.total_bytes
            && self.available_bytes <= self.free_bytes
            && self.used_bytes == self.total_bytes - self.free_bytes
            && match (self.inodes_total, self.inodes_free, self.inodes_used) {
                (None, None, None) => true,
                (Some(total), Some(free), Some(used)) => {
                    total > 0 && free <= total && used == total - free
                }
                _ => false,
            }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HostBlockDevice {
    pub name: String,
    pub device_id: String,
    pub leaf_device: bool,
    pub read_bytes_per_second: Option<f64>,
    pub write_bytes_per_second: Option<f64>,
    pub read_iops: Option<f64>,
    pub write_iops: Option<f64>,
    pub read_latency_ms: Option<f64>,
    pub write_latency_ms: Option<f64>,
    pub busy_percent: Option<f64>,
    pub in_flight: u64,
    pub average_queue_depth: Option<f64>,
}
impl HostBlockDevice {
    fn validate(&self) -> bool {
        !self.name.is_empty()
            && self.name.len() <= 64
            && !matches!(self.name.as_str(), "." | "..")
            && self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            && storage_device_id(&self.device_id)
            && [
                self.read_bytes_per_second,
                self.write_bytes_per_second,
                self.read_iops,
                self.write_iops,
                self.read_latency_ms,
                self.write_latency_ms,
                self.busy_percent,
                self.average_queue_depth,
            ]
            .iter()
            .all(|v| v.is_none_or(|v| v.is_finite() && v >= 0.0))
            && self.busy_percent.is_none_or(|v| v <= 100.0)
    }
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
    #[serde(default)]
    pub fan_speed_percent: Option<f64>,
    #[serde(default)]
    pub pcie_rx_kb_per_second: Option<f64>,
    #[serde(default)]
    pub pcie_tx_kb_per_second: Option<f64>,
    #[serde(default)]
    pub pcie_link_generation: Option<f64>,
    #[serde(default)]
    pub pcie_link_width: Option<f64>,
    #[serde(default)]
    pub pcie_max_link_generation: Option<f64>,
    #[serde(default)]
    pub pcie_max_link_width: Option<f64>,
    #[serde(default)]
    pub pcie_replay_count: Option<f64>,
    #[serde(default)]
    pub pcie_recovery_count: Option<f64>,
    #[serde(default)]
    pub pcie_correctable_error_count: Option<f64>,
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
        self.collection.as_ref().is_none_or(|c|
            (100..=60000).contains(&c.host_interval_ms)
            && (100..=60000).contains(&c.gpu_interval_ms)
            && (1000..=60000).contains(&c.container_interval_ms)
            && matches!(c.gpu_backend.as_str(), "" | "nvml" | "nvidia-smi")
            && [&c.gpu_sampled_at, &c.containers_sampled_at].iter().all(|v| v.as_ref().is_none_or(|v| chrono::DateTime::parse_from_rfc3339(v).is_ok()))
            && metric(c.gpu_duration_ms)
            && c.metrics.len() <= 20 && c.metrics.iter().all(|(name, value)| value.validate(name)))
            && self.engine_observations.len()<=4 && self.engine_observations.iter().all(EngineObservation::validate)
            && self.schema_version == 1
            && chrono::DateTime::parse_from_rfc3339(&self.sampled_at).is_ok()
            && !self.hostname.is_empty() && text(&self.hostname, 128)
            && text(&self.architecture, 32) && metric(self.uptime_seconds)
            && self.cpu.as_ref().is_none_or(|c| c.logical_cores > 0 && c.logical_cores <= 65536
                && percent(c.utilization_percent) && c.load_average.len() <= 3
                && c.load_average.iter().all(|v| metric(Some(*v))))
            && [&self.memory, &self.disk].iter().all(|m| m.as_ref().is_none_or(|m|
                m.available_bytes <= m.total_bytes && m.used_bytes <= m.total_bytes))
            && self.filesystems.len() <= 16 && self.filesystems.iter().all(HostFilesystem::validate)
            && self.block_devices.len() <= 16 && self.block_devices.iter().all(HostBlockDevice::validate)
            && self.network.len() <= 32 && self.network.iter().all(|n| text(&n.name, 128)
                && metric(n.rx_bytes_per_second) && metric(n.tx_bytes_per_second))
            && self.gpus.len() <= 16 && self.gpus.iter().all(|g| text(&g.uuid, 128) && text(&g.name, 256)
                && metric(g.memory_total_mib) && metric(g.memory_used_mib)
                && percent(g.utilization_percent) && metric(g.temperature_celsius) && metric(g.power_watts) && metric(g.clock_mhz) && metric(g.memory_clock_mhz) && metric(g.fan_speed_percent) && metric(g.pcie_rx_kb_per_second) && metric(g.pcie_tx_kb_per_second) && metric(g.pcie_link_generation) && metric(g.pcie_link_width) && metric(g.pcie_max_link_generation) && metric(g.pcie_max_link_width) && metric(g.pcie_replay_count) && metric(g.pcie_recovery_count) && metric(g.pcie_correctable_error_count))
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
    fn storage_round_trip_legacy_nulls_and_validation() {
        let legacy = fixture();
        assert!(legacy.filesystems.is_empty() && legacy.block_devices.is_empty());
        let mut value = serde_json::to_value(legacy).unwrap();
        value["filesystems"] = serde_json::json!([{"mount_point":"/","mount_points":["/","/home"],"device":"/dev/nvme0n1p2","device_id":"259:2","filesystem_type":"ext4","read_only":false,"total_bytes":1000,"free_bytes":600,"available_bytes":500,"used_bytes":400,"inodes_total":100,"inodes_free":30,"inodes_used":70}]);
        value["block_devices"] = serde_json::json!([{"name":"nvme0n1","device_id":"259:0","leaf_device":true,"read_bytes_per_second":512.0,"write_bytes_per_second":0.0,"read_iops":1.0,"write_iops":0.0,"read_latency_ms":2.0,"write_latency_ms":null,"busy_percent":2.0,"in_flight":0,"average_queue_depth":0.02}]);
        value["collection"] = serde_json::json!({"host_interval_ms":1000,"gpu_interval_ms":5000,"container_interval_ms":5000,"gpu_backend":"nvml","gpu_sampled_at":null,"containers_sampled_at":null,"gpu_duration_ms":null,"metrics":{"disk":{"interval_ms":30000,"sampled_at":"2026-10-03T12:00:00Z"},"disk_io":{"interval_ms":1000,"sampled_at":"2026-10-03T12:00:00Z"}}});
        let snapshot: FleetHostSnapshot = serde_json::from_value(value.clone()).unwrap();
        assert!(snapshot.validate());
        assert_eq!(
            snapshot,
            serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap()
        );
        for (field, bad) in [
            ("mount_point", serde_json::json!("/../private")),
            ("available_bytes", serde_json::json!(700)),
            ("inodes_used", serde_json::json!(71)),
            ("device_id", serde_json::json!("SECRET")),
        ] {
            let mut bad_value = value.clone();
            bad_value["filesystems"][0][field] = bad;
            assert!(
                !serde_json::from_value::<FleetHostSnapshot>(bad_value)
                    .unwrap()
                    .validate()
            );
        }
        for (field, bad) in [
            ("name", serde_json::json!("../device")),
            ("busy_percent", serde_json::json!(101.0)),
            ("read_iops", serde_json::json!(-1.0)),
        ] {
            let mut bad_value = value.clone();
            bad_value["block_devices"][0][field] = bad;
            assert!(
                !serde_json::from_value::<FleetHostSnapshot>(bad_value)
                    .unwrap()
                    .validate()
            );
        }
        value["block_devices"][0]["read_bytes_per_second"] = serde_json::Value::Null;
        value["block_devices"][0]["read_iops"] = serde_json::Value::Null;
        for field in ["inodes_total", "inodes_free", "inodes_used"] {
            value["filesystems"][0][field] = serde_json::Value::Null;
        }
        assert!(
            serde_json::from_value::<FleetHostSnapshot>(value)
                .unwrap()
                .validate()
        );
        let mut bounded = snapshot;
        bounded.filesystems = vec![bounded.filesystems[0].clone(); 17];
        assert!(!bounded.validate());
    }
    #[test]
    fn fan_telemetry_preserves_unavailable_and_above_100_with_own_interval() {
        let mut sample = fixture();
        assert!(sample.gpus[0].fan_speed_percent.is_none());
        sample.gpus[0].fan_speed_percent = Some(123.0);
        sample.collection=Some(serde_json::from_value(serde_json::json!({"host_interval_ms":5000,"gpu_interval_ms":5000,"container_interval_ms":5000,"gpu_backend":"nvml","gpu_sampled_at":null,"containers_sampled_at":null,"gpu_duration_ms":null,"metrics":{"gpu_fan":{"interval_ms":1000,"sampled_at":"2026-10-03T12:00:00Z"}}})).unwrap());
        assert!(sample.validate());
        let decoded: FleetHostSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&sample).unwrap()).unwrap();
        assert_eq!(decoded.gpus[0].fan_speed_percent, Some(123.0));
        assert_eq!(
            decoded.collection.unwrap().metrics["gpu_fan"].interval_ms,
            1000
        );
        sample.gpus[0].fan_speed_percent = Some(-1.0);
        assert!(!sample.validate());
        sample.gpus[0].fan_speed_percent = Some(f64::INFINITY);
        assert!(!sample.validate());
    }
    #[test]
    fn per_metric_collection_survives_mesh_and_rejects_invalid_overrides() {
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["collection"] = serde_json::json!({"host_interval_ms":100,"gpu_interval_ms":100,"container_interval_ms":5000,"gpu_backend":"nvml","gpu_sampled_at":null,"containers_sampled_at":null,"gpu_duration_ms":null,"metrics":{"cpu":{"interval_ms":100,"sampled_at":"2026-10-03T12:00:00Z"},"container_cpu":{"interval_ms":0,"sampled_at":null}}});
        let snapshot: FleetHostSnapshot = serde_json::from_value(value.clone()).unwrap();
        assert!(snapshot.validate());
        assert_eq!(
            serde_json::to_value(snapshot).unwrap()["collection"]["metrics"],
            value["collection"]["metrics"]
        );
        for (name, interval, at) in [
            ("unknown", 5000, None),
            ("container_cpu", 100, None),
            ("cpu", 61000, None),
            ("cpu", 0, Some("2026-10-03T12:00:00Z")),
            ("cpu", 100, Some("bad")),
        ] {
            value["collection"]["metrics"] =
                serde_json::json!({name:{"interval_ms":interval,"sampled_at":at}});
            assert!(
                !serde_json::from_value::<FleetHostSnapshot>(value.clone())
                    .unwrap()
                    .validate()
            );
        }
    }
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

    #[test]
    fn pcie_and_sampling_cadence_survive_wire_and_invalid_values_are_rejected() {
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["collection"] = serde_json::json!({"host_interval_ms":100,"gpu_interval_ms":100,"container_interval_ms":5000,"gpu_backend":"nvml","gpu_sampled_at":"2026-10-03T12:00:00Z","containers_sampled_at":null,"gpu_duration_ms":45.0});
        for field in [
            "pcie_rx_kb_per_second",
            "pcie_tx_kb_per_second",
            "pcie_link_generation",
            "pcie_link_width",
            "pcie_max_link_generation",
            "pcie_max_link_width",
            "pcie_replay_count",
            "pcie_correctable_error_count",
            "pcie_recovery_count",
        ] {
            value["gpus"][0][field] = serde_json::json!(123.0);
        }
        let sample: super::FleetHostSnapshot = serde_json::from_value(value).unwrap();
        assert!(sample.validate());
        assert_eq!(
            sample,
            serde_json::from_slice(&serde_json::to_vec(&sample).unwrap()).unwrap()
        );
        let mut bad = sample.clone();
        bad.gpus[0].pcie_rx_kb_per_second = Some(-1.0);
        assert!(!bad.validate());
        bad = sample.clone();
        bad.collection.as_mut().unwrap().container_interval_ms = 100;
        assert!(!bad.validate());
        bad = sample;
        bad.collection.as_mut().unwrap().gpu_sampled_at = Some("invalid".into());
        assert!(!bad.validate());
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

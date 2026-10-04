//! Central admission uses desired state for race-free reservations. Fleet checks
//! the same budgets locally; observed availability is never the reservation ledger.
use crate::dashboard_fleet_host::FleetHostSnapshot;
use crate::dashboard_management::{DesiredDeployment, ManagementState};
use serde_json::Value;
use std::collections::BTreeMap;
fn memory_mib(spec: &Value) -> Result<u64, String> {
    let raw = spec
        .get("memory_limit")
        .and_then(Value::as_str)
        .ok_or("memory limit is missing")?;
    if !raw.is_ascii() {
        return Err("invalid memory limit".into());
    }
    let (amount, unit) = raw.split_at(raw.len().checked_sub(1).ok_or("invalid memory limit")?);
    let value = amount.parse::<u64>().map_err(|_| "invalid memory limit")?;
    match unit {
        "m" | "M" => Ok(value),
        "g" | "G" => value.checked_mul(1024).ok_or("invalid memory limit".into()),
        _ => Err("memory limit needs MiB/GiB units".into()),
    }
}
fn devices(spec: &Value) -> Vec<u32> {
    spec.get("gpus")
        .and_then(Value::as_str)
        .and_then(|s| s.strip_prefix("device="))
        .map(|s| s.split(',').filter_map(|i| i.parse().ok()).collect())
        .unwrap_or_default()
}
fn held(d: &DesiredDeployment) -> bool {
    d.desired_state == "ready"
        || d.observed
            .as_ref()
            .and_then(|o| o.pointer("/status/phase"))
            .and_then(Value::as_str)
            .is_some_and(|s| s != "unloaded")
}
pub(crate) fn check(
    state: &ManagementState,
    id: &str,
    candidate: &DesiredDeployment,
    snapshot: Option<&FleetHostSnapshot>,
) -> Result<(), String> {
    if candidate.desired_state != "ready" {
        return Ok(());
    }
    let revision = state
        .recipes
        .get(&candidate.recipe_id)
        .and_then(|r| r.iter().find(|r| r.digest == candidate.revision_digest))
        .ok_or("recipe revision is missing")?;
    let incoming = crate::dashboard_deployment_groups::render(
        &revision.model_spec,
        candidate.placement.as_ref(),
    )?;
    let shared = incoming.pointer("/gpu_memory/mode").and_then(Value::as_str) == Some("shared");
    // Old agents can keep accepting their existing exclusive contract. Shared
    // placement requires a fresh, explicit policy/capacity report from the host.
    let snapshot = match snapshot {
        Some(s) => s,
        None if !shared => return Ok(()),
        None => return Err("host capacity is unavailable".into()),
    };
    let Some(capacity) = snapshot.managed_capacity.as_ref() else {
        return if shared {
            Err("host does not report shared GPU capacity".into())
        } else {
            Ok(())
        };
    };
    let sampled = chrono::DateTime::parse_from_rfc3339(&snapshot.sampled_at)
        .map_err(|_| "host capacity is stale")?
        .timestamp_millis();
    let age = chrono::Utc::now().timestamp_millis() - sampled;
    if !(-30_000..=20_000).contains(&age) {
        return Err("host capacity is stale".into());
    }
    if shared && !capacity.gpu_sharing_enabled {
        return Err("host policy does not allow shared GPUs".into());
    }
    let mut ram = 0u64;
    let mut reservations: BTreeMap<u32, (u64, bool, String)> = BTreeMap::new();
    let mut all = state
        .deployments
        .iter()
        .filter(|(other, d)| {
            other.as_str() != id && d.endpoint_id == candidate.endpoint_id && held(d)
        })
        .map(|(other, d)| (other.as_str(), d))
        .collect::<Vec<_>>();
    all.push((id, candidate));
    for (other, d) in all {
        let revision = state
            .recipes
            .get(&d.recipe_id)
            .and_then(|r| r.iter().find(|r| r.digest == d.revision_digest))
            .ok_or("existing deployment revision is missing")?;
        let spec =
            crate::dashboard_deployment_groups::render(&revision.model_spec, d.placement.as_ref())?;
        let memory = memory_mib(&spec)?;
        ram = ram
            .checked_add(memory)
            .ok_or("host memory estimate overflow")?;
        let share = spec.pointer("/gpu_memory/mode").and_then(Value::as_str) == Some("shared");
        let mut unified = 0;
        for index in devices(&spec) {
            let gpu = capacity
                .gpus
                .iter()
                .find(|g| g.index == index)
                .ok_or("host GPU capacity is unavailable")?;
            if share && (!capacity.gpu_sharing_enabled || gpu.budget_mib == 0) {
                return Err("host policy does not allow shared GPU memory".into());
            }
            let amount = if share {
                spec.pointer("/gpu_memory/mib_per_device")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .max(
                        (spec
                            .pointer("/gpu_memory/fraction")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                            * gpu.total_mib as f64)
                            .ceil() as u64,
                    )
            } else {
                gpu.budget_mib
            };
            let entry = reservations
                .entry(index)
                .or_insert((0, false, String::new()));
            if !entry.2.is_empty() && (!share || entry.1) {
                return Err(format!(
                    "GPU {index} is reserved exclusively by {}",
                    entry.2
                ));
            }
            entry.0 = entry.0.checked_add(amount).ok_or("GPU estimate overflow")?;
            entry.1 = !share;
            entry.2 = other.into();
            if gpu.budget_mib > 0 && entry.0 > gpu.budget_mib {
                return Err(format!(
                    "GPU {index} memory reservations exceed {} MiB",
                    gpu.budget_mib
                ));
            }
            if gpu.unified_memory && share {
                unified += amount
            }
        }
        if unified > memory {
            return Err("unified GPU estimate exceeds the container RAM reservation".into());
        }
    }
    if ram > capacity.memory_budget_mib {
        return Err(format!(
            "host RAM reservations exceed {} MiB",
            capacity.memory_budget_mib
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (ManagementState, DesiredDeployment, FleetHostSnapshot) {
        let state:ManagementState=serde_json::from_value(json!({"recipes":{"r":[{"digest":"d","revision":1,"description":"","created_at_ms":1,"model_spec":{"image":"fixture","memory_limit":"128m","gpus":"device=0","gpu_memory":{"mode":"shared","mib_per_device":100,"fraction":0.2}}}]},"deployments":{}})).unwrap();
        let d:DesiredDeployment=serde_json::from_value(json!({"generation":1,"endpoint_id":"host","recipe_id":"r","revision_digest":"d","desired_state":"ready","updated_at_ms":1,"observed_at_ms":null,"observed":null,"error":null})).unwrap();
        let mut h = crate::dashboard_fleet_host::fixture();
        h.sampled_at = chrono::Utc::now().to_rfc3339();
        h.managed_capacity=Some(serde_json::from_value(json!({"memory_budget_mib":1024,"memory_reserved_mib":0,"gpu_sharing_enabled":true,"gpus":[{"index":0,"total_mib":1000,"budget_mib":350,"reserved_mib":0,"unified_memory":false}]})).unwrap());
        (state, d, h)
    }
    #[test]
    fn capacity_counts_pending_and_stopping_intent_and_does_not_double_count_itself() {
        let (mut state, d, h) = fixture();
        assert!(check(&state, "one", &d, Some(&h)).is_ok());
        state.deployments.insert("one".into(), d.clone());
        assert!(check(&state, "one", &d, Some(&h)).is_ok());
        assert!(
            check(&state, "two", &d, Some(&h))
                .unwrap_err()
                .contains("GPU 0 memory")
        );
        let one = state.deployments.get_mut("one").unwrap();
        one.desired_state = "unloaded".into();
        one.observed = Some(json!({"status":{"phase":"stopping"}}));
        assert!(check(&state, "two", &d, Some(&h)).is_err());
        let one = state.deployments.get_mut("one").unwrap();
        one.observed = Some(json!({"status":{"phase":"unloaded"}}));
        assert!(check(&state, "two", &d, Some(&h)).is_ok());
    }
    #[test]
    fn reservations_follow_placement_devices_instead_of_recipe_defaults() {
        let (mut state, d, mut h) = fixture();
        let mut second = h.managed_capacity.as_ref().unwrap().gpus[0].clone();
        second.index = 1;
        h.managed_capacity.as_mut().unwrap().gpus.push(second);
        state.deployments.insert("first".into(), d.clone());
        let mut placed = d.clone();
        placed.placement = Some(crate::dashboard_deployment_groups::Placement {
            group_id: "g".into(),
            rank: 0,
            addresses: vec!["host".into()],
            gpus: Some(vec![1]),
        });
        assert!(check(&state, "second", &placed, Some(&h)).is_ok());
        state.deployments.insert("second".into(), placed.clone());
        assert!(
            check(&state, "third", &placed, Some(&h))
                .unwrap_err()
                .contains("GPU 1 memory")
        );
    }
    #[test]
    fn shared_capacity_requires_fresh_permission_and_unified_ram_coverage() {
        let (state, d, mut h) = fixture();
        assert!(check(&state, "one", &d, None).is_err());
        h.managed_capacity.as_mut().unwrap().gpu_sharing_enabled = false;
        assert!(check(&state, "one", &d, Some(&h)).is_err());
        h.managed_capacity.as_mut().unwrap().gpu_sharing_enabled = true;
        h.managed_capacity.as_mut().unwrap().gpus[0].unified_memory = true;
        assert!(
            check(&state, "one", &d, Some(&h))
                .unwrap_err()
                .contains("unified")
        );
        h.managed_capacity.as_mut().unwrap().gpus[0].unified_memory = false;
        h.sampled_at = "2020-01-01T00:00:00Z".into();
        assert!(
            check(&state, "one", &d, Some(&h))
                .unwrap_err()
                .contains("stale")
        );
    }
    #[test]
    fn shared_and_exclusive_routes_cannot_mix_and_ram_reservations_are_separate() {
        let (mut state, d, h) = fixture();
        let mut exclusive = state.recipes["r"][0].clone();
        exclusive.digest = "exclusive".into();
        exclusive
            .model_spec
            .as_object_mut()
            .unwrap()
            .remove("gpu_memory");
        state.recipes.get_mut("r").unwrap().push(exclusive);
        let mut other = d.clone();
        other.revision_digest = "exclusive".into();
        state.deployments.insert("exclusive".into(), other);
        assert!(
            check(&state, "shared", &d, Some(&h))
                .unwrap_err()
                .contains("exclusively")
        );
        state.deployments.clear();
        state.recipes.get_mut("r").unwrap()[0].model_spec["memory_limit"] = json!("2048m");
        assert!(
            check(&state, "shared", &d, Some(&h))
                .unwrap_err()
                .contains("RAM")
        );
    }
}

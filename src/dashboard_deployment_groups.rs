//! Atomic desired-state selection for replicas or coordinated distributed ranks.
//! Actual host execution remains asynchronous and reports each rank separately.
use crate::{
    dashboard_api::json_no_store,
    dashboard_auth::{AuthSession, DashboardAuth},
    dashboard_management::{self, DesiredDeployment},
    dashboard_mesh::authorize_mesh_mutation,
    engine::Gateway,
};
use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use utoipa::ToSchema;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub group_id: String,
    pub rank: u32,
    pub addresses: Vec<String>,
    /// Physical host indices; absence retains the pinned recipe's device selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<Vec<u32>>,
    /// Authoritative GPU identities keep CUDA masks correct across privileged containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_uuids: Option<Vec<String>>,
}
#[derive(Clone, utoipa::ToSchema, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub endpoint_id: String,
    pub address: String,
    #[serde(default)]
    pub gpus: Option<Vec<u32>>,
}
#[derive(Clone, utoipa::ToSchema, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    #[serde(default)]
    pub size: Option<String>,
    pub group_id: Option<String>,
    pub recipe_id: String,
    pub revision_digest: String,
    pub hosts: Vec<Host>,
    #[serde(default)]
    pub expected_generations: BTreeMap<String, u64>,
    pub desired_state: String,
}
fn err(status: StatusCode, message: &str) -> Response {
    json_no_store(status, &json!({"error":message}))
}
pub fn render(spec: &Value, placement: Option<&Placement>) -> Result<Value, String> {
    let mut model = if let Some(ranks) = spec.get("distributed").and_then(Value::as_array) {
        let p = placement.ok_or("distributed deployment needs placement")?;
        if p.addresses.len() != ranks.len() {
            return Err("placement rank count differs from recipe".into());
        }
        ranks.get(p.rank as usize).cloned().ok_or("invalid rank")?
    } else {
        spec.clone()
    };
    if let Some(object) = model.as_object_mut() {
        object.remove("metadata");
        object.remove("sizes");
    }
    if let Some(p) = placement {
        if p.addresses.is_empty()
            || p.rank as usize >= p.addresses.len()
            || p.addresses.iter().any(|s| !valid_address(s))
        {
            return Err("invalid placement addresses".into());
        }
        if let Some(selected) = &p.gpus {
            let original = model
                .get("gpus")
                .and_then(Value::as_str)
                .and_then(|s| s.strip_prefix("device="))
                .and_then(|s| {
                    s.split(',')
                        .map(str::parse::<u32>)
                        .collect::<Result<Vec<_>, _>>()
                        .ok()
                })
                .ok_or("GPU placement requires an explicit recipe device count")?;
            if selected.is_empty()
                || selected.len() != original.len()
                || selected.len() > 64
                || selected.iter().any(|i| *i >= 64)
                || selected
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != selected.len()
            {
                return Err("select distinct GPUs matching the recipe's device count".into());
            }
            let csv = selected
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            model["gpus"] = json!(format!("device={csv}"));
            if let Some(identities) = &p.gpu_uuids {
                if identities.len() != selected.len()
                    || identities.iter().any(|id| {
                        !id.strip_prefix("GPU-")
                            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
                    })
                    || identities
                        .iter()
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        != identities.len()
                {
                    return Err(
                        "GPU placement identities must uniquely match selected devices".into(),
                    );
                }
                if model.get("environment").is_none() {
                    model["environment"] = json!({});
                }
                let env = model
                    .get_mut("environment")
                    .and_then(Value::as_object_mut)
                    .ok_or("GPU placement requires an environment object")?;
                // Privileged containers can see all host devices despite DeviceRequests;
                // CUDA ordinals and NVIDIA host indices need not match. UUIDs do.
                env.insert("CUDA_VISIBLE_DEVICES".into(), json!(identities.join(",")));
                if env.contains_key("NVIDIA_VISIBLE_DEVICES") {
                    env.insert("NVIDIA_VISIBLE_DEVICES".into(), json!(csv));
                }
            } else if selected != &original {
                // Compatibility for placements persisted before UUID pinning.
                if let Some(env) = model.get_mut("environment").and_then(Value::as_object_mut) {
                    if env.contains_key("NVIDIA_VISIBLE_DEVICES") {
                        env.insert("NVIDIA_VISIBLE_DEVICES".into(), json!(csv));
                    }
                    if env.contains_key("CUDA_VISIBLE_DEVICES") {
                        env.insert(
                            "CUDA_VISIBLE_DEVICES".into(),
                            json!(
                                (0..selected.len())
                                    .map(|i| i.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            ),
                        );
                    }
                }
            }
        }
        fn replace(value: &mut Value, p: &Placement) {
            match value {
                Value::String(s) => {
                    *s = s
                        .replace("{{head_address}}", &p.addresses[0])
                        .replace("{{host_address}}", &p.addresses[p.rank as usize])
                        .replace("{{node_rank}}", &p.rank.to_string())
                        .replace("{{num_nodes}}", &p.addresses.len().to_string());
                }
                Value::Array(items) => {
                    for item in items {
                        replace(item, p)
                    }
                }
                Value::Object(items) => {
                    for item in items.values_mut() {
                        replace(item, p)
                    }
                }
                _ => {}
            }
        }
        replace(&mut model, p);
    } else if serde_json::to_string(&model)
        .unwrap_or_default()
        .contains("{{host_address}}")
        || serde_json::to_string(&model)
            .unwrap_or_default()
            .contains("{{head_address}}")
    {
        return Err("portable recipe needs placement addresses".into());
    }
    Ok(model)
}
fn valid_address(s: &str) -> bool {
    (!s.is_empty()
        && s.len() <= 253
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".:-".contains(&c)))
        && !s.starts_with('-')
        && !s.ends_with('-')
}
fn apply(
    state: &mut dashboard_management::ManagementState,
    body: &Selection,
    group: &str,
    gpu_uuids: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<(String, DesiredDeployment)>, String> {
    if !dashboard_management::valid_id(group)
        || group.len() > 32
        || !dashboard_management::valid_id(&body.recipe_id)
        || !(1..=8).contains(&body.hosts.len())
        || !matches!(body.desired_state.as_str(), "ready" | "unloaded")
    {
        return Err("invalid group selection".into());
    }
    let revision = state
        .recipes
        .get(&body.recipe_id)
        .and_then(|r| r.iter().find(|r| r.digest == body.revision_digest))
        .ok_or("recipe revision does not exist")?;
    if let Some(ranks) = revision
        .model_spec
        .get("distributed")
        .and_then(Value::as_array)
    {
        if ranks.len() != body.hosts.len() {
            return Err(format!("recipe needs {} ordered hosts", ranks.len()));
        }
    } else if revision.model_spec.get("image").is_none() {
        return Err("convert this source before deployment".into());
    }
    let mut hosts = std::collections::HashSet::new();
    let addresses = body
        .hosts
        .iter()
        .map(|h| h.address.clone())
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    for (rank, host) in body.hosts.iter().enumerate() {
        if host.endpoint_id.parse::<iroh::EndpointId>().is_err()
            || !valid_address(&host.address)
            || !hosts.insert(&host.endpoint_id)
        {
            return Err("choose distinct enrolled hosts with valid addresses".into());
        }
        let id = state
            .deployments
            .iter()
            .find(|(_, d)| {
                d.placement
                    .as_ref()
                    .is_some_and(|p| p.group_id == group && p.rank == rank as u32)
            })
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| format!("{group}-r{rank}"));
        let current = state.deployments.get(&id);
        let expected = *body.expected_generations.get(&id).unwrap_or(&0);
        if current.map_or(0, |d| d.generation) != expected {
            return Err(format!("generation changed for rank {rank}"));
        }
        let placement = Placement {
            group_id: group.to_owned(),
            rank: rank as u32,
            addresses: addresses.clone(),
            gpus: host.gpus.clone(),
            gpu_uuids: gpu_uuids.get(&host.endpoint_id).cloned(),
        };
        if let Some(current) = current {
            if current.endpoint_id != host.endpoint_id {
                return Err("unload the old group and create a new placement".into());
            }
            if (current.revision_digest != body.revision_digest
                || current.placement.as_ref() != Some(&placement))
                && (current.desired_state != "unloaded"
                    || current
                        .observed
                        .as_ref()
                        .and_then(|v| v.pointer("/status/phase"))
                        .and_then(Value::as_str)
                        != Some("unloaded"))
            {
                return Err(
                    "all ranks must be observed unloaded before changing the revision, addresses or GPUs"
                        .into(),
                );
            }
        }
        render(&revision.model_spec, Some(&placement))?;
        result.push((
            id,
            DesiredDeployment {
                generation: expected.checked_add(1).ok_or("generation exhausted")?,
                placement: Some(placement),
                endpoint_id: host.endpoint_id.clone(),
                recipe_id: body.recipe_id.clone(),
                revision_digest: body.revision_digest.clone(),
                desired_state: body.desired_state.clone(),
                updated_at_ms: chrono::Utc::now().timestamp_millis(),
                observed_at_ms: None,
                observed: None,
                error: None,
            },
        ));
    }
    if state.deployments.values().any(|d| {
        d.placement
            .as_ref()
            .is_some_and(|p| p.group_id == group && p.rank as usize >= body.hosts.len())
    }) {
        return Err("group rank count cannot change; create a new group".into());
    }
    if state.deployments.len()
        + result
            .iter()
            .filter(|(id, _)| !state.deployments.contains_key(id))
            .count()
        > 64
    {
        return Err("deployment capacity reached".into());
    }
    for (id, deployment) in &result {
        state.deployments.insert(id.clone(), deployment.clone());
    }
    Ok(result)
}
#[utoipa::path(post,path="/dashboard/api/management/deployment-groups",tag="dashboard",request_body=Selection,responses((status=200,body=serde_json::Value),(status=403,description="Admin authorization required"),(status=409,description="Revision or state conflict")))]
pub async fn select(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    Extension(session): Extension<AuthSession>,
    headers: HeaderMap,
    Json(body): Json<Selection>,
) -> Response {
    if let Some(response) = authorize_mesh_mutation(&auth, &session, &headers).await {
        return response;
    }
    let Some(store) = gateway.persistence_store() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "SQL store required");
    };
    let Some(admin) = gateway.mesh_admin() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "mesh controller required");
    };
    if admin.initialize().await.is_err() {
        return err(StatusCode::SERVICE_UNAVAILABLE, "mesh state unavailable");
    }
    let Ok(nodes) = admin.store().list_nodes().await else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "mesh state unavailable");
    };
    if body.hosts.iter().any(|h| {
        !nodes
            .iter()
            .any(|n| n.endpoint_id == h.endpoint_id && n.enabled)
    }) {
        return err(StatusCode::BAD_REQUEST, "select enrolled enabled hosts");
    }
    let group = body
        .group_id
        .clone()
        .unwrap_or_else(|| format!("d-{}", &uuid::Uuid::new_v4().simple().to_string()[..24]));
    for _ in 0..4 {
        let Ok((raw, mut state)) = dashboard_management::read(store.as_ref()).await else {
            return err(StatusCode::SERVICE_UNAVAILABLE, "catalog unavailable");
        };
        let revision = match crate::dashboard_recipe_sizes::pin(
            &mut state,
            &body.recipe_id,
            &body.revision_digest,
            body.size.as_deref(),
        ) {
            Ok(revision) => revision,
            Err(message) => return err(StatusCode::BAD_REQUEST, message),
        };
        let mut resolved_body = body.clone();
        resolved_body.revision_digest = revision.digest;
        resolved_body.size = None;
        let incompatible = state
            .recipes
            .get(&body.recipe_id)
            .and_then(|versions| {
                versions
                    .iter()
                    .find(|r| r.digest == resolved_body.revision_digest)
            })
            .is_some_and(|revision| {
                body.hosts.iter().any(|host| {
                    !dashboard_management::matches_hardware(
                        &revision.model_spec,
                        host.endpoint_id
                            .parse()
                            .ok()
                            .and_then(|endpoint| admin.registry().host_snapshot(endpoint))
                            .as_ref(),
                    )
                })
            });
        if body.desired_state == "ready" && incompatible {
            return err(
                StatusCode::CONFLICT,
                "recipe GPU families do not match selected hosts' detected hardware, or a host observation is unavailable",
            );
        }
        let mut identities = BTreeMap::new();
        for host in &body.hosts {
            let Some(indices) = &host.gpus else {
                continue;
            };
            // Unload must remain possible if a host is offline. Preserve the old mask.
            if body.desired_state == "unloaded" {
                if let Some(ids) = state
                    .deployments
                    .values()
                    .find(|d| {
                        d.endpoint_id == host.endpoint_id
                            && d.placement.as_ref().is_some_and(|p| {
                                p.group_id == group && p.gpus.as_ref() == Some(indices)
                            })
                    })
                    .and_then(|d| d.placement.as_ref())
                    .and_then(|p| p.gpu_uuids.clone())
                {
                    identities.insert(host.endpoint_id.clone(), ids);
                }
                continue;
            }
            let snapshot = host
                .endpoint_id
                .parse()
                .ok()
                .and_then(|e| admin.registry().host_snapshot(e));
            let ids = indices
                .iter()
                .map(|index| {
                    snapshot
                        .as_ref()
                        .and_then(|s| s.gpus.iter().find(|g| g.index == *index))
                        .map(|g| g.uuid.clone())
                })
                .collect::<Option<Vec<_>>>();
            let Some(ids) = ids else {
                return err(StatusCode::CONFLICT, "selected GPU identity is unavailable");
            };
            identities.insert(host.endpoint_id.clone(), ids);
        }
        let result = match apply(&mut state, &resolved_body, &group, &identities) {
            Ok(result) => result,
            Err(message) => return err(StatusCode::CONFLICT, &message),
        };
        for (id, deployment) in &result {
            if body.desired_state == "ready" {
                let rendered = state
                    .recipes
                    .get(&deployment.recipe_id)
                    .and_then(|rs| rs.iter().find(|r| r.digest == deployment.revision_digest))
                    .and_then(|r| render(&r.model_spec, deployment.placement.as_ref()).ok());
                if let Some(indices) = rendered
                    .as_ref()
                    .and_then(|s| s.get("gpus"))
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("device="))
                {
                    let snapshot = deployment
                        .endpoint_id
                        .parse()
                        .ok()
                        .and_then(|e| admin.registry().host_snapshot(e));
                    let recipe = &state.recipes[&deployment.recipe_id]
                        .iter()
                        .find(|r| r.digest == deployment.revision_digest)
                        .unwrap()
                        .model_spec;
                    if indices.split(',').any(|i| {
                        let Some(mut one) = snapshot.clone() else {
                            return true;
                        };
                        one.gpus.retain(|g| Some(g.index) == i.parse::<u32>().ok());
                        one.gpus.is_empty()
                            || !dashboard_management::matches_hardware(recipe, Some(&one))
                    }) {
                        return err(
                            StatusCode::CONFLICT,
                            "selected GPU is unavailable or does not match the recipe's hardware",
                        );
                    }
                }
            }
            if let Err(message) = crate::dashboard_resource_admission::check(
                &state,
                id,
                deployment,
                deployment
                    .endpoint_id
                    .parse()
                    .ok()
                    .and_then(|endpoint| admin.registry().host_snapshot(endpoint))
                    .as_ref(),
            ) {
                return err(StatusCode::CONFLICT, &message);
            }
        }
        match dashboard_management::commit(store.as_ref(), raw.as_deref(), &state).await {
            Ok(true) => {
                return json_no_store(
                    StatusCode::ACCEPTED,
                    &json!({"group_id":group,"deployments":result.into_iter().collect::<BTreeMap<_,_>>()}),
                );
            }
            Ok(false) => continue,
            Err(_) => return err(StatusCode::SERVICE_UNAVAILABLE, "could not persist group"),
        }
    }
    err(StatusCode::CONFLICT, "catalog changed; retry")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_placement_round_trips_and_renders_each_rank_independently() {
        let input = json!({"endpoint_id":"host","address":"host","gpus":[4,5]});
        let host: Host = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(host).unwrap(), input);
        let spec = json!({"distributed":[{"gpus":"device=0,1","environment":{"NVIDIA_VISIBLE_DEVICES":"all","CUDA_VISIBLE_DEVICES":"0,1","OTHER":"unchanged"}},{"gpus":"device=0,1"}]});
        let p: Placement = serde_json::from_value(
            json!({"group_id":"group","rank":0,"addresses":["one","two"],"gpus":[4,5]}),
        )
        .unwrap();
        let restored: Placement =
            serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(p, restored);
        let rendered = render(&spec, Some(&p)).unwrap();
        assert_eq!(rendered["gpus"], "device=4,5");
        assert_eq!(rendered["environment"]["NVIDIA_VISIBLE_DEVICES"], "4,5");
        assert_eq!(rendered["environment"]["CUDA_VISIBLE_DEVICES"], "0,1");
        assert_eq!(rendered["environment"]["OTHER"], "unchanged");
        let mut worker = p.clone();
        worker.rank = 1;
        worker.gpus = Some(vec![2, 3]);
        assert_eq!(render(&spec, Some(&worker)).unwrap()["gpus"], "device=2,3");
        assert_eq!(spec["distributed"][0]["gpus"], "device=0,1");
        for devices in [vec![], vec![0], vec![0, 0], vec![64, 65]] {
            let mut invalid = p.clone();
            invalid.gpus = Some(devices);
            assert!(render(&spec, Some(&invalid)).is_err());
        }
        let old: Placement =
            serde_json::from_value(json!({"group_id":"group","rank":0,"addresses":["one","two"]}))
                .unwrap();
        assert!(old.gpus.is_none());
        assert_eq!(
            render(&spec, Some(&old)).unwrap()["environment"]["NVIDIA_VISIBLE_DEVICES"],
            "all"
        );
    }
    #[test]
    fn uuid_masks_select_physical_devices_even_for_privileged_recipes() {
        let ids = vec![
            "GPU-11111111-1111-1111-1111-111111111111",
            "GPU-22222222-2222-2222-2222-222222222222",
        ];
        let input =
            json!({"group_id":"g","rank":0,"addresses":["one"],"gpus":[3,7],"gpu_uuids":ids});
        let placement: Placement = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(&placement).unwrap(), input);
        let spec = json!({"gpus":"device=0,1","privileged":true,"environment":{"CUDA_VISIBLE_DEVICES":"0,1","NVIDIA_VISIBLE_DEVICES":"all"}});
        let rendered = render(&spec, Some(&placement)).unwrap();
        assert_eq!(rendered["gpus"], "device=3,7");
        assert_eq!(
            rendered["environment"]["CUDA_VISIBLE_DEVICES"],
            ids.join(",")
        );
        assert_eq!(rendered["environment"]["NVIDIA_VISIBLE_DEVICES"], "3,7");
        let mut same_indices = placement.clone();
        same_indices.gpus = Some(vec![0, 1]);
        assert_eq!(
            render(&spec, Some(&same_indices)).unwrap()["environment"]["CUDA_VISIBLE_DEVICES"],
            ids.join(",")
        );
        assert_eq!(
            render(&json!({"gpus":"device=0,1"}), Some(&placement)).unwrap()["environment"]["CUDA_VISIBLE_DEVICES"],
            ids.join(",")
        );
        for invalid in [vec![], vec![ids[0]], vec![ids[0], ids[0]], vec!["0", "1"]] {
            let mut p = placement.clone();
            p.gpu_uuids = Some(invalid.into_iter().map(str::to_owned).collect());
            assert!(render(&spec, Some(&p)).is_err());
        }
    }
    #[test]
    fn distributed_render_preserves_rank_specs_and_binds_addresses() {
        let spec = json!({"distributed":[{"command":["--master-addr","{{head_address}}","--host","{{host_address}}"]},{"command":["--node-rank","{{node_rank}}","--nnodes","{{num_nodes}}"],"environment":{"VLLM_HOST_IP":"{{host_address}}"}}]});
        let p = Placement {
            group_id: "group".into(),
            rank: 1,
            addresses: vec!["192.168.37.107".into(), "192.168.37.108".into()],
            gpus: None,
            gpu_uuids: None,
        };
        let rendered = render(&spec, Some(&p)).unwrap();
        assert_eq!(
            rendered["command"],
            json!(["--node-rank", "1", "--nnodes", "2"])
        );
        assert_eq!(rendered["environment"]["VLLM_HOST_IP"], "192.168.37.108");
        assert!(render(&spec, None).is_err());
        assert!(!valid_address("host; curl secret"));
    }
    #[test]
    fn batch_conflicts_leave_all_ranks_untouched() {
        let endpoint = iroh::SecretKey::generate().public().to_string();
        let mut state = dashboard_management::ManagementState::default();
        state.recipes.insert(
            "recipe".into(),
            vec![dashboard_management::RecipeRevision {
                digest: "a".repeat(64),
                revision: 1,
                description: "test".into(),
                model_spec: json!({"image":"pinned"}),
                created_at_ms: 1,
            }],
        );
        let body = Selection {
            size: None,
            group_id: Some("group".into()),
            recipe_id: "recipe".into(),
            revision_digest: "a".repeat(64),
            hosts: vec![
                Host {
                    endpoint_id: endpoint.clone(),
                    address: "127.0.0.1".into(),
                    gpus: None,
                },
                Host {
                    endpoint_id: endpoint,
                    address: "127.0.0.2".into(),
                    gpus: None,
                },
            ],
            expected_generations: BTreeMap::new(),
            desired_state: "ready".into(),
        };
        assert!(apply(&mut state, &body, "group", &BTreeMap::new()).is_err());
        assert!(state.deployments.is_empty());
    }
}

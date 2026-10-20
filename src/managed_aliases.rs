//! Durable client-facing aliases. SQL stores profile references, never backend credentials.
use crate::{
    config::{CapabilitiesConfig, Config},
    control_plane::{OperationalProviderPlan, OperationalRoutePlan},
    control_plane_store::PersistenceStore,
    error::{AppError, AppResult},
    managed_providers::ManagedProviderOptions,
    replay::ReplayStore,
    upstream::*,
};
use async_trait::async_trait;
use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode, header},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use uuid::Uuid;

fn unavailable(message: &str) -> AppError {
    AppError::traffic(false, message)
}

pub const STORE_KEY: &str = "model_aliases";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct AliasMapping {
    #[schema(value_type=String, format="uuid")]
    pub id: Uuid,
    pub name: String,
    #[schema(value_type=Vec<String>)]
    pub profiles: Vec<Uuid>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct StoredAliases {
    revision: u64,
    aliases: Vec<AliasMapping>,
    #[serde(default)]
    retired_names: BTreeSet<String>,
}
#[derive(Clone, Serialize)]
pub struct AliasTarget {
    pub id: Uuid,
    pub name: String,
    pub upstream_models: Vec<String>,
    pub backends: Vec<String>,
    pub available: bool,
}
#[derive(Serialize)]
pub struct AliasView {
    #[serde(flatten)]
    pub mapping: AliasMapping,
    pub error: Option<String>,
}
#[derive(Serialize)]
pub struct AliasesView {
    pub revision: u64,
    pub aliases: Vec<AliasView>,
    pub targets: Vec<AliasTarget>,
    pub persistent: bool,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveAliases {
    pub revision: u64,
    pub aliases: Vec<AliasMapping>,
}
#[derive(Clone)]
struct Target {
    view: AliasTarget,
    providers: Vec<OperationalProviderPlan>,
    capabilities: Option<CapabilitiesConfig>,
}
/// Shared across all requests at one revision. Old requests retain this Arc through streaming.
pub struct AliasSnapshot {
    stored: StoredAliases,
    routes: BTreeMap<String, DynUpstreamClient>,
    profiles: BTreeMap<String, String>,
    errors: BTreeMap<Uuid, String>,
    pub replay: ReplayStore,
}
impl AliasSnapshot {
    pub fn profiles(&self) -> &BTreeMap<String, String> {
        &self.profiles
    }
    pub fn is_retired(&self, model: &str) -> bool {
        self.stored
            .retired_names
            .contains(&model.trim().to_ascii_lowercase())
    }
    fn route(&self, model: &str) -> AppResult<Option<DynUpstreamClient>> {
        if let Some(alias) = self
            .stored
            .aliases
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(model.trim()))
        {
            return self
                .routes
                .get(&alias.name)
                .cloned()
                .map(Some)
                .ok_or_else(|| {
                    unavailable("alias target is unavailable; review the alias configuration")
                });
        }
        if self.is_retired(model) {
            return Err(AppError::not_found(
                "requested alias is no longer configured",
            ));
        }
        Ok(None)
    }
}
struct RegistryState {
    loaded: bool,
    snapshot: Arc<AliasSnapshot>,
}
pub struct AliasRegistry {
    store: Option<Arc<dyn PersistenceStore>>,
    targets: BTreeMap<Uuid, Target>,
    catalog_config: Config,
    reserved: BTreeSet<String>,
    options: ManagedProviderOptions,
    cooldown: Duration,
    max_replay_entries: usize,
    state: Mutex<RegistryState>,
}
impl AliasRegistry {
    pub fn new(
        store: Option<Arc<dyn PersistenceStore>>,
        routes: &[OperationalRoutePlan],
        config: &Config,
        reserved: BTreeSet<String>,
        options: ManagedProviderOptions,
        cooldown: Duration,
        max_replay_entries: usize,
    ) -> Result<Arc<Self>, String> {
        let mut targets: BTreeMap<Uuid, Target> = BTreeMap::new();
        for route in routes {
            targets
                .entry(route.primary_profile_id)
                .or_insert_with(|| Target {
                    view: AliasTarget {
                        id: route.primary_profile_id,
                        name: route.primary_profile_name.clone(),
                        upstream_models: Vec::new(),
                        backends: Vec::new(),
                        available: false,
                    },
                    providers: Vec::new(),
                    capabilities: config
                        .model_profiles
                        .get(&route.primary_profile_name)
                        .and_then(|profile| profile.capabilities.clone()),
                });
            for p in &route.providers {
                let target = targets.entry(p.profile_id).or_insert_with(|| Target {
                    view: AliasTarget {
                        id: p.profile_id,
                        name: p.profile_name.clone(),
                        upstream_models: Vec::new(),
                        backends: Vec::new(),
                        available: true,
                    },
                    providers: Vec::new(),
                    capabilities: config
                        .model_profiles
                        .get(&p.profile_name)
                        .and_then(|profile| profile.capabilities.clone()),
                });
                if !target
                    .providers
                    .iter()
                    .any(|old| old.backend_id == p.backend_id)
                {
                    target.providers.push(p.clone());
                }
            }
        }
        for target in targets.values_mut() {
            target.view.available = !target.providers.is_empty();
            target.view.upstream_models = target
                .providers
                .iter()
                .map(|p| {
                    p.upstream_model
                        .clone()
                        .unwrap_or_else(|| target.view.name.clone())
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            target.view.backends = target
                .providers
                .iter()
                .map(|p| p.backend_name.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        let aliases = routes
            .iter()
            .filter(|r| r.route_id != r.primary_profile_id)
            .map(|r| AliasMapping {
                id: r.route_id,
                name: r.name.clone(),
                profiles: r.profile_ids.clone(),
            })
            .collect();
        let stored = StoredAliases {
            aliases,
            ..Default::default()
        };
        let empty = Arc::new(AliasSnapshot {
            stored: StoredAliases::default(),
            routes: BTreeMap::new(),
            profiles: BTreeMap::new(),
            errors: BTreeMap::new(),
            replay: ReplayStore::new(max_replay_entries),
        });
        let mut registry = Self {
            store,
            catalog_config: config.clone(),
            targets,
            reserved,
            options,
            cooldown,
            max_replay_entries,
            state: Mutex::new(RegistryState {
                loaded: false,
                snapshot: empty,
            }),
        };
        registry.state.get_mut().snapshot = Arc::new(registry.compile(stored)?);
        registry.state.get_mut().loaded = registry.store.is_none();
        Ok(Arc::new(registry))
    }
    fn compile(&self, stored: StoredAliases) -> Result<AliasSnapshot, String> {
        let mut snapshot = AliasSnapshot {
            stored,
            routes: BTreeMap::new(),
            profiles: BTreeMap::new(),
            errors: BTreeMap::new(),
            replay: ReplayStore::new(self.max_replay_entries),
        };
        for alias in &snapshot.stored.aliases {
            let primary = alias.profiles.first().and_then(|id| self.targets.get(id));
            snapshot.profiles.insert(
                alias.name.clone(),
                primary
                    .map(|p| p.view.name.clone())
                    .unwrap_or_else(|| alias.name.clone()),
            );
            if alias.profiles.is_empty()
                || alias
                    .profiles
                    .iter()
                    .any(|id| !self.targets.contains_key(id))
            {
                snapshot.errors.insert(
                    alias.id,
                    "A target profile was removed. Select a current model to repair this alias."
                        .into(),
                );
                continue;
            }
            let providers: Vec<_> = alias
                .profiles
                .iter()
                .flat_map(|id| self.targets[id].providers.iter())
                .collect();
            if providers.is_empty() {
                snapshot
                    .errors
                    .insert(alias.id, "No enabled backend for these targets.".into());
                continue;
            }
            let legs = providers
                .into_iter()
                .map(|p| {
                    FailoverUpstreamProvider::new(
                        p.backend_name.clone(),
                        ReqwestUpstreamClient::with_options(
                            self.options.http_client.clone(),
                            p.base_url.clone(),
                            p.api_key.clone(),
                            p.request_log_path.clone(),
                            self.options.flatten_content,
                            self.options.min_completion_tokens,
                            self.options.max_sse_frame_bytes,
                        )
                        .with_finalization_policies(
                            self.options
                                .finalization_policies
                                .for_profile(&p.profile_name, p.upstream_model.as_deref()),
                        )
                        .with_flow_store(self.options.flow_store.clone())
                        .with_traffic(self.options.traffic.clone()),
                        p.upstream_model.clone(),
                        None,
                        p.upstream_chat_kwargs.clone(),
                    )
                })
                .collect();
            let provider = RouteUpstreamProvider::from_failover(
                format!("alias-{}", alias.name),
                legs,
                self.cooldown,
            )?;
            snapshot.routes.insert(
                alias.name.clone(),
                Arc::new(
                    RoutingUpstreamClient::with_routes(
                        Vec::new(),
                        vec![provider],
                        vec![ModelRouteSpec::advertised_exact(alias.name.clone(), 0)],
                    )
                    .with_traffic(self.options.traffic.clone()),
                ),
            );
        }
        Ok(snapshot)
    }
    async fn load_locked(&self, state: &mut RegistryState) -> AppResult<()> {
        if state.loaded {
            return Ok(());
        }
        let Some(store) = &self.store else {
            return Ok(());
        };
        let mut raw = store
            .get_setting(STORE_KEY)
            .await
            .map_err(|_| unavailable("alias storage unavailable"))?;
        if raw.is_none() {
            let seed = serde_json::to_string(&state.snapshot.stored)
                .map_err(|e| AppError::internal(e.to_string()))?;
            if store
                .compare_exchange_setting(STORE_KEY, None, &seed)
                .await
                .map_err(|_| unavailable("alias storage unavailable"))?
            {
                raw = Some(seed);
            } else {
                raw = store
                    .get_setting(STORE_KEY)
                    .await
                    .map_err(|_| unavailable("alias storage unavailable"))?;
            }
        }
        let stored: StoredAliases = serde_json::from_str(
            raw.as_deref()
                .ok_or_else(|| unavailable("alias storage unavailable"))?,
        )
        .map_err(|_| unavailable("stored aliases are invalid"))?;
        self.validate(&stored.aliases, Some(&stored.aliases))
            .map_err(|_| unavailable("stored aliases are invalid"))?;
        if stored.retired_names.len() > 1024 {
            return Err(unavailable("stored aliases are invalid"));
        }
        state.snapshot = Arc::new(
            self.compile(stored)
                .map_err(|e| AppError::internal(e.to_string()))?,
        );
        state.loaded = true;
        Ok(())
    }
    pub async fn snapshot(&self) -> AppResult<Arc<AliasSnapshot>> {
        let mut state = self.state.lock().await;
        self.load_locked(&mut state).await?;
        Ok(state.snapshot.clone())
    }
    pub async fn view(&self) -> AppResult<AliasesView> {
        let snapshot = self.snapshot().await?;
        Ok(self.view_of(&snapshot))
    }
    fn view_of(&self, snapshot: &AliasSnapshot) -> AliasesView {
        let mut targets: Vec<_> = self.targets.values().map(|t| t.view.clone()).collect();
        targets.sort_by_key(|a| a.name.to_lowercase());
        let mut aliases: Vec<_> = snapshot
            .stored
            .aliases
            .iter()
            .map(|a| AliasView {
                mapping: a.clone(),
                error: snapshot.errors.get(&a.id).cloned(),
            })
            .collect();
        aliases.sort_by(|a, b| a.mapping.name.cmp(&b.mapping.name));
        AliasesView {
            revision: snapshot.stored.revision,
            aliases,
            targets,
            persistent: self.store.is_some(),
        }
    }
    fn validate(
        &self,
        aliases: &[AliasMapping],
        previous: Option<&[AliasMapping]>,
    ) -> Result<(), String> {
        if aliases.len() > 128 {
            return Err("At most 128 aliases are supported.".into());
        }
        let mut names = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for alias in aliases {
            let name = alias.name.trim();
            if name.is_empty()
                || name != alias.name
                || name.len() > 128
                || name.chars().any(|c| c.is_control() || c.is_whitespace())
            {
                return Err("Alias names must be 1–128 characters without whitespace.".into());
            }
            if alias.id.is_nil() || !ids.insert(alias.id) {
                return Err("Alias IDs must be unique.".into());
            }
            let key = name.to_ascii_lowercase();
            if !names.insert(key.clone())
                || self.reserved.contains(&key)
                || self
                    .targets
                    .values()
                    .any(|t| t.view.name.eq_ignore_ascii_case(name))
            {
                return Err("Alias name conflicts with an existing model or alias.".into());
            }
            if alias.profiles.is_empty()
                || alias.profiles.len() > 16
                || alias.profiles.iter().collect::<BTreeSet<_>>().len() != alias.profiles.len()
            {
                return Err("Select 1–16 distinct target models in fallback order.".into());
            }
            if alias
                .profiles
                .iter()
                .any(|id| !self.targets.contains_key(id))
                && !previous.is_some_and(|old| {
                    old.iter()
                        .any(|a| a.id == alias.id && a.profiles == alias.profiles)
                })
            {
                return Err("One of the target profiles is unavailable.".into());
            }
        }
        Ok(())
    }
    pub async fn save(&self, body: SaveAliases) -> AppResult<AliasesView> {
        let Some(store) = &self.store else {
            return Err(unavailable("SQL persistence is required to edit aliases"));
        };
        let mut state = self.state.lock().await;
        self.load_locked(&mut state).await?;
        let raw = store
            .get_setting(STORE_KEY)
            .await
            .map_err(|_| unavailable("alias storage unavailable"))?;
        let old: StoredAliases = serde_json::from_str(raw.as_deref().unwrap_or_default())
            .map_err(|_| unavailable("stored aliases are invalid"))?;
        if body.revision != old.revision {
            return Err(AppError::conflict("Aliases changed; reload before saving."));
        }
        self.validate(&body.aliases, Some(&old.aliases))
            .map_err(AppError::bad_request)?;
        let mut retired = old.retired_names;
        for alias in &old.aliases {
            if !body
                .aliases
                .iter()
                .any(|a| a.name.eq_ignore_ascii_case(&alias.name))
            {
                retired.insert(alias.name.to_ascii_lowercase());
            }
        }
        for alias in &body.aliases {
            retired.remove(&alias.name.to_ascii_lowercase());
        }
        if retired.len() > 1024 {
            return Err(AppError::bad_request("Alias retirement history is full."));
        }
        let next = StoredAliases {
            revision: old
                .revision
                .checked_add(1)
                .ok_or_else(|| AppError::internal("alias revision overflow"))?,
            aliases: body.aliases,
            retired_names: retired,
        };
        let snapshot = Arc::new(
            self.compile(next.clone())
                .map_err(|e| AppError::internal(e.to_string()))?,
        );
        let encoded =
            serde_json::to_string(&next).map_err(|e| AppError::internal(e.to_string()))?;
        if !store
            .compare_exchange_setting(STORE_KEY, raw.as_deref(), &encoded)
            .await
            .map_err(|_| unavailable("alias storage unavailable"))?
        {
            return Err(AppError::conflict("Aliases changed; reload before saving."));
        }
        state.snapshot = snapshot;
        Ok(self.view_of(&state.snapshot))
    }
}
/// The live wrapper is used for inventory; inference receives a frozen wrapper in Gateway::request_snapshot.
pub struct AliasUpstream {
    base: DynUpstreamClient,
    registry: Arc<AliasRegistry>,
    frozen: Option<Arc<AliasSnapshot>>,
}

/// A client may use any enabled target. Unknown or conflicting declarations
/// therefore cannot imply support; one explicit refusal is enough to refuse.
fn shared_capabilities(targets: &[Value]) -> Option<Value> {
    if targets.is_empty() {
        return None;
    }
    let keys = targets
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|object| object.keys())
        .collect::<BTreeSet<_>>();
    let mut common = serde_json::Map::new();
    for key in keys {
        let first = targets[0].get(key);
        if let Some(value) = first
            && targets.iter().all(|target| target.get(key) == Some(value))
        {
            common.insert(key.clone(), value.clone());
        } else if targets.iter().any(|target| {
            target
                .get(key)
                .and_then(|cap| cap.get("supported"))
                .and_then(Value::as_bool)
                == Some(false)
        }) {
            common.insert(key.clone(), json!({"supported":false}));
        }
    }
    (!common.is_empty()).then_some(Value::Object(common))
}

impl AliasUpstream {
    pub fn new(base: DynUpstreamClient, registry: Arc<AliasRegistry>) -> Self {
        Self {
            base,
            registry,
            frozen: None,
        }
    }
    pub fn frozen(
        base: DynUpstreamClient,
        registry: Arc<AliasRegistry>,
        snapshot: Arc<AliasSnapshot>,
    ) -> Self {
        Self {
            base,
            registry,
            frozen: Some(snapshot),
        }
    }
    async fn snapshot(&self) -> AppResult<Arc<AliasSnapshot>> {
        match &self.frozen {
            Some(s) => Ok(s.clone()),
            None => self.registry.snapshot().await,
        }
    }
    async fn client(&self, model: &str) -> AppResult<DynUpstreamClient> {
        Ok(self
            .snapshot()
            .await?
            .route(model)?
            .unwrap_or_else(|| self.base.clone()))
    }
}
#[async_trait]
impl UpstreamClient for AliasUpstream {
    async fn stream_chat_completion(&self, r: &BackendChatRequest) -> AppResult<UpstreamStream> {
        self.client(&r.request.model)
            .await?
            .stream_chat_completion(r)
            .await
    }
    async fn stream_chat_completion_with_timeout(
        &self,
        r: &BackendChatRequest,
        t: Duration,
    ) -> AppResult<UpstreamStream> {
        self.client(&r.request.model)
            .await?
            .stream_chat_completion_with_timeout(r, t)
            .await
    }
    async fn proxy_completions(&self, r: ProxyCompletionsRequest) -> AppResult<reqwest::Response> {
        self.client(&r.model().unwrap_or_default())
            .await?
            .proxy_completions(r)
            .await
    }
    async fn count_tokens(&self, r: &BackendChatRequest) -> AppResult<Option<u64>> {
        self.client(&r.request.model).await?.count_tokens(r).await
    }
    async fn backend_candidate_plan(&self, m: &str) -> BackendCandidatePlan {
        match self.client(m).await {
            Ok(c) => c.backend_candidate_plan(m).await,
            Err(_) => BackendCandidatePlan {
                candidates: Vec::new(),
            },
        }
    }
    async fn supported_model_catalog(&self) -> AppResult<Vec<UpstreamModelEntry>> {
        let s = self.snapshot().await?;
        let mut catalog = self
            .base
            .supported_model_catalog()
            .await
            .unwrap_or_default();
        catalog.retain(|m| {
            !s.is_retired(&m.id)
                && !s
                    .stored
                    .aliases
                    .iter()
                    .any(|a| a.name.eq_ignore_ascii_case(&m.id))
        });
        for entry in &mut catalog {
            if let Some(configured) = self
                .registry
                .catalog_config
                .resolve_capabilities_for_upstream(&entry.id)
            {
                entry.capabilities = Some(
                    configured.merge_into(entry.capabilities.take().unwrap_or_else(|| json!({}))),
                );
            }
        }
        // Profile routes already advertise conservative backend context limits. Reuse their
        // catalogs rather than refetching every backend once for each alias on every request.
        let aliases = s
            .stored
            .aliases
            .iter()
            .filter(|a| s.routes.contains_key(&a.name))
            .map(|a| {
                let context_limit = a
                    .profiles
                    .iter()
                    .filter_map(|id| self.registry.targets.get(id))
                    .filter_map(|t| {
                        catalog
                            .iter()
                            .find(|m| m.id == t.view.name)
                            .and_then(|m| m.context_limit)
                    })
                    .min();
                let target_capabilities = a
                    .profiles
                    .iter()
                    .filter_map(|id| self.registry.targets.get(id))
                    .filter(|target| !target.providers.is_empty())
                    .map(|target| {
                        // Union model IDs do not retain backend ownership. Taking
                        // another backend's same-ID metadata could incorrectly
                        // certify an unknown alias target. Profile declarations
                        // are the authoritative contract for these route chains.
                        target
                            .capabilities
                            .as_ref()
                            .map_or_else(|| json!({}), |caps| caps.merge_into(json!({})))
                    })
                    .collect::<Vec<_>>();
                UpstreamModelEntry {
                    capabilities: shared_capabilities(&target_capabilities),
                    id: a.name.clone(),
                    context_limit,
                }
            })
            .collect::<Vec<_>>();
        catalog.extend(aliases);
        Ok(catalog)
    }
    async fn list_models(&self) -> AppResult<UpstreamModelsResponse> {
        let data = self.supported_model_catalog().await?.into_iter().map(|model| {
            let mut entry = json!({"id":model.id,"object":"model","owned_by":"llmconduit","context_window":model.context_limit});
            if let Some(capabilities) = model.capabilities {
                entry["capabilities"] = capabilities;
            }
            entry
        }).collect::<Vec<_>>();
        let body = serde_json::to_vec(&json!({"object":"list","data":data}))
            .map_err(|error| AppError::internal(error.to_string()))?;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        headers.insert(header::ETAG, catalogue_etag(&body));
        Ok(UpstreamModelsResponse {
            status: StatusCode::OK,
            headers,
            body: Bytes::from(body),
        })
    }
    async fn provider_inventory(&self) -> AppResult<Vec<ProviderInventoryEntry>> {
        let s = self.snapshot().await?;
        let mut result = self.base.provider_inventory().await?;
        for c in s.routes.values() {
            result.extend(c.provider_inventory().await?);
        }
        Ok(result)
    }
    fn provider_health(&self) -> Vec<ProviderHealth> {
        let mut result = self.base.provider_health();
        if let Some(s) = &self.frozen {
            for c in s.routes.values() {
                result.extend(c.provider_health());
            }
        } else if let Ok(state) = self.registry.state.try_lock() {
            for c in state.snapshot.routes.values() {
                result.extend(c.provider_health());
            }
        }
        result
    }
    fn provider_base_url(&self) -> String {
        self.base.provider_base_url()
    }
    fn model_catalog_cache_ttl(&self) -> Duration {
        Duration::ZERO
    }
}

#[cfg(test)]
mod catalogue_tests {
    use super::*;

    #[test]
    fn catalogue_capabilities_require_explicit_consensus_and_preserve_refusal() {
        let yes = json!({"video_generation":{"supported":true}});
        let no = json!({"video_generation":{"supported":false}});
        let unknown = json!({});
        assert_eq!(shared_capabilities(&[]), None);
        assert_eq!(shared_capabilities(std::slice::from_ref(&unknown)), None);
        assert_eq!(shared_capabilities(&[yes.clone(), unknown.clone()]), None);
        assert_eq!(
            shared_capabilities(&[yes.clone(), yes.clone()]),
            Some(yes.clone())
        );
        assert_eq!(shared_capabilities(&[yes, no.clone()]), Some(no.clone()));
        assert_eq!(shared_capabilities(&[unknown, no.clone()]), Some(no));
    }
}

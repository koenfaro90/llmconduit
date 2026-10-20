use crate::control_plane_store::PersistenceStore;
use crate::dashboard_flow::DashboardFlowStore;
use crate::error::{AppError, AppResult};
use crate::upstream::{
    BackendCandidate, BackendCandidatePlan, BackendChatRequest, DynUpstreamClient,
    InferenceEndpoint, ProviderInventoryEntry, ProxyCompletionsRequest, ReqwestUpstreamClient,
    UpstreamClient, UpstreamModelEntry, UpstreamModelsResponse,
};
use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, header};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use utoipa::ToSchema;
use uuid::Uuid;

fn default_enabled() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateConfiguredProviderRequest {
    pub revision: u64,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub api_key: Option<String>,
}

const STORE_KEY: &str = "configured_providers";
const MAX_CONFIGURED_PROVIDERS: usize = 32;
const MAX_PROVIDER_MODELS: usize = 512;
const MODEL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct ManagedProviderOptions {
    pub traffic: crate::traffic::TrafficController,
    pub http_client: reqwest::Client,
    pub flatten_content: bool,
    pub min_completion_tokens: i64,
    pub max_sse_frame_bytes: usize,
    pub finalization_policies: crate::upstream::BackendFinalizationPolicies,
    pub flow_store: DashboardFlowStore,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfiguredProvidersBody {
    pub providers: Vec<ConfiguredProviderView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfiguredProviderView {
    pub revision: u64,
    pub enabled: bool,
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key_present: bool,
    pub models: Vec<UpstreamModelEntry>,
}

#[derive(Clone, Deserialize, ToSchema)]
pub struct CreateConfiguredProviderRequest {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredProviders {
    providers: Vec<StoredProvider>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredProvider {
    #[serde(default)]
    revision: u64,
    #[serde(default = "default_enabled")]
    enabled: bool,
    id: String,
    name: String,
    base_url: String,
    api_key: String,
    models: Vec<UpstreamModelEntry>,
}

#[derive(Clone)]
struct ManagedProvider {
    stored: StoredProvider,
    client: DynUpstreamClient,
}

#[derive(Default)]
struct ManagedProviderState {
    loaded: bool,
    providers: Vec<ManagedProvider>,
}

pub struct ManagedProviderRegistry {
    store: Arc<dyn PersistenceStore>,
    options: ManagedProviderOptions,
    state: RwLock<ManagedProviderState>,
}

impl ManagedProviderRegistry {
    pub fn new(store: Arc<dyn PersistenceStore>, options: ManagedProviderOptions) -> Arc<Self> {
        let registry = Arc::new(Self {
            store,
            options,
            state: RwLock::new(ManagedProviderState::default()),
        });
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let registry = Arc::clone(&registry);
            handle.spawn(async move {
                if let Err(err) = registry.ensure_loaded().await {
                    tracing::warn!(error = %err, "failed to load configured providers");
                }
            });
        }
        registry
    }

    pub async fn list(&self) -> AppResult<Vec<ConfiguredProviderView>> {
        self.ensure_loaded().await?;
        Ok(self
            .state
            .read()
            .await
            .providers
            .iter()
            .map(provider_view)
            .collect())
    }

    pub async fn add(
        &self,
        request: CreateConfiguredProviderRequest,
    ) -> AppResult<ConfiguredProviderView> {
        self.ensure_loaded().await?;
        let name = validate_provider_name(&request.name)?;
        let base_url = normalize_provider_url(&request.base_url)?;
        let api_key = request.api_key.trim().to_string();
        if api_key.is_empty() {
            return Err(AppError::bad_request("api_key is required"));
        }
        let client = self.make_client(base_url.clone(), Some(api_key.clone()));
        let mut models =
            tokio::time::timeout(MODEL_DISCOVERY_TIMEOUT, client.supported_model_catalog())
                .await
                .map_err(|_| AppError::upstream("provider model discovery timed out"))??;
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models.dedup_by(|a, b| a.id == b.id);
        if models.is_empty() {
            return Err(AppError::bad_request(
                "provider returned no models from /v1/models",
            ));
        }
        if models.len() > MAX_PROVIDER_MODELS {
            return Err(AppError::bad_request(format!(
                "provider returned too many models; maximum is {MAX_PROVIDER_MODELS}"
            )));
        }

        let mut state = self.state.write().await;
        if state.providers.len() >= MAX_CONFIGURED_PROVIDERS {
            return Err(AppError::bad_request(format!(
                "too many configured providers; maximum is {MAX_CONFIGURED_PROVIDERS}"
            )));
        }
        if state
            .providers
            .iter()
            .any(|provider| provider.stored.name.eq_ignore_ascii_case(&name))
        {
            return Err(AppError::bad_request("provider name already exists"));
        }
        if state
            .providers
            .iter()
            .any(|provider| provider.stored.base_url == base_url.as_str())
        {
            return Err(AppError::bad_request("provider URL already exists"));
        }
        let stored = StoredProvider {
            revision: 1,
            enabled: true,
            id: format!("cfg_{}", Uuid::new_v4().simple()),
            name,
            base_url: base_url.to_string(),
            api_key,
            models,
        };
        state.providers.push(ManagedProvider {
            stored,
            client: Arc::new(client),
        });
        match self.persist_locked(&state).await {
            Ok(()) => {
                let provider = state
                    .providers
                    .last()
                    .map(provider_view)
                    .expect("provider was just inserted");
                Ok(provider)
            }
            Err(err) => {
                state.providers.pop();
                Err(err)
            }
        }
    }

    pub async fn update(
        &self,
        id: &str,
        request: UpdateConfiguredProviderRequest,
    ) -> AppResult<ConfiguredProviderView> {
        self.ensure_loaded().await?;
        let old = self
            .state
            .read()
            .await
            .providers
            .iter()
            .find(|p| p.stored.id == id)
            .cloned()
            .ok_or_else(|| AppError::not_found("Configured provider not found"))?;
        if request.revision != old.stored.revision {
            return Err(AppError::conflict(
                "Provider changed; reload before editing",
            ));
        }
        let name = validate_provider_name(&request.name)?;
        let base_url = normalize_provider_url(&request.base_url)?;
        let api_key = request
            .api_key
            .map(|v| v.trim().to_owned())
            .unwrap_or_else(|| old.stored.api_key.clone());
        let client = self.make_client(
            base_url.clone(),
            (!api_key.is_empty()).then_some(api_key.clone()),
        );
        let mut stored = old.stored.clone();
        stored.name = name;
        stored.base_url = base_url.to_string();
        stored.api_key = api_key;
        stored.enabled = request.enabled;
        if stored.enabled
            && (!old.stored.enabled
                || stored.base_url != old.stored.base_url
                || stored.api_key != old.stored.api_key)
        {
            let mut models =
                tokio::time::timeout(MODEL_DISCOVERY_TIMEOUT, client.supported_model_catalog())
                    .await
                    .map_err(|_| AppError::upstream("Provider discovery timed out"))??;
            models.sort_by(|a, b| a.id.cmp(&b.id));
            models.dedup_by(|a, b| a.id == b.id);
            if models.is_empty() || models.len() > MAX_PROVIDER_MODELS {
                return Err(AppError::bad_request(
                    "Provider must advertise 1–512 models",
                ));
            }
            stored.models = models;
        }
        stored.revision = stored
            .revision
            .checked_add(1)
            .ok_or_else(|| AppError::bad_request("Provider revision exhausted"))?;
        let mut state = self.state.write().await;
        if state.providers.iter().any(|p| {
            p.stored.id != id
                && (p.stored.name.eq_ignore_ascii_case(&stored.name)
                    || p.stored.base_url == stored.base_url)
        }) {
            return Err(AppError::bad_request("Provider name or URL already exists"));
        }
        let index = state
            .providers
            .iter()
            .position(|p| p.stored.id == id)
            .ok_or_else(|| AppError::not_found("Configured provider not found"))?;
        if state.providers[index].stored.revision != request.revision {
            return Err(AppError::conflict(
                "Provider changed; reload before editing",
            ));
        }
        let prior = std::mem::replace(
            &mut state.providers[index],
            ManagedProvider {
                stored,
                client: Arc::new(client),
            },
        );
        if let Err(error) = self.persist_locked(&state).await {
            state.providers[index] = prior;
            return Err(error);
        }
        Ok(provider_view(&state.providers[index]))
    }

    pub async fn delete(&self, id: &str) -> AppResult<bool> {
        self.ensure_loaded().await?;
        let mut state = self.state.write().await;
        let Some(index) = state
            .providers
            .iter()
            .position(|provider| provider.stored.id == id)
        else {
            return Ok(false);
        };
        let removed = state.providers.remove(index);
        match self.persist_locked(&state).await {
            Ok(()) => Ok(true),
            Err(err) => {
                state.providers.insert(index, removed);
                Err(err)
            }
        }
    }

    async fn ensure_loaded(&self) -> AppResult<()> {
        if self.state.read().await.loaded {
            return Ok(());
        }
        let mut state = self.state.write().await;
        if state.loaded {
            return Ok(());
        }
        let stored = match self.store.get_setting(STORE_KEY).await {
            Ok(Some(value)) if !value.trim().is_empty() => {
                serde_json::from_str::<StoredProviders>(&value).map_err(|err| {
                    AppError::internal(format!("configured providers JSON is invalid: {err}"))
                })?
            }
            Ok(_) => StoredProviders {
                providers: Vec::new(),
            },
            Err(err) => {
                return Err(AppError::internal(format!(
                    "failed to load configured providers: {err}"
                )));
            }
        };
        let providers = stored
            .providers
            .into_iter()
            .take(MAX_CONFIGURED_PROVIDERS)
            .filter_map(|stored| match Url::parse(&stored.base_url) {
                Ok(base_url) => Some(ManagedProvider {
                    client: Arc::new(self.make_client(base_url, Some(stored.api_key.clone()))),
                    stored,
                }),
                Err(err) => {
                    tracing::warn!(
                        provider_id = %stored.id,
                        error = %err,
                        "skipping configured provider with invalid stored URL"
                    );
                    None
                }
            })
            .collect();
        state.providers = providers;
        state.loaded = true;
        Ok(())
    }

    async fn persist_locked(&self, state: &ManagedProviderState) -> AppResult<()> {
        let stored = StoredProviders {
            providers: state
                .providers
                .iter()
                .map(|provider| provider.stored.clone())
                .collect(),
        };
        let json = serde_json::to_string(&stored).map_err(|err| {
            AppError::internal(format!("failed to serialize configured providers: {err}"))
        })?;
        self.store
            .set_setting(STORE_KEY, &json)
            .await
            .map_err(|err| {
                AppError::internal(format!("failed to persist configured providers: {err}"))
            })
    }

    fn make_client(&self, base_url: Url, api_key: Option<String>) -> ReqwestUpstreamClient {
        ReqwestUpstreamClient::with_options(
            self.options.http_client.clone(),
            base_url,
            api_key,
            None::<PathBuf>,
            self.options.flatten_content,
            self.options.min_completion_tokens,
            self.options.max_sse_frame_bytes,
        )
        .with_finalization_policies(self.options.finalization_policies.clone())
        .with_flow_store(self.options.flow_store.clone())
        .with_traffic(self.options.traffic.clone())
    }

    async fn route_for_model(
        &self,
        model: &str,
        base_models: &[UpstreamModelEntry],
    ) -> AppResult<Option<ManagedProvider>> {
        self.ensure_loaded().await?;
        let mut base_ids = HashSet::new();
        for entry in base_models {
            base_ids.insert(entry.id.to_ascii_lowercase());
        }
        if base_ids.contains(&model.to_ascii_lowercase()) {
            return Ok(None);
        }
        let state = self.state.read().await;
        let mut matches = state
            .providers
            .iter()
            .filter(|provider| {
                provider.stored.enabled
                    && provider
                        .stored
                        .models
                        .iter()
                        .any(|entry| entry.id.eq_ignore_ascii_case(model))
            })
            .cloned()
            .collect::<Vec<_>>();
        if matches.is_empty()
            && state.providers.iter().any(|p| {
                !p.stored.enabled
                    && p.stored
                        .models
                        .iter()
                        .any(|m| m.id.eq_ignore_ascii_case(model))
            })
        {
            return Err(AppError::not_found("Requested provider is disabled"));
        }
        Ok((matches.len() == 1).then(|| matches.remove(0)))
    }

    async fn inventory(&self) -> AppResult<Vec<ProviderInventoryEntry>> {
        self.ensure_loaded().await?;
        Ok(self
            .state
            .read()
            .await
            .providers
            .iter()
            .filter(|provider| provider.stored.enabled)
            .map(|provider| ProviderInventoryEntry {
                provider_id: provider.stored.id.clone(),
                provider_name: provider.stored.name.clone(),
                resource_id: None,
                route: Some("configured".to_string()),
                base_url: provider.stored.base_url.clone(),
                models: provider.stored.models.clone(),
                availability: None,
                capacity_limit: None,
                active_requests: None,
                accepting_requests: true,
                healthy: true,
            })
            .collect())
    }
}

#[derive(Clone)]
struct CachedBaseCatalog {
    fetched_at: Instant,
    catalog: Vec<UpstreamModelEntry>,
}

pub struct ManagedProviderUpstream {
    base: DynUpstreamClient,
    registry: Arc<ManagedProviderRegistry>,
    base_catalog: RwLock<Option<CachedBaseCatalog>>,
}

impl ManagedProviderUpstream {
    pub fn new(base: DynUpstreamClient, registry: Arc<ManagedProviderRegistry>) -> Self {
        Self {
            base,
            registry,
            base_catalog: RwLock::new(None),
        }
    }

    async fn base_catalog(&self) -> AppResult<Vec<UpstreamModelEntry>> {
        if let Some(cached) = self.base_catalog.read().await.as_ref()
            && cached.fetched_at.elapsed() < self.base.model_catalog_cache_ttl()
        {
            return Ok(cached.catalog.clone());
        }
        let catalog = self.base.supported_model_catalog().await?;
        *self.base_catalog.write().await = Some(CachedBaseCatalog {
            fetched_at: Instant::now(),
            catalog: catalog.clone(),
        });
        Ok(catalog)
    }

    async fn base_catalog_or_empty(&self) -> Vec<UpstreamModelEntry> {
        self.base_catalog().await.unwrap_or_default()
    }

    fn routed_request(
        &self,
        backend: &BackendChatRequest,
        provider: &ManagedProvider,
    ) -> BackendChatRequest {
        let mut request = backend.request.clone();
        let resolved = provider
            .stored
            .models
            .iter()
            .find(|entry| entry.id.eq_ignore_ascii_case(&request.model))
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| request.model.clone());
        request.model = resolved;
        if let Some(serving) = &backend.serving {
            serving.set_route("configured");
            serving.set_provider(provider.stored.name.clone());
        }
        BackendChatRequest {
            request,
            client_chat_template_kwargs: backend.client_chat_template_kwargs.clone(),
            thinking_override: backend.thinking_override,
            response_id: backend.response_id.clone(),
            serving: backend.serving.clone(),
            capture: backend.capture.clone(),
            persistence_capture: backend.persistence_capture.clone(),
            wire_capture: backend.wire_capture.clone(),
            authorization: backend.authorization.clone(),
            endpoint: backend.endpoint,
            authorization_route: Some("configured".to_string()),
            authorization_provider: Some(provider.stored.name.clone()),
        }
    }
}

#[async_trait]
impl UpstreamClient for ManagedProviderUpstream {
    async fn stream_chat_completion(
        &self,
        request: &BackendChatRequest,
    ) -> AppResult<crate::upstream::UpstreamStream> {
        self.stream_chat_completion_with_timeout(request, Duration::from_secs(60))
            .await
    }

    async fn stream_chat_completion_with_timeout(
        &self,
        request: &BackendChatRequest,
        request_timeout: Duration,
    ) -> AppResult<crate::upstream::UpstreamStream> {
        let base_catalog = self.base_catalog_or_empty().await;
        if let Some(provider) = self
            .registry
            .route_for_model(&request.request.model, &base_catalog)
            .await?
        {
            let routed = self.routed_request(request, &provider);
            ensure_provider_authorized(
                &routed.authorization,
                &provider,
                &routed.request.model,
                routed.endpoint,
            )?;
            return provider
                .client
                .stream_chat_completion_with_timeout(&routed, request_timeout)
                .await;
        }
        self.base
            .stream_chat_completion_with_timeout(request, request_timeout)
            .await
    }

    async fn list_models(&self) -> AppResult<UpstreamModelsResponse> {
        let catalog = self.supported_model_catalog().await?;
        models_response(catalog)
    }

    async fn proxy_completions(
        &self,
        mut request: ProxyCompletionsRequest,
    ) -> AppResult<reqwest::Response> {
        let base_catalog = self.base_catalog_or_empty().await;
        let requested_model = proxy_body_model(&request.body).unwrap_or_default();
        if let Some(provider) = self
            .registry
            .route_for_model(&requested_model, &base_catalog)
            .await?
        {
            ensure_provider_authorized(
                &request.authorization,
                &provider,
                &requested_model,
                request.endpoint,
            )?;
            request.body = proxy_body_with_model(request.body, &requested_model)?;
            request.authorization_route = Some("configured".to_string());
            request.authorization_provider = Some(provider.stored.name.clone());
            return provider.client.proxy_completions(request).await;
        }
        self.base.proxy_completions(request).await
    }

    async fn count_tokens(&self, request: &BackendChatRequest) -> AppResult<Option<u64>> {
        let base_catalog = self.base_catalog_or_empty().await;
        if let Some(provider) = self
            .registry
            .route_for_model(&request.request.model, &base_catalog)
            .await?
        {
            let routed = self.routed_request(request, &provider);
            ensure_provider_authorized(
                &routed.authorization,
                &provider,
                &routed.request.model,
                routed.endpoint,
            )?;
            return provider.client.count_tokens(&routed).await;
        }
        self.base.count_tokens(request).await
    }

    async fn supported_model_catalog(&self) -> AppResult<Vec<UpstreamModelEntry>> {
        let mut catalog = self.base_catalog().await?;
        let mut seen = catalog
            .iter()
            .map(|entry| entry.id.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let mut counts = HashMap::<String, usize>::new();
        self.registry.ensure_loaded().await?;
        let state = self.registry.state.read().await;
        for provider in state.providers.iter().filter(|p| p.stored.enabled) {
            for model in &provider.stored.models {
                *counts.entry(model.id.to_ascii_lowercase()).or_default() += 1;
            }
        }
        for provider in state.providers.iter().filter(|p| p.stored.enabled) {
            for model in &provider.stored.models {
                let key = model.id.to_ascii_lowercase();
                if counts.get(&key) == Some(&1) && seen.insert(key) {
                    catalog.push(model.clone());
                }
            }
        }
        Ok(catalog)
    }

    async fn provider_inventory(&self) -> AppResult<Vec<ProviderInventoryEntry>> {
        let mut inventory = self.base.provider_inventory().await.unwrap_or_default();
        inventory.extend(self.registry.inventory().await?);
        Ok(inventory)
    }

    async fn backend_candidate_plan(&self, requested_model: &str) -> BackendCandidatePlan {
        let base_plan = self.base.backend_candidate_plan(requested_model).await;
        let base_catalog = self.base_catalog_or_empty().await;
        let provider = match self
            .registry
            .route_for_model(requested_model, &base_catalog)
            .await
        {
            Ok(provider) => provider,
            Err(_) => return BackendCandidatePlan { candidates: vec![] },
        };
        if let Some(provider) = provider
            && let Some(model) = provider
                .stored
                .models
                .iter()
                .find(|entry| entry.id.eq_ignore_ascii_case(requested_model))
        {
            return BackendCandidatePlan {
                candidates: vec![BackendCandidate {
                    model: model.id.clone(),
                    context_limit: model.context_limit,
                }],
            };
        }
        base_plan
    }

    fn provider_health(&self) -> Vec<crate::upstream::ProviderHealth> {
        self.base.provider_health()
    }

    fn provider_base_url(&self) -> String {
        self.base.provider_base_url()
    }

    fn model_catalog_cache_ttl(&self) -> Duration {
        Duration::from_secs(0)
    }
}

fn ensure_provider_authorized(
    authorization: &crate::upstream::AuthorizationScope,
    provider: &ManagedProvider,
    model: &str,
    endpoint: InferenceEndpoint,
) -> AppResult<()> {
    if authorization.allows_candidate(&provider.stored.name, Some("configured"), model, endpoint) {
        Ok(())
    } else {
        Err(AppError::forbidden(
            "the authenticated key is not authorized for this inference candidate",
        ))
    }
}

fn provider_view(provider: &ManagedProvider) -> ConfiguredProviderView {
    ConfiguredProviderView {
        revision: provider.stored.revision,
        enabled: provider.stored.enabled,
        id: provider.stored.id.clone(),
        name: provider.stored.name.clone(),
        base_url: provider.stored.base_url.clone(),
        api_key_present: !provider.stored.api_key.is_empty(),
        models: provider.stored.models.clone(),
    }
}

fn validate_provider_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(AppError::bad_request(
            "provider name must be 1-64 characters",
        ));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ' ' | '.'))
    {
        return Err(AppError::bad_request(
            "provider name may contain letters, numbers, spaces, '.', '_' and '-'",
        ));
    }
    Ok(name.to_string())
}

fn normalize_provider_url(input: &str) -> AppResult<Url> {
    let mut url = Url::parse(input.trim())
        .map_err(|err| AppError::bad_request(format!("invalid provider URL: {err}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::bad_request(
            "provider URL must not contain credentials",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(AppError::bad_request(
            "provider URL must not contain query or fragment",
        ));
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_url(&url) => {}
        _ => {
            return Err(AppError::bad_request(
                "provider URL must use https, except http is allowed for loopback hosts",
            ));
        }
    }
    let mut segments = url
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    if segments.is_empty() {
        segments.push("v1");
    }
    let mut path = format!("/{}", segments.join("/"));
    if !path.ends_with('/') {
        path.push('/');
    }
    url.set_path(&path);
    Ok(url)
}

fn is_loopback_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
}

fn models_response(catalog: Vec<UpstreamModelEntry>) -> AppResult<UpstreamModelsResponse> {
    let data = catalog
        .into_iter()
        .map(|entry| {
            let mut value = serde_json::json!({
                "id": entry.id,
                "object": "model",
            });
            if let Some(context_limit) = entry.context_limit
                && let Some(object) = value.as_object_mut()
            {
                object.insert(
                    "context_length".to_string(),
                    Value::Number(context_limit.into()),
                );
            }
            if let Some(capabilities) = entry.capabilities {
                value["capabilities"] = capabilities;
            }
            value
        })
        .collect::<Vec<_>>();
    let body = serde_json::to_vec(&serde_json::json!({
        "object": "list",
        "data": data,
    }))
    .map_err(|err| AppError::internal(format!("failed to serialize configured models: {err}")))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/json".parse().expect("static header"),
    );
    Ok(UpstreamModelsResponse {
        status: StatusCode::OK,
        headers,
        body: Bytes::from(body),
    })
}

fn proxy_body_model(body: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn proxy_body_with_model(body: Bytes, model: &str) -> AppResult<Bytes> {
    let mut value = serde_json::from_slice::<Value>(&body)
        .map_err(|err| AppError::bad_request(format!("invalid completions JSON body: {err}")))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("model".to_string(), Value::String(model.to_string()));
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|err| AppError::internal(format!("failed to serialize completions body: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane_store::SqlStore;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[derive(Clone)]
    struct CatalogOnlyUpstream {
        models: Vec<UpstreamModelEntry>,
    }

    #[async_trait]
    impl UpstreamClient for CatalogOnlyUpstream {
        async fn stream_chat_completion(
            &self,
            _request: &BackendChatRequest,
        ) -> AppResult<crate::upstream::UpstreamStream> {
            Err(AppError::internal("not implemented"))
        }

        async fn list_models(&self) -> AppResult<UpstreamModelsResponse> {
            models_response(self.models.clone())
        }

        async fn supported_model_catalog(&self) -> AppResult<Vec<UpstreamModelEntry>> {
            Ok(self.models.clone())
        }
    }

    fn test_options() -> ManagedProviderOptions {
        ManagedProviderOptions {
            traffic: crate::traffic::TrafficController::new(),
            http_client: reqwest::Client::new(),
            flatten_content: true,
            min_completion_tokens: 1,
            max_sse_frame_bytes: 1024,
            finalization_policies: crate::upstream::BackendFinalizationPolicies::default(),
            flow_store: DashboardFlowStore::disabled(),
        }
    }

    async fn test_registry() -> Arc<ManagedProviderRegistry> {
        let store = Arc::new(
            SqlStore::connect_sqlite("sqlite::memory:")
                .await
                .expect("sqlite store"),
        );
        ManagedProviderRegistry::new(store, test_options())
    }

    #[test]
    fn normalizes_loopback_http_and_adds_v1() {
        let url = normalize_provider_url("http://127.0.0.1:8080").expect("valid");
        assert_eq!(url.as_str(), "http://127.0.0.1:8080/v1/");
    }

    #[test]
    fn rejects_non_loopback_http_and_url_credentials() {
        assert!(normalize_provider_url("http://example.com/v1").is_err());
        assert!(normalize_provider_url("https://user:pass@example.com/v1").is_err());
    }

    #[test]
    fn provider_views_redact_api_key() {
        let provider = ManagedProvider {
            stored: StoredProvider {
                revision: 1,
                enabled: true,
                id: "cfg_1".to_string(),
                name: "external".to_string(),
                base_url: "https://api.example.com/v1/".to_string(),
                api_key: "secret".to_string(),
                models: vec![UpstreamModelEntry {
                    capabilities: None,
                    id: "model-a".to_string(),
                    context_limit: Some(4096),
                }],
            },
            client: Arc::new(ReqwestUpstreamClient::with_options(
                reqwest::Client::new(),
                Url::parse("https://api.example.com/v1/").unwrap(),
                Some("secret".to_string()),
                None,
                true,
                1,
                1024,
            )),
        };
        let json = serde_json::to_string(&provider_view(&provider)).expect("serialize");
        assert!(json.contains("\"api_key_present\":true"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn configured_provider_routing_honors_authorization_scope() {
        let provider = ManagedProvider {
            stored: StoredProvider {
                revision: 1,
                enabled: true,
                id: "cfg_1".to_string(),
                name: "external".to_string(),
                base_url: "https://api.example.com/v1/".to_string(),
                api_key: "secret".to_string(),
                models: vec![UpstreamModelEntry {
                    capabilities: None,
                    id: "model-a".to_string(),
                    context_limit: None,
                }],
            },
            client: Arc::new(CatalogOnlyUpstream { models: Vec::new() }),
        };
        let denied = crate::upstream::AuthorizationScope::restricted(
            |_provider, _route, _model, _endpoint| false,
        );
        let allowed =
            crate::upstream::AuthorizationScope::restricted(|provider, route, model, endpoint| {
                provider == "external"
                    && route == Some("configured")
                    && model == "model-a"
                    && endpoint == InferenceEndpoint::ChatCompletions
            });

        assert!(
            ensure_provider_authorized(
                &denied,
                &provider,
                "model-a",
                InferenceEndpoint::ChatCompletions,
            )
            .is_err()
        );
        assert!(
            ensure_provider_authorized(
                &allowed,
                &provider,
                "model-a",
                InferenceEndpoint::ChatCompletions,
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn add_discovers_models_persists_and_redacts_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer secret-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [
                    {"id": "external-a", "object": "model", "context_length": 8192}
                ]
            })))
            .mount(&server)
            .await;
        let registry = test_registry().await;
        let added = registry
            .add(CreateConfiguredProviderRequest {
                name: "external".to_string(),
                base_url: server.uri(),
                api_key: "secret-key".to_string(),
            })
            .await
            .expect("provider added");
        assert_eq!(added.name, "external");
        assert_eq!(added.models[0].id, "external-a");
        let json = serde_json::to_string(&ConfiguredProvidersBody {
            providers: registry.list().await.expect("list"),
        })
        .expect("serialize");
        assert!(json.contains("external-a"));
        assert!(!json.contains("secret-key"));
    }

    #[tokio::test]
    async fn update_rotates_credentials_disables_routing_and_checks_revision() {
        let server = MockServer::start().await;
        for secret in ["old-key", "new-key"] {
            Mock::given(method("GET"))
                .and(path("/v1/models"))
                .and(header("authorization", format!("Bearer {secret}")))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"data":[{"id":"managed-model"}]})),
                )
                .expect(1)
                .mount(&server)
                .await;
        }
        let registry = test_registry().await;
        let added = registry
            .add(CreateConfiguredProviderRequest {
                name: "provider".into(),
                base_url: server.uri(),
                api_key: "old-key".into(),
            })
            .await
            .unwrap();
        let updated = registry
            .update(
                &added.id,
                UpdateConfiguredProviderRequest {
                    revision: added.revision,
                    name: "renamed".into(),
                    base_url: added.base_url.clone(),
                    enabled: true,
                    api_key: Some("new-key".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.id, added.id);
        assert_eq!(updated.revision, added.revision + 1);
        assert!(!serde_json::to_string(&updated).unwrap().contains("new-key"));
        let disabled = registry
            .update(
                &added.id,
                UpdateConfiguredProviderRequest {
                    revision: updated.revision,
                    name: updated.name.clone(),
                    base_url: updated.base_url.clone(),
                    enabled: false,
                    api_key: None,
                },
            )
            .await
            .unwrap();
        assert!(!disabled.enabled);
        assert!(registry.inventory().await.unwrap().is_empty());
        assert!(
            registry
                .route_for_model("managed-model", &[])
                .await
                .is_err()
        );
        assert!(
            registry
                .update(
                    &added.id,
                    UpdateConfiguredProviderRequest {
                        revision: updated.revision,
                        name: updated.name,
                        base_url: updated.base_url,
                        enabled: false,
                        api_key: Some("".into())
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(
            registry.list().await.unwrap()[0].revision,
            disabled.revision
        );
    }
    #[test]
    fn update_provider_wire_roundtrip() {
        let value = serde_json::json!({"revision":3,"name":"provider","base_url":"https://example.com/v1/","enabled":false,"api_key":null});
        let request: UpdateConfiguredProviderRequest =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), value);
    }

    #[tokio::test]
    async fn catalog_union_preserves_base_precedence_and_routes_unique_dynamic_models() {
        let registry = test_registry().await;
        {
            let dynamic = StoredProvider {
                revision: 1,
                enabled: true,
                id: "cfg_dynamic".to_string(),
                name: "dynamic".to_string(),
                base_url: "https://dynamic.example/v1/".to_string(),
                api_key: "secret".to_string(),
                models: vec![
                    UpstreamModelEntry {
                        capabilities: None,
                        id: "base-model".to_string(),
                        context_limit: Some(123),
                    },
                    UpstreamModelEntry {
                        capabilities: None,
                        id: "dynamic-model".to_string(),
                        context_limit: Some(456),
                    },
                ],
            };
            registry.state.write().await.providers = vec![ManagedProvider {
                stored: dynamic,
                client: Arc::new(CatalogOnlyUpstream { models: Vec::new() }),
            }];
            registry.state.write().await.loaded = true;
        }
        let upstream = ManagedProviderUpstream::new(
            Arc::new(CatalogOnlyUpstream {
                models: vec![UpstreamModelEntry {
                    capabilities: None,
                    id: "base-model".to_string(),
                    context_limit: Some(999),
                }],
            }),
            registry,
        );

        let catalog = upstream.supported_model_catalog().await.expect("catalog");
        assert_eq!(
            catalog
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["base-model", "dynamic-model"]
        );
        assert_eq!(
            catalog
                .iter()
                .find(|entry| entry.id == "base-model")
                .and_then(|entry| entry.context_limit),
            Some(999),
            "base catalog entry wins on collision"
        );

        let dynamic_plan = upstream.backend_candidate_plan("dynamic-model").await;
        assert_eq!(dynamic_plan.candidates.len(), 1);
        assert_eq!(dynamic_plan.candidates[0].model, "dynamic-model");
        assert_eq!(dynamic_plan.candidates[0].context_limit, Some(456));

        let base_plan = upstream.backend_candidate_plan("base-model").await;
        assert_eq!(base_plan.candidates[0].model, "base-model");
        assert_eq!(base_plan.candidates[0].context_limit, None);
    }
}

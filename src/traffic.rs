//! Opt-in admission and routing. Policy is durable; queues contain only bounded
//! metadata. One controller owns all HTTP leaf attempts in this gateway process.
use crate::error::{AppError, AppResult};
use chrono::{Datelike, NaiveDate, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Notify;

pub const STORE_KEY: &str = "traffic_policy_v1";

#[derive(utoipa::ToSchema, Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Off,
    Observe,
    Enforce,
}
#[derive(utoipa::ToSchema, Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Affinity {
    #[default]
    Off,
    Prefer,
    Strict,
}
#[derive(utoipa::ToSchema, Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub priority: Option<u16>,
    pub max_concurrent: Option<u32>,
    pub models: BTreeMap<String, u32>,
    pub max_wait_ms: Option<u64>,
}
#[derive(utoipa::ToSchema, Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    #[serde(flatten)]
    pub limits: Limits,
    /// A separate shared ceiling; the inherited max_concurrent is per key.
    pub shared_concurrent: Option<u32>,
    pub shared_models: BTreeMap<String, u32>,
    pub schedules: Vec<Window>,
}
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub timezone: String,
    /// ISO weekdays, 1=Monday. Empty means every day.
    #[serde(default)]
    pub weekdays: Vec<u8>,
    /// Dated exceptions; when present these replace weekday matching.
    #[serde(default)]
    #[schema(value_type=Vec<String>)]
    pub dates: Vec<NaiveDate>,
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub shared_concurrent: Option<u32>,
    #[serde(default)]
    pub shared_models: BTreeMap<String, u32>,
}
fn minute(text: &str) -> Result<u16, String> {
    let (h, m) = text.split_once(':').ok_or("time must be HH:MM")?;
    let h: u16 = h.parse().map_err(|_| "invalid hour")?;
    let m: u16 = m.parse().map_err(|_| "invalid minute")?;
    if h > 23 || m > 59 {
        return Err("invalid time".into());
    }
    Ok(h * 60 + m)
}
impl Window {
    fn active(&self, now: chrono::DateTime<Utc>) -> bool {
        let Ok(tz) = self.timezone.parse::<Tz>() else {
            return false;
        };
        let local = now.with_timezone(&tz);
        let (Ok(start), Ok(end)) = (minute(&self.start), minute(&self.end)) else {
            return false;
        };
        let current = (local.hour() * 60 + local.minute()) as u16;
        let overnight = start > end;
        let date = if overnight && current < end {
            local.date_naive().pred_opt().unwrap()
        } else {
            local.date_naive()
        };
        let day = if self.dates.is_empty() {
            self.weekdays.is_empty()
                || self
                    .weekdays
                    .contains(&(date.weekday().number_from_monday() as u8))
        } else {
            self.dates.contains(&date)
        };
        day && (start == end
            || if overnight {
                current >= start || current < end
            } else {
                current >= start && current < end
            })
    }
}
impl Rule {
    fn at(&self, now: chrono::DateTime<Utc>) -> Self {
        let mut rule = self.clone();
        // Dated exceptions override weekly windows. Within a tier, the last
        // matching window wins; the editor documents and previews this order.
        for dated in [false, true] {
            for window in self
                .schedules
                .iter()
                .filter(|w| !w.dates.is_empty() == dated && w.active(now))
            {
                if window.limits.priority.is_some() {
                    rule.limits.priority = window.limits.priority;
                }
                if window.limits.max_concurrent.is_some() {
                    rule.limits.max_concurrent = window.limits.max_concurrent;
                }
                if window.limits.max_wait_ms.is_some() {
                    rule.limits.max_wait_ms = window.limits.max_wait_ms;
                }
                rule.limits.models.extend(window.limits.models.clone());
                if window.shared_concurrent.is_some() {
                    rule.shared_concurrent = window.shared_concurrent;
                }
                rule.shared_models.extend(window.shared_models.clone());
            }
        }
        rule
    }
}
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct QueueConfig {
    pub enabled: bool,
    pub max_requests: usize,
    pub max_bytes: usize,
    pub per_key: usize,
    pub max_wait_ms: u64,
    pub aging_ms: u64,
}
impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_requests: 512,
            max_bytes: 32 * 1024 * 1024,
            per_key: 32,
            max_wait_ms: 30_000,
            aging_ms: 10_000,
        }
    }
}
#[derive(utoipa::ToSchema, Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BackendPolicy {
    pub max_concurrent: Option<u32>,
    pub paused: bool,
    /// Physical resource identity for URLs that reach the same engine.
    pub resource: Option<String>,
    /// Managed deployment whose local benchmark uses this engine.
    pub deployment_id: Option<String>,
}
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelPolicy {
    pub budget: Option<String>,
    /// Explicit compatible replicas, matched by provider name or base URL.
    pub replicas: Vec<String>,
    pub balance: bool,
    pub affinity: Affinity,
    pub affinity_ttl_secs: u64,
    pub affinity_wait_ms: u64,
}
impl Default for ModelPolicy {
    fn default() -> Self {
        Self {
            budget: None,
            replicas: vec![],
            balance: false,
            affinity: Affinity::Off,
            affinity_ttl_secs: 1800,
            affinity_wait_ms: 1000,
        }
    }
}
#[derive(utoipa::ToSchema, Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TrafficConfig {
    pub mode: Mode,
    pub defaults: Rule,
    pub groups: BTreeMap<String, Rule>,
    /// Members are stable user:<id> or key:<id> references; legacy SQL keys inherit owner membership.
    pub group_members: BTreeMap<String, Vec<String>>,
    pub users: BTreeMap<String, Rule>,
    pub keys: BTreeMap<String, Rule>,
    pub models: BTreeMap<String, ModelPolicy>,
    pub backends: BTreeMap<String, BackendPolicy>,
    pub queue: QueueConfig,
    pub mobile_compact: bool,
    pub capture_engine_metrics: bool,
    pub cancellation_grace_ms: u64,
}
impl TrafficConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.keys.len()
            + self.groups.len()
            + self.users.len()
            + self.models.len()
            + self.backends.len()
            > 4096
        {
            return Err("too many traffic policies".into());
        }
        if self.group_members.len() > 4096
            || self.group_members.iter().any(|(group, members)| {
                group.is_empty()
                    || group.len() > 256
                    || members.len() > 1024
                    || members.iter().any(|id| {
                        id.len() > 256 || !(id.starts_with("user:") || id.starts_with("key:"))
                    })
            })
        {
            return Err("invalid traffic group membership".into());
        }
        if self.cancellation_grace_ms > 30_000 {
            return Err("cancellation grace is at most 30 seconds".into());
        }
        let q = &self.queue;
        if q.max_requests > 4096
            || q.max_bytes > 256 * 1024 * 1024
            || q.per_key > 4096
            || q.max_wait_ms > 300_000
            || q.aging_ms == 0
        {
            return Err("queue exceeds bounds (4096 requests, 256 MiB, 5 minutes)".into());
        }
        for rule in std::iter::once(&self.defaults)
            .chain(self.groups.values())
            .chain(self.users.values())
            .chain(self.keys.values())
        {
            if rule.schedules.len() > 128 {
                return Err("too many schedule windows".into());
            }
            for limits in
                std::iter::once(&rule.limits).chain(rule.schedules.iter().map(|w| &w.limits))
            {
                if limits.priority.is_some_and(|p| p == 0 || p > 1000)
                    || limits.max_wait_ms.is_some_and(|v| v > 300_000)
                    || limits.models.len() > 256
                {
                    return Err("priority must be 1..1000; wait at most 5 minutes".into());
                }
            }
            for w in &rule.schedules {
                w.timezone
                    .parse::<Tz>()
                    .map_err(|_| "unknown IANA timezone")?;
                minute(&w.start)?;
                minute(&w.end)?;
                if w.weekdays.iter().any(|d| !(1..=7).contains(d)) || w.dates.len() > 366 {
                    return Err("invalid schedule days".into());
                }
            }
        }
        for (url, backend) in &self.backends {
            let parsed = url
                .parse::<url::Url>()
                .map_err(|_| "backend keys must be HTTP(S) base URLs")?;
            if (url != "mesh://workers" && !matches!(parsed.scheme(), "http" | "https"))
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || url.ends_with('/')
            {
                return Err(
                    "backend URLs must omit credentials, queries and trailing slash".into(),
                );
            }
            if backend
                .resource
                .as_ref()
                .is_some_and(|r| r.is_empty() || r.len() > 256)
            {
                return Err("invalid backend resource".into());
            }
        }
        for model in self.models.values() {
            if model.replicas.len() > 64
                || model.affinity_ttl_secs > 86400
                || model.affinity_wait_ms > 300_000
            {
                return Err("model routing exceeds bounds".into());
            }
            if (model.balance || model.affinity != Affinity::Off) && model.replicas.len() < 2 {
                return Err(
                    "balancing/affinity requires at least two explicit compatible replicas".into(),
                );
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| "cannot encode policy")?
            .len()
            > 1024 * 1024
        {
            return Err("traffic config exceeds 1 MiB".into());
        }
        Ok(())
    }
    fn model_policy(&self, model: &str) -> ModelPolicy {
        self.models
            .get(model)
            .or_else(|| self.models.get("*"))
            .cloned()
            .unwrap_or_default()
    }
    fn budget(&self, model: &str) -> String {
        self.model_policy(model)
            .budget
            .unwrap_or_else(|| model.to_owned())
    }
}

#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, Default)]
pub struct Identity {
    pub key: String,
    pub user: String,
    pub groups: Vec<String>,
}
/// Authenticated HTTP attribution; raw credentials never cross this boundary.
#[derive(Clone, Default)]
pub struct Caller {
    pub identity: Identity,
    pub session: Option<String>,
    pub client_auth: Option<crate::client_auth::ClientAuth>,
}

#[derive(Debug)]
pub struct TrafficRequest {
    pub id: String,
    pub identity: Identity,
    pub model: String,
    /// One-way session digest, never a raw credential/conversation identifier.
    pub session: Option<String>,
    pub bytes: AtomicUsize,
    owner: Mutex<Option<Weak<Inner>>>,
}
impl TrafficRequest {
    pub fn new(
        identity: Identity,
        model: String,
        session: Option<String>,
        bytes: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: uuid::Uuid::new_v4().to_string(),
            identity,
            model,
            session,
            bytes: AtomicUsize::new(bytes),
            owner: Mutex::new(None),
        })
    }
    pub fn set_bytes(&self, bytes: usize) {
        self.bytes.store(bytes, Ordering::Relaxed);
    }
}
impl Drop for TrafficRequest {
    fn drop(&mut self) {
        if let Some(inner) = self.owner.lock().unwrap().as_ref().and_then(Weak::upgrade) {
            inner.state.lock().unwrap().clients.remove(&self.id);
            inner.changed.notify_waiters();
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct RequestView {
    pub id: String,
    pub key: String,
    pub user: String,
    pub groups: Vec<String>,
    pub model: String,
    pub budget: String,
    pub backend: String,
    pub priority: u16,
    pub created_at_ms: i64,
    pub state: String,
    pub reason: Option<String>,
    pub bytes: usize,
    pub affinity: String,
}
#[derive(Debug)]
struct Pending {
    view: RequestView,
    started: Instant,
    flow: String,
    request: Arc<TrafficRequest>,
    provider: String,
}
#[derive(Debug, Clone)]
struct Pin {
    backend: String,
    seen: Instant,
}
#[derive(Debug, Default)]
struct State {
    config: TrafficConfig,
    revision: u64,
    ready: bool,
    pending: BTreeMap<String, Pending>,
    clients: HashMap<String, RequestView>,
    attempts: HashMap<String, RequestView>,
    pins: HashMap<String, Pin>,
    finish: HashMap<String, f64>,
    virtual_time: f64,
    completed_metrics: std::collections::VecDeque<CompletedMetrics>,
    admitted: u64,
    rejected: u64,
    cancelled: u64,
    shadow_blocked: u64,
    reservations: HashMap<String, String>,
    cancelled_tickets: std::collections::HashSet<String>,
}
#[derive(Debug, Default)]
struct Inner {
    state: Mutex<State>,
    changed: Notify,
    jobs: Mutex<HashMap<String, AttemptLease>>,
}
#[derive(Debug, Clone, Default)]
pub struct TrafficController {
    inner: Arc<Inner>,
}
#[derive(Debug, Clone, Serialize)]
pub struct CompletedMetrics {
    pub request: RequestView,
    pub received_at_ms: i64,
    pub metrics: crate::models::chat::EngineRequestMetrics,
}
#[derive(Debug, Clone, Serialize)]
pub struct TrafficSnapshot {
    pub completed_metrics: Vec<CompletedMetrics>,
    pub mode: Mode,
    pub ready: bool,
    pub revision: u64,
    pub queued: Vec<RequestView>,
    pub active: Vec<RequestView>,
    pub clients: usize,
    pub admitted: u64,
    pub rejected: u64,
    pub cancelled: u64,
    pub shadow_blocked: u64,
    pub queued_bytes: usize,
    pub reservations: BTreeMap<String, String>,
}
#[derive(Debug)]
pub struct AttemptLease {
    inner: Weak<Inner>,
    id: String,
    request: Arc<TrafficRequest>,
    completed: bool,
}
impl AttemptLease {
    pub fn wire_id(&self) -> &str {
        &self.id
    }
    pub fn engine_metrics(&self, metrics: &crate::models::chat::EngineRequestMetrics) {
        if !metrics.valid() {
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut s = inner.state.lock().unwrap();
        if !s.config.capture_engine_metrics {
            return;
        }
        let Some(request) = s.attempts.get(&self.id).cloned() else {
            return;
        };
        if s.completed_metrics.len() >= 128 {
            s.completed_metrics.pop_front();
        }
        s.completed_metrics.push_back(CompletedMetrics {
            request,
            received_at_ms: Utc::now().timestamp_millis(),
            metrics: metrics.clone(),
        });
    }

    pub fn awaiting(&self) {
        self.set_state("awaiting_first_token");
    }
    pub fn streaming(&self) {
        self.set_state("streaming");
        if let Some(inner) = self.inner.upgrade() {
            let mut s = inner.state.lock().unwrap();
            let p = s.config.model_policy(&self.request.model);
            if s.config.mode == Mode::Enforce && p.affinity != Affinity::Off {
                let backend = s.attempts.get(&self.id).map(|v| v.backend.clone());
                if let (Some(key), Some(backend)) = (
                    pin_key(&self.request, &s.config.budget(&self.request.model)),
                    backend,
                ) {
                    if s.pins.len() >= 8192
                        && !s.pins.contains_key(&key)
                        && let Some(old) = s
                            .pins
                            .iter()
                            .min_by_key(|(_, p)| p.seen)
                            .map(|(k, _)| k.clone())
                    {
                        s.pins.remove(&old);
                    }
                    s.pins.insert(
                        key,
                        Pin {
                            backend,
                            seen: Instant::now(),
                        },
                    );
                }
            }
        }
    }
    pub fn set_state(&self, phase: &str) {
        if let Some(inner) = self.inner.upgrade()
            && let Some(v) = inner.state.lock().unwrap().attempts.get_mut(&self.id)
        {
            v.state = phase.to_owned();
        }
    }
    pub fn complete(&mut self) {
        self.completed = true;
    }
    pub fn request(&self) -> Arc<TrafficRequest> {
        self.request.clone()
    }
}
impl Drop for AttemptLease {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut state = inner.state.lock().unwrap();
            let grace = if !self.completed
                && state.config.mode == Mode::Enforce
                && state.attempts.get(&self.id).is_some_and(|v| {
                    matches!(
                        v.state.as_str(),
                        "streaming" | "awaiting_first_token" | "backend_response"
                    )
                }) {
                state.config.cancellation_grace_ms
            } else {
                0
            };
            if grace > 0
                && let Ok(handle) = tokio::runtime::Handle::try_current()
            {
                if let Some(v) = state.attempts.get_mut(&self.id) {
                    v.state = "cancelling".into();
                }
                let weak = Arc::downgrade(&inner);
                let id = self.id.clone();
                handle.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(grace)).await;
                    if let Some(inner) = weak.upgrade() {
                        inner.state.lock().unwrap().attempts.remove(&id);
                        inner.changed.notify_waiters();
                    }
                });
            } else {
                state.attempts.remove(&self.id);
            }
            if !self.completed {
                state.cancelled += 1;
            }
            drop(state);
            inner.changed.notify_waiters();
        }
    }
}
struct Ticket {
    inner: Weak<Inner>,
    id: String,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .state
                .lock()
                .unwrap()
                .cancelled_tickets
                .remove(&self.id);
            if inner
                .state
                .lock()
                .unwrap()
                .pending
                .remove(&self.id)
                .is_some()
            {
                inner.changed.notify_waiters();
            }
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct EffectivePolicy {
    pub priority: u16,
    pub limits: Vec<(String, u32)>,
    pub max_wait_ms: u64,
    pub budget: String,
}
fn effective(
    config: &TrafficConfig,
    r: &TrafficRequest,
    backend: &str,
    _provider: &str,
    now: chrono::DateTime<Utc>,
) -> EffectivePolicy {
    let budget = config.budget(&r.model);
    let mut rules = vec![("defaults".to_owned(), config.defaults.at(now))];
    let mut groups = groups_for(config, &r.identity);
    groups.sort();
    groups.dedup();
    for group in groups {
        if let Some(rule) = config.groups.get(&group) {
            rules.push((format!("group:{group}"), rule.at(now)));
        }
    }
    if let Some(rule) = config.users.get(&r.identity.user) {
        rules.push((format!("user:{}", r.identity.user), rule.at(now)));
    }
    if let Some(rule) = config.keys.get(&r.identity.key) {
        rules.push((format!("key:{}", r.identity.key), rule.at(now)));
    }
    let mut priority = 1;
    let mut limits = vec![];
    let mut wait = config.queue.max_wait_ms;
    for (scope, rule) in rules {
        if let Some(p) = rule.limits.priority {
            priority = p;
        }
        if let Some(v) = rule.limits.max_concurrent {
            limits.push((format!("key:{}", r.identity.key), v));
        }
        for model in [&budget, &r.model] {
            if let Some(v) = rule
                .limits
                .models
                .get(model)
                .or_else(|| rule.limits.models.get("*"))
            {
                limits.push((format!("key-model:{}:{budget}", r.identity.key), *v));
            }
        }
        if let Some(v) = rule.shared_concurrent {
            limits.push((scope.clone(), v));
        }
        for model in [&budget, &r.model] {
            if let Some(v) = rule
                .shared_models
                .get(model)
                .or_else(|| rule.shared_models.get("*"))
            {
                limits.push((format!("{scope}-model:{budget}"), *v));
            }
        }
        if let Some(v) = rule.limits.max_wait_ms {
            wait = wait.min(v);
        }
    }
    let physical = resource(config, backend);
    for (url, policy) in &config.backends {
        if resource(config, url) != physical {
            continue;
        }
        if let Some(limit) = policy.max_concurrent {
            limits.push((format!("backend:{physical}"), limit));
        }
        if policy.paused {
            limits.push((format!("backend:{physical}"), 0));
        }
    }
    EffectivePolicy {
        priority,
        limits,
        max_wait_ms: wait,
        budget,
    }
}
fn resource(config: &TrafficConfig, backend: &str) -> String {
    config
        .backends
        .get(backend)
        .and_then(|b| b.resource.clone())
        .unwrap_or_else(|| backend.to_owned())
}
fn groups_for(config: &TrafficConfig, identity: &Identity) -> Vec<String> {
    let mut groups = identity.groups.clone();
    let user = format!("user:{}", identity.user);
    let key = format!("key:{}", identity.key);
    for (group, members) in &config.group_members {
        if members.contains(&user) || members.contains(&key) {
            groups.push(group.clone());
        }
    }
    groups.sort();
    groups.dedup();
    groups
}
fn member(view: &RequestView, scope: &str, config: &TrafficConfig) -> bool {
    scope == "defaults"
        || scope == format!("key:{}", view.key)
        || scope == format!("user:{}", view.user)
        || groups_for(
            config,
            &Identity {
                key: view.key.clone(),
                user: view.user.clone(),
                groups: view.groups.clone(),
            },
        )
        .iter()
        .any(|g| scope == format!("group:{g}"))
        || scope == format!("key-model:{}:{}", view.key, view.budget)
        || scope == format!("user:{}-model:{}", view.user, view.budget)
        || scope == format!("key:{}-model:{}", view.key, view.budget)
        || scope == format!("defaults-model:{}", view.budget)
        || groups_for(
            config,
            &Identity {
                key: view.key.clone(),
                user: view.user.clone(),
                groups: view.groups.clone(),
            },
        )
        .iter()
        .any(|g| scope == format!("group:{g}-model:{}", view.budget))
        || scope == format!("backend:{}", resource(config, &view.backend))
}
fn blocked(
    state: &State,
    r: &TrafficRequest,
    policy: &EffectivePolicy,
    backend: &str,
) -> Option<String> {
    if state
        .reservations
        .values()
        .any(|b| b == "*" || b == &resource(&state.config, backend))
    {
        return Some("backend reserved for benchmark".into());
    }
    for (scope, limit) in &policy.limits {
        if !scope.starts_with("backend:") && state.clients.contains_key(&r.id) {
            continue;
        }
        let count = if scope.starts_with("backend:") {
            state
                .attempts
                .values()
                .filter(|v| member(v, scope, &state.config))
                .count()
        } else {
            state
                .clients
                .iter()
                .filter(|(id, v)| *id != &r.id && member(v, scope, &state.config))
                .count()
        };
        if count >= *limit as usize {
            return Some(scope.clone());
        }
    }
    None
}
fn pin_key(r: &TrafficRequest, budget: &str) -> Option<String> {
    r.session
        .as_ref()
        .map(|s| format!("{}:{}:{budget}:{s}", r.identity.user, r.identity.key))
}

impl TrafficController {
    pub fn cancel_queued(&self, id: &str) -> bool {
        let mut s = self.inner.state.lock().unwrap();
        let removed = s.pending.remove(id).is_some();
        if removed {
            s.cancelled_tickets.insert(id.to_owned());
            s.cancelled += 1;
        }
        drop(s);
        self.inner.changed.notify_waiters();
        removed
    }
    pub fn hold_job(&self, id: String, lease: AttemptLease) {
        lease.set_state("backend_job");
        self.inner.jobs.lock().unwrap().insert(id, lease);
    }
    pub fn bind_job(&self, request_id: &str, job_id: &str) {
        let lease = self.inner.jobs.lock().unwrap().remove(request_id);
        if let Some(lease) = lease {
            self.inner
                .jobs
                .lock()
                .unwrap()
                .insert(job_id.to_owned(), lease);
        }
    }
    pub fn job_identity(&self, id: &str) -> Option<(Identity, Option<String>, usize)> {
        self.inner.jobs.lock().unwrap().get(id).map(|lease| {
            (
                lease.request.identity.clone(),
                lease.request.session.clone(),
                lease.request.bytes.load(Ordering::Relaxed),
            )
        })
    }
    pub fn finish_job(&self, id: &str) {
        let lease = self.inner.jobs.lock().unwrap().remove(id);
        if let Some(mut lease) = lease {
            lease.complete();
            drop(lease);
        }
    }
    pub fn restore_job(
        &self,
        id: &str,
        identity: Identity,
        model: String,
        session: Option<String>,
        bytes: usize,
        backend: &str,
    ) {
        if self.inner.jobs.lock().unwrap().contains_key(id) {
            return;
        }
        let request = TrafficRequest::new(identity, model, session, bytes);
        let lease = {
            let mut state = self.inner.state.lock().unwrap();
            let backend = backend.trim_end_matches('/');
            let policy = effective(&state.config, &request, backend, "", Utc::now());
            admit(
                &mut state,
                &self.inner,
                &request,
                id,
                backend,
                &policy,
                "restored",
                None,
            )
        };
        self.hold_job(id.to_owned(), lease);
    }
    pub fn replicas(&self, model: &str) -> Option<Vec<String>> {
        let s = self.inner.state.lock().unwrap();
        let p = s.config.model_policy(model);
        (s.config.mode == Mode::Enforce && (p.balance || p.affinity != Affinity::Off))
            .then_some(p.replicas)
    }
    pub fn new() -> Self {
        let this = Self::default();
        this.inner.state.lock().unwrap().ready = true;
        this
    }
    pub fn loading() -> Self {
        Self::default()
    }
    pub fn install(&self, config: TrafficConfig, revision: u64) -> Result<(), String> {
        config.validate()?;
        let mut state = self.inner.state.lock().unwrap();
        if state.config.models != config.models || state.config.backends != config.backends {
            state.pins.clear();
        }
        for view in state.clients.values_mut() {
            view.budget = config.budget(&view.model);
        }
        for view in state.attempts.values_mut() {
            view.budget = config.budget(&view.model);
        }
        state.config = config;
        state.revision = revision;
        state.ready = true;
        drop(state);
        self.inner.changed.notify_waiters();
        Ok(())
    }
    pub fn config(&self) -> (u64, TrafficConfig) {
        let s = self.inner.state.lock().unwrap();
        (s.revision, s.config.clone())
    }
    pub fn enabled(&self) -> bool {
        self.inner.state.lock().unwrap().config.mode != Mode::Off
    }
    pub fn snapshot(&self) -> TrafficSnapshot {
        let s = self.inner.state.lock().unwrap();
        TrafficSnapshot {
            completed_metrics: s.completed_metrics.iter().cloned().collect(),
            mode: s.config.mode,
            ready: s.ready,
            revision: s.revision,
            queued: s.pending.values().map(|p| p.view.clone()).collect(),
            active: s.attempts.values().cloned().collect(),
            clients: s.clients.len(),
            admitted: s.admitted,
            rejected: s.rejected,
            cancelled: s.cancelled,
            shadow_blocked: s.shadow_blocked,
            queued_bytes: s.pending.values().map(|p| p.view.bytes).sum(),
            reservations: s
                .reservations
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
    pub fn preview(
        &self,
        r: &TrafficRequest,
        backend: &str,
        provider: &str,
        now: chrono::DateTime<Utc>,
    ) -> EffectivePolicy {
        effective(
            &self.inner.state.lock().unwrap().config,
            r,
            backend,
            provider,
            now,
        )
    }
    pub fn reserve(&self, id: &str, backend: &str) -> AppResult<()> {
        let mut s = self.inner.state.lock().unwrap();
        if s.config.mode != Mode::Enforce {
            return Ok(());
        }
        let resource = resource(&s.config, backend.trim_end_matches('/'));
        if s.reservations.values().any(|v| v == &resource)
            || s.attempts
                .values()
                .any(|v| resource == crate::traffic::resource(&s.config, &v.backend))
        {
            return Err(AppError::conflict(
                "backend must finish active gateway requests before benchmarking",
            ));
        }
        s.reservations.insert(id.to_owned(), resource);
        drop(s);
        self.inner.changed.notify_waiters();
        Ok(())
    }
    pub fn unreserve(&self, id: &str) {
        self.inner
            .state
            .lock()
            .unwrap()
            .reservations
            .retain(|key, _| key != id && !key.starts_with(&format!("{id}/")));
        self.inner.changed.notify_waiters();
    }
    pub fn reserve_deployment(
        &self,
        id: &str,
        deployment: &str,
        already_running: bool,
    ) -> AppResult<()> {
        let mut s = self.inner.state.lock().unwrap();
        if s.config.mode != Mode::Enforce {
            return Ok(());
        }
        let mut targets: Vec<_> = s
            .config
            .backends
            .iter()
            .filter(|(_, b)| b.deployment_id.as_deref() == Some(deployment))
            .map(|(url, _)| resource(&s.config, url))
            .collect();
        targets.sort();
        targets.dedup();
        if targets.is_empty() {
            if !already_running {
                return Err(AppError::conflict(
                    "map backend.deployment_id before benchmarking with admission enabled",
                ));
            }
            targets.push("*".into());
        }
        if !already_running
            && targets.iter().any(|r| {
                s.attempts
                    .values()
                    .any(|v| r == &resource(&s.config, &v.backend))
                    || s.reservations
                        .iter()
                        .any(|(key, b)| !key.starts_with(&format!("{id}/")) && (b == r || b == "*"))
            })
        {
            return Err(AppError::conflict(
                "backend must finish active requests before benchmarking",
            ));
        }
        for target in targets {
            s.reservations.insert(format!("{id}/{target}"), target);
        }
        drop(s);
        self.inner.changed.notify_waiters();
        Ok(())
    }
    /// Candidate order only changes inside an explicit opt-in replica pool.
    pub fn order(&self, r: &TrafficRequest, candidates: &[(usize, String, String)]) -> Vec<usize> {
        let s = self.inner.state.lock().unwrap();
        let config = &s.config;
        let p = config.model_policy(&r.model);
        if config.mode != Mode::Enforce || (!p.balance && p.affinity == Affinity::Off) {
            return candidates.iter().map(|v| v.0).collect();
        }
        let mut pool: Vec<_> = candidates
            .iter()
            .filter(|(_, name, url)| {
                p.replicas.contains(name)
                    || p.replicas
                        .iter()
                        .any(|r| r.trim_end_matches('/') == url.trim_end_matches('/'))
            })
            .collect();
        if pool.is_empty() {
            return Vec::new();
        }
        let pin = pin_key(r, &config.budget(&r.model))
            .and_then(|key| s.pins.get(&key))
            .filter(|pin| pin.seen.elapsed() < Duration::from_secs(p.affinity_ttl_secs));
        pool.sort_by(|a, b| {
            let rank = |c: &&(usize, String, String)| {
                let sticky = p.affinity != Affinity::Off
                    && pin.is_some_and(|pin| pin.backend == c.2.trim_end_matches('/'));
                let active = s
                    .attempts
                    .values()
                    .filter(|v| {
                        resource(config, &v.backend) == resource(config, c.2.trim_end_matches('/'))
                    })
                    .count();
                let cap = config
                    .backends
                    .get(c.2.trim_end_matches('/'))
                    .and_then(|p| p.max_concurrent)
                    .unwrap_or(1)
                    .max(1);
                let normalized = active as f64 / cap as f64;
                (!sticky, if p.balance { normalized } else { c.0 as f64 })
            };
            rank(a)
                .partial_cmp(&rank(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if p.affinity == Affinity::Strict && pin.is_some() {
            // A failed/unavailable pinned replica must not silently move work.
            return pool
                .iter()
                .filter(|c| pin.is_some_and(|pin| pin.backend == c.2.trim_end_matches('/')))
                .map(|c| c.0)
                .collect();
        }
        pool.into_iter().map(|c| c.0).collect()
    }
    pub async fn acquire(
        &self,
        r: Arc<TrafficRequest>,
        backend: String,
        provider: String,
    ) -> AppResult<Option<AttemptLease>> {
        let backend = backend.trim_end_matches('/').to_owned();
        let id = uuid::Uuid::new_v4().to_string();
        let _ticket = Ticket {
            inner: Arc::downgrade(&self.inner),
            id: id.clone(),
        };
        let started = Instant::now();
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut s = self.inner.state.lock().unwrap();
                if s.cancelled_tickets.remove(&id) {
                    return Err(AppError::cancelled());
                }
                if !s.ready {
                    return Err(AppError::traffic(false, "traffic policy is loading"));
                }
                if s.config.mode == Mode::Off {
                    s.pending.remove(&id);
                    return Ok(None);
                }
                let policy = effective(&s.config, &r, &backend, &provider, Utc::now());
                let reason = blocked(&s, &r, &policy, &backend);
                if s.config.mode == Mode::Observe {
                    if reason.is_some() {
                        s.shadow_blocked += 1;
                    }
                    s.pending.remove(&id);
                    return Ok(Some(admit(
                        &mut s,
                        &self.inner,
                        &r,
                        &id,
                        &backend,
                        &policy,
                        "observe",
                        reason,
                    )));
                }
                let mut view = RequestView {
                    id: id.clone(),
                    key: r.identity.key.clone(),
                    user: r.identity.user.clone(),
                    groups: r.identity.groups.clone(),
                    model: r.model.clone(),
                    budget: policy.budget.clone(),
                    backend: backend.clone(),
                    priority: policy.priority,
                    created_at_ms: Utc::now().timestamp_millis(),
                    state: "queued".into(),
                    reason: reason.clone(),
                    bytes: r.bytes.load(Ordering::Relaxed),
                    affinity: "off".into(),
                };
                if let Some(old) = s.pending.get(&id) {
                    view.created_at_ms = old.view.created_at_ms;
                }
                let model_policy = s.config.model_policy(&r.model);
                let pinned = pin_key(&r, &policy.budget)
                    .and_then(|key| s.pins.get(&key))
                    .is_some_and(|pin| {
                        pin.backend == backend
                            && pin.seen.elapsed()
                                < Duration::from_secs(model_policy.affinity_ttl_secs)
                    });
                view.affinity = if pinned { "pinned" } else { "new" }.into();
                let flow = groups_for(&s.config, &r.identity)
                    .first()
                    .map(|g| format!("group:{g}"))
                    .unwrap_or_else(|| format!("user:{}", r.identity.user));
                let current_finish = s
                    .finish
                    .get(&flow)
                    .copied()
                    .unwrap_or(s.virtual_time)
                    .max(s.virtual_time)
                    + 1.0 / f64::from(policy.priority);
                let current_aged = started.elapsed().as_millis() as u64 >= s.config.queue.aging_ms;
                let other_eligible =
                    s.pending
                        .iter()
                        .filter(|(other, _)| *other != &id)
                        .any(|(_, pending)| {
                            let other_policy = effective(
                                &s.config,
                                &pending.request,
                                &pending.view.backend,
                                &pending.provider,
                                Utc::now(),
                            );
                            let other_finish = s
                                .finish
                                .get(&pending.flow)
                                .copied()
                                .unwrap_or(s.virtual_time)
                                .max(s.virtual_time)
                                + 1.0 / f64::from(other_policy.priority);
                            let aged = pending.started.elapsed().as_millis() as u64
                                >= s.config.queue.aging_ms;
                            let outranks = if aged && current_aged {
                                pending.started < started
                            } else if aged != current_aged {
                                aged
                            } else {
                                other_finish < current_finish
                                    || (other_finish == current_finish && pending.started < started)
                            };
                            resource(&s.config, &pending.view.backend)
                                == resource(&s.config, &backend)
                                && outranks
                                && blocked(
                                    &s,
                                    &pending.request,
                                    &other_policy,
                                    &pending.view.backend,
                                )
                                .is_none()
                        });
                if reason.is_none() && !other_eligible {
                    s.pending.remove(&id);
                    s.finish.insert(flow.clone(), current_finish);
                    if s.pending.is_empty() {
                        s.virtual_time = s.virtual_time.max(current_finish);
                    }
                    return Ok(Some(admit(
                        &mut s,
                        &self.inner,
                        &r,
                        &id,
                        &backend,
                        &policy,
                        &view.affinity,
                        None,
                    )));
                }
                let wait = if model_policy.affinity == Affinity::Prefer && pinned {
                    policy.max_wait_ms.min(model_policy.affinity_wait_ms)
                } else {
                    policy.max_wait_ms
                };
                if !s.config.queue.enabled || started.elapsed() >= Duration::from_millis(wait) {
                    s.rejected += 1;
                    let backend_block = reason.as_ref().is_some_and(|s| s.starts_with("backend"));
                    let error = AppError::traffic(
                        !backend_block,
                        reason.as_deref().unwrap_or("queue wait deadline reached"),
                    );
                    if model_policy.affinity == Affinity::Prefer && pinned && backend_block {
                        return Err(AppError::affinity_spillover());
                    }
                    return Err(error);
                }
                view.reason = reason.or_else(|| Some("fair scheduling".into()));
                if !s.pending.contains_key(&id) {
                    let q = &s.config.queue;
                    if s.pending.len() >= q.max_requests
                        || s.pending
                            .values()
                            .map(|p| p.view.bytes)
                            .sum::<usize>()
                            .saturating_add(r.bytes.load(Ordering::Relaxed))
                            > q.max_bytes
                        || s.pending
                            .values()
                            .filter(|p| p.view.key == r.identity.key)
                            .count()
                            >= q.per_key
                    {
                        s.rejected += 1;
                        return Err(AppError::traffic(true, "gateway queue is full"));
                    }
                    if s.finish.len() > 8192 {
                        let retain: std::collections::HashSet<_> =
                            s.pending.values().map(|p| p.flow.clone()).collect();
                        s.finish.retain(|k, _| retain.contains(k));
                    }
                    // Group fairness prevents gaining weight merely by minting keys.
                    let flow = groups_for(&s.config, &r.identity)
                        .first()
                        .map(|g| format!("group:{g}"))
                        .unwrap_or_else(|| format!("user:{}", r.identity.user));
                    s.pending.insert(
                        id.clone(),
                        Pending {
                            view,
                            started,
                            flow,
                            request: r.clone(),
                            provider: provider.clone(),
                        },
                    );
                } else {
                    s.pending.get_mut(&id).unwrap().view = view;
                }
            }
            tokio::select! { _=notified=>{},_=tokio::time::sleep(Duration::from_millis(50))=>{} }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn admit(
    s: &mut State,
    inner: &Arc<Inner>,
    r: &Arc<TrafficRequest>,
    id: &str,
    backend: &str,
    policy: &EffectivePolicy,
    affinity: &str,
    reason: Option<String>,
) -> AttemptLease {
    let view = RequestView {
        id: id.into(),
        key: r.identity.key.clone(),
        user: r.identity.user.clone(),
        groups: r.identity.groups.clone(),
        model: r.model.clone(),
        budget: policy.budget.clone(),
        backend: backend.into(),
        priority: policy.priority,
        created_at_ms: Utc::now().timestamp_millis(),
        state: "dispatching".into(),
        reason,
        bytes: r.bytes.load(Ordering::Relaxed),
        affinity: affinity.into(),
    };
    s.clients
        .entry(r.id.clone())
        .or_insert_with(|| view.clone());
    *r.owner.lock().unwrap() = Some(Arc::downgrade(inner));
    s.attempts.insert(id.into(), view);
    s.admitted += 1;
    AttemptLease {
        inner: Arc::downgrade(inner),
        id: id.into(),
        request: r.clone(),
        completed: false,
    }
}

pub fn encoded_size(value: &impl Serialize) -> usize {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    let _ = serde_json::to_writer(&mut counter, value);
    counter.0
}
pub fn hold_chat_stream(
    mut stream: crate::upstream::UpstreamStream,
    lease: Option<AttemptLease>,
) -> crate::upstream::UpstreamStream {
    use futures::StreamExt;
    let Some(mut lease) = lease else {
        return stream;
    };
    lease.awaiting();
    Box::pin(async_stream::stream! {
        while let Some(chunk)=stream.next().await {
            if let Some(metrics)=chunk.as_ref().ok().and_then(|c|c.metrics.as_ref()){lease.engine_metrics(metrics);}
            if chunk.as_ref().ok().is_some_and(|c| c.choices.iter().any(|c| c.delta.content.as_ref().is_some_and(|s| !s.is_empty()) || c.delta.reasoning_content.as_ref().is_some_and(|s| !s.is_empty()) || c.delta.tool_calls.as_ref().is_some_and(|calls| !calls.is_empty()))) { lease.streaming(); }
            yield chunk;
        }
        lease.complete();
    })
}
pub fn hold_proxy_response(
    response: reqwest::Response,
    lease: Option<AttemptLease>,
) -> reqwest::Response {
    use futures::StreamExt;
    let Some(mut lease) = lease else {
        return response;
    };
    let status = response.status();
    let headers = response.headers().clone();
    let version = response.version();
    lease.set_state("backend_response");
    let mut body = response.bytes_stream();
    let stream = async_stream::stream! {
        while let Some(chunk)=body.next().await {if status.is_success() && chunk.as_ref().is_ok_and(|b| !b.is_empty()) {lease.streaming();} yield chunk;}
        lease.complete();
    };
    let mut wrapped = axum::http::Response::new(reqwest::Body::wrap_stream(stream));
    *wrapped.status_mut() = status;
    *wrapped.headers_mut() = headers;
    *wrapped.version_mut() = version;
    reqwest::Response::from(wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(key: &str, model: &str) -> Arc<TrafficRequest> {
        TrafficRequest::new(
            Identity {
                key: key.into(),
                user: format!("user-{key}"),
                groups: vec!["team".into()],
            },
            model.into(),
            None,
            128,
        )
    }
    fn config() -> TrafficConfig {
        let mut c = TrafficConfig {
            mode: Mode::Enforce,
            ..Default::default()
        };
        c.backends.insert(
            "http://engine/v1".into(),
            BackendPolicy {
                max_concurrent: Some(1),
                ..Default::default()
            },
        );
        c
    }
    async fn acquire(
        t: &TrafficController,
        r: Arc<TrafficRequest>,
    ) -> AppResult<Option<AttemptLease>> {
        t.acquire(r, "http://engine/v1/".into(), "engine".into())
            .await
    }
    async fn queued(t: &TrafficController, n: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while t.snapshot().queued.len() != n {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn policy_roundtrip_timezone_and_dated_precedence() {
        let c:TrafficConfig=serde_json::from_value(serde_json::json!({"mode":"enforce","groups":{"team":{"priority":2,"max_concurrent":4,"shared_concurrent":6,"schedules":[{"timezone":"Europe/Amsterdam","weekdays":[1,2,3,4,5],"start":"09:00","end":"17:00","limits":{"max_concurrent":1}},{"timezone":"Europe/Amsterdam","dates":["2026-10-05"],"start":"00:00","end":"00:00","limits":{"max_concurrent":3}}]}}})).unwrap();
        c.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<TrafficConfig>(serde_json::to_value(&c).unwrap()).unwrap(),
            c
        );
        let r = request("a", "model");
        let monday = "2026-10-05T10:00:00Z".parse().unwrap();
        let p = effective(&c, &r, "http://engine/v1", "", monday);
        assert_eq!(p.priority, 2);
        assert!(p.limits.contains(&("key:a".into(), 3)));
        assert!(p.limits.contains(&("group:team".into(), 6)));
        let tuesday = "2026-10-06T10:00:00Z".parse().unwrap();
        assert!(
            effective(&c, &r, "", "", tuesday)
                .limits
                .contains(&("key:a".into(), 1))
        );
        let window = Window {
            timezone: "Europe/Amsterdam".into(),
            weekdays: vec![7],
            dates: vec![],
            start: "02:00".into(),
            end: "03:00".into(),
            limits: Limits::default(),
            shared_concurrent: None,
            shared_models: BTreeMap::new(),
        };
        assert!(window.active("2026-10-25T00:30:00Z".parse().unwrap()));
        assert!(window.active("2026-10-25T01:30:00Z".parse().unwrap()));
        let overnight = Window {
            weekdays: vec![1],
            start: "23:00".into(),
            end: "02:00".into(),
            ..window
        };
        assert!(overnight.active("2026-10-05T23:00:00Z".parse().unwrap()));
    }
    #[tokio::test]
    async fn opt_in_and_observation_do_not_reject() {
        let t = TrafficController::new();
        let mut c = config();
        c.mode = Mode::Off;
        c.defaults.limits.max_concurrent = Some(0);
        t.install(c.clone(), 1).unwrap();
        assert!(acquire(&t, request("a", "m")).await.unwrap().is_none());
        assert_eq!(t.snapshot().admitted, 0);
        c.mode = Mode::Observe;
        t.install(c, 2).unwrap();
        let lease = acquire(&t, request("a", "m")).await.unwrap();
        assert_eq!(t.snapshot().shadow_blocked, 1);
        drop(lease);
        assert_eq!(t.snapshot().clients, 0);
    }
    #[tokio::test]
    async fn aliases_and_urls_share_physical_and_group_budgets() {
        let t = TrafficController::new();
        let mut c = config();
        c.backends.get_mut("http://engine/v1").unwrap().resource = Some("physical".into());
        c.backends.insert(
            "http://second-url/v1".into(),
            BackendPolicy {
                resource: Some("physical".into()),
                max_concurrent: Some(1),
                ..Default::default()
            },
        );
        c.groups.insert(
            "team".into(),
            Rule {
                shared_concurrent: Some(1),
                ..Default::default()
            },
        );
        t.install(c, 1).unwrap();
        let lease = acquire(&t, request("a", "alias-one")).await.unwrap();
        let error = t
            .acquire(
                request("b", "alias-two"),
                "http://second-url/v1".into(),
                "other".into(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error.status.as_u16(), 429 | 503));
        drop(lease);
        assert!(acquire(&t, request("b", "alias-two")).await.is_ok());
    }
    #[tokio::test]
    async fn per_model_key_limits_and_shared_user_limits_are_distinct() {
        let t = TrafficController::new();
        let mut c = TrafficConfig {
            mode: Mode::Enforce,
            ..Default::default()
        };
        c.models.insert(
            "alias".into(),
            ModelPolicy {
                budget: Some("canonical".into()),
                ..Default::default()
            },
        );
        c.defaults.limits.models.insert("canonical".into(), 1);
        t.install(c.clone(), 1).unwrap();
        let lease = acquire(&t, request("a", "canonical")).await.unwrap();
        assert_eq!(
            acquire(&t, request("a", "alias"))
                .await
                .unwrap_err()
                .status
                .as_u16(),
            429
        );
        let b = acquire(&t, request("b", "alias")).await.unwrap();
        assert_eq!(t.snapshot().clients, 2);
        drop(b);
        drop(lease);
        c.users.insert(
            "shared".into(),
            Rule {
                shared_concurrent: Some(1),
                ..Default::default()
            },
        );
        t.install(c, 2).unwrap();
        let a = TrafficRequest::new(
            Identity {
                user: "shared".into(),
                key: "a".into(),
                groups: vec![],
            },
            "m".into(),
            None,
            1,
        );
        let b = TrafficRequest::new(
            Identity {
                user: "shared".into(),
                key: "b".into(),
                groups: vec![],
            },
            "m".into(),
            None,
            1,
        );
        let lease = acquire(&t, a).await.unwrap();
        assert_eq!(acquire(&t, b).await.unwrap_err().status.as_u16(), 429);
        drop(lease);
    }

    #[tokio::test]
    async fn sql_owner_group_membership_shares_budgets_across_keys() {
        let t = TrafficController::new();
        let mut c = config();
        c.backends.clear();
        c.group_members
            .insert("workers".into(), vec!["user:owner".into()]);
        c.groups.insert(
            "workers".into(),
            Rule {
                shared_concurrent: Some(1),
                ..Default::default()
            },
        );
        t.install(c, 1).unwrap();
        let r = |key: &str| {
            TrafficRequest::new(
                Identity {
                    key: key.into(),
                    user: "owner".into(),
                    groups: vec![],
                },
                "m".into(),
                None,
                1,
            )
        };
        let first = acquire(&t, r("a")).await.unwrap();
        assert_eq!(acquire(&t, r("b")).await.unwrap_err().status.as_u16(), 429);
        drop(first);
        assert!(acquire(&t, r("b")).await.is_ok());
    }
    #[tokio::test]
    async fn cancellation_grace_holds_only_dispatched_backend_capacity() {
        let t = TrafficController::new();
        let mut c = config();
        c.cancellation_grace_ms = 60;
        t.install(c, 1).unwrap();
        let first = acquire(&t, request("a", "m")).await.unwrap().unwrap();
        first.awaiting();
        drop(first);
        assert_eq!(t.snapshot().active[0].state, "cancelling");
        assert!(acquire(&t, request("b", "m")).await.is_err());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut second = acquire(&t, request("b", "m")).await.unwrap().unwrap();
        second.awaiting();
        second.complete();
        drop(second);
        assert!(t.snapshot().active.is_empty());
    }
    #[tokio::test]
    async fn aged_work_precedes_new_high_priority_work() {
        let t = TrafficController::new();
        let mut c = config();
        c.queue.enabled = true;
        c.queue.aging_ms = 40;
        c.keys.insert(
            "high".into(),
            Rule {
                limits: Limits {
                    priority: Some(100),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        t.install(c, 1).unwrap();
        let active = acquire(&t, request("active", "m")).await.unwrap();
        let t2 = t.clone();
        let old = tokio::spawn(async move { acquire(&t2, request("old", "m")).await });
        queued(&t, 1).await;
        tokio::time::sleep(Duration::from_millis(70)).await;
        let t2 = t.clone();
        let high = tokio::spawn(async move { acquire(&t2, request("high", "m")).await });
        queued(&t, 2).await;
        drop(active);
        let old = tokio::time::timeout(Duration::from_secs(2), old)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!high.is_finished());
        drop(old);
        drop(high.await.unwrap().unwrap());
    }
    #[tokio::test]
    async fn cancellation_removes_queue_metadata_and_releases_capacity() {
        let t = TrafficController::new();
        let mut c = config();
        c.queue.enabled = true;
        t.install(c, 1).unwrap();
        let active = acquire(&t, request("a", "m")).await.unwrap();
        let t2 = t.clone();
        let waiting = tokio::spawn(async move { acquire(&t2, request("b", "m")).await });
        queued(&t, 1).await;
        waiting.abort();
        let _ = waiting.await;
        queued(&t, 0).await;
        assert_eq!(t.snapshot().queued_bytes, 0);
        drop(active);
        assert_eq!(t.snapshot().clients, 0);
        assert!(acquire(&t, request("c", "m")).await.is_ok());
    }
    #[tokio::test]
    async fn priority_and_aging_progress_without_head_of_line_blocking() {
        let t = TrafficController::new();
        let mut c = config();
        c.queue.enabled = true;
        c.keys.insert(
            "high".into(),
            Rule {
                limits: Limits {
                    priority: Some(10),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        t.install(c, 1).unwrap();
        let active = acquire(&t, request("active", "m")).await.unwrap();
        let low_t = t.clone();
        let low = tokio::spawn(async move { acquire(&low_t, request("low", "m")).await });
        queued(&t, 1).await;
        let high_t = t.clone();
        let high = tokio::spawn(async move { acquire(&high_t, request("high", "m")).await });
        queued(&t, 2).await;
        drop(active);
        let high = tokio::time::timeout(Duration::from_secs(2), high)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!low.is_finished());
        drop(high);
        let low = tokio::time::timeout(Duration::from_secs(2), low)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(low);
        assert_eq!(t.snapshot().clients, 0);
    }
    #[tokio::test]
    async fn bounded_queue_and_runtime_edits_take_effect() {
        let t = TrafficController::new();
        let mut c = config();
        c.queue.enabled = true;
        c.queue.max_requests = 1;
        c.queue.max_bytes = 128;
        t.install(c.clone(), 1).unwrap();
        let active = acquire(&t, request("a", "m")).await.unwrap();
        let t2 = t.clone();
        let waiting = tokio::spawn(async move { acquire(&t2, request("b", "m")).await });
        queued(&t, 1).await;
        assert_eq!(
            acquire(&t, request("c", "m"))
                .await
                .unwrap_err()
                .status
                .as_u16(),
            429
        );
        c.backends
            .get_mut("http://engine/v1")
            .unwrap()
            .max_concurrent = Some(2);
        t.install(c, 2).unwrap();
        let second = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(t.snapshot().active.len(), 2);
        drop(active);
        drop(second);
    }
    #[tokio::test]
    async fn asynchronous_jobs_hold_and_restore_capacity_until_terminal() {
        let t = TrafficController::new();
        t.install(config(), 1).unwrap();
        let r = request("a", "video");
        let id = r.id.clone();
        let lease = acquire(&t, r).await.unwrap().unwrap();
        t.hold_job(id.clone(), lease);
        t.bind_job(&id, "video_job");
        assert!(acquire(&t, request("b", "video")).await.is_err());
        assert_eq!(t.snapshot().active[0].state, "backend_job");
        t.finish_job("video_job");
        assert_eq!(t.snapshot().clients, 0);
        t.restore_job(
            "restored",
            Identity {
                key: "a".into(),
                user: "u".into(),
                groups: vec![],
            },
            "video".into(),
            None,
            128,
            "http://engine/v1",
        );
        assert!(acquire(&t, request("b", "video")).await.is_err());
        t.finish_job("restored");
        assert!(acquire(&t, request("b", "video")).await.is_ok());
    }
    #[tokio::test]
    async fn successful_affinity_is_namespaced_and_optional() {
        let t = TrafficController::new();
        let mut c = config();
        c.models.insert(
            "m".into(),
            ModelPolicy {
                balance: true,
                affinity: Affinity::Strict,
                replicas: vec!["one".into(), "two".into()],
                ..Default::default()
            },
        );
        t.install(c, 1).unwrap();
        let r = TrafficRequest::new(
            Identity {
                key: "a".into(),
                user: "u".into(),
                groups: vec![],
            },
            "m".into(),
            Some("session-hash".into()),
            0,
        );
        let lease = t
            .acquire(r.clone(), "http://engine/v1".into(), "one".into())
            .await
            .unwrap()
            .unwrap();
        lease.streaming();
        drop(lease);
        let candidates = vec![
            (0, "two".into(), "http://other/v1".into()),
            (1, "one".into(), "http://engine/v1/".into()),
        ];
        assert_eq!(t.order(&r, &candidates), vec![1]);
        assert!(t.order(&r, &candidates[..1]).is_empty());
        let different_key = TrafficRequest::new(
            Identity {
                key: "b".into(),
                user: "u".into(),
                groups: vec![],
            },
            "m".into(),
            Some("session-hash".into()),
            0,
        );
        assert_eq!(t.order(&different_key, &candidates), vec![0, 1]);
    }
    #[tokio::test]
    async fn backend_benchmark_reservation_is_atomic_and_reversible() {
        let t = TrafficController::new();
        let mut c = config();
        c.backends
            .get_mut("http://engine/v1")
            .unwrap()
            .deployment_id = Some("deployment".into());
        t.install(c, 1).unwrap();
        let active = acquire(&t, request("a", "m")).await.unwrap();
        assert!(t.reserve_deployment("bench", "deployment", false).is_err());
        drop(active);
        t.reserve_deployment("bench", "deployment", false).unwrap();
        assert!(acquire(&t, request("a", "m")).await.is_err());
        t.unreserve("bench");
        assert!(acquire(&t, request("a", "m")).await.is_ok());
        assert!(t.reserve_deployment("bench", "unknown", false).is_err());
    }
    #[test]
    fn invalid_policy_is_rejected_before_installation() {
        let t = TrafficController::new();
        let mut c = config();
        c.backends.insert(
            "http://user:secret@host/v1".into(),
            BackendPolicy::default(),
        );
        assert!(t.install(c, 1).is_err());
        assert_eq!(t.config().0, 0);
    }
}

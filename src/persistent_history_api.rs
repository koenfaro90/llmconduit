//! Auth-gated durable inference history and its explicit administrative clear.
//!
//! Route registration lives inside the existing dashboard API router, so the
//! same `require_session`, `no-store`, CSP, and clickjacking protections apply.
//! This module never exposes users, settings, API-key rows, or secret digests.

use crate::control_plane_store::{
    EventRow, HistoryClearCounts, MetricSample, RequestListFilter, RequestSummary, SessionFacets,
    SessionTableFilter, SessionTablePage, UsageBucket, UsageFilter,
};
use crate::dashboard_auth::{AuthSession, DashboardAuth};
use crate::engine::Gateway;
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_REQUEST_LIMIT: i64 = 100;
pub const MAX_REQUEST_LIMIT: i64 = 500;
pub const DEFAULT_METRICS_LIMIT: usize = 1_000;
pub const MAX_METRICS_LIMIT: usize = 5_000;
pub const DEFAULT_USAGE_LIMIT: usize = 100;
pub const MAX_USAGE_LIMIT: usize = 500;
pub const DEFAULT_METRICS_WINDOW_MS: i64 = 24 * 60 * 60 * 1_000;
pub const DEFAULT_USAGE_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
const HISTORY_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_EVENTS: usize = 64;
const MAX_ID_BYTES: usize = 256;
const MAX_KEY_ID_BYTES: usize = 256;
const CLEAR_HISTORY_CONFIRMATION: &str = "DELETE ALL REQUESTS AND SESSIONS";

const _: () = {
    assert!(DEFAULT_REQUEST_LIMIT >= 1 && DEFAULT_REQUEST_LIMIT <= MAX_REQUEST_LIMIT);
    assert!(DEFAULT_METRICS_LIMIT >= 1 && DEFAULT_METRICS_LIMIT <= MAX_METRICS_LIMIT);
    assert!(DEFAULT_USAGE_LIMIT >= 1 && DEFAULT_USAGE_LIMIT <= MAX_USAGE_LIMIT);
    assert!(MAX_REQUEST_LIMIT <= 500);
    assert!(MAX_METRICS_LIMIT <= 5_000);
    assert!(DEFAULT_METRICS_WINDOW_MS > 0);
    assert!(DEFAULT_USAGE_WINDOW_MS > 0);
};

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HistoryRequestsQuery {
    /// Maximum rows to return; clamped to 1..=500, default 100.
    pub limit: Option<i64>,
    /// Case-insensitive literal substring across request, model, backend, client and session identifiers.
    pub q: Option<String>,
    pub status: Option<String>,
    pub model: Option<String>,
    pub backend: Option<String>,
    /// JSON arrays of exact model/provider names selected by the dashboard facet controls.
    pub model_values: Option<String>,
    pub model_exclude: Option<bool>,
    pub backend_values: Option<String>,
    pub backend_exclude: Option<bool>,
    pub protocol: Option<String>,
    /// Exact stable user id, typically selected by display name in the UI.
    pub user_id: Option<String>,
    /// Exact stable key id, typically selected by display name in the UI.
    pub virtual_key_id: Option<String>,
    /// JSON object of named `{include:[],exclude:[]}` categorical facets.
    pub facets: Option<String>,
    /// Optional inclusive start of the request creation range (epoch ms).
    pub since_ms: Option<i64>,
    /// Optional exclusive end of the request creation range (epoch ms).
    pub until_ms: Option<i64>,
    /// Both cursor fields must be supplied together; no time window is applied by default.
    pub before_ms: Option<i64>,
    pub before_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FacetValues {
    include: Vec<String>,
    exclude: Vec<String>,
}

fn parse_facets(raw: Option<String>, allowed: &[&str]) -> Result<Option<String>, ()> {
    let Some(raw) = raw else { return Ok(None) };
    if raw.len() > 32_768 {
        return Err(());
    }
    let facets: std::collections::BTreeMap<String, FacetValues> =
        serde_json::from_str(&raw).map_err(|_| ())?;
    if facets.len() > allowed.len()
        || facets.iter().any(|(name, values)| {
            !allowed.contains(&name.as_str())
                || values.include.len() > 64
                || values.exclude.len() > 64
                || values
                    .include
                    .iter()
                    .chain(&values.exclude)
                    .any(|value| value.is_empty() || value.len() > MAX_ID_BYTES)
                || values
                    .include
                    .iter()
                    .any(|value| values.exclude.contains(value))
        })
    {
        return Err(());
    }
    serde_json::to_string(&facets).map(Some).map_err(|_| ())
}

#[utoipa::path(
    get,
    path = "/dashboard/api/history/requests/facets",
    tag = "history",
    operation_id = "history_request_facets",
    responses((status = 200, body = crate::control_plane_store::RequestFacets))
)]
pub async fn history_request_facets(State(gateway): State<Arc<Gateway>>) -> Response {
    let Some(store) = gateway.persistence_store() else {
        return unavailable();
    };
    match tokio::time::timeout(HISTORY_QUERY_TIMEOUT, store.request_facets()).await {
        Ok(Ok(facets)) => json_response(StatusCode::OK, &facets),
        Ok(Err(error)) => internal_error("list request facets", &error),
        Err(_) => query_timeout(),
    }
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HistoryUsageQuery {
    /// Restrict to one virtual key by its stable database id (never the credential or its digest); trimmed, 1..=256 bytes.
    pub virtual_key_id: Option<String>,
    /// Restrict to one user id; trimmed, blank means no filter.
    pub user_id: Option<String>,
    /// Window start (epoch ms, must not be negative); default now − 30 days.
    pub since_ms: Option<i64>,
    /// Maximum buckets to return; clamped to 1..=500, default 100.
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HistoryMetricsQuery {
    /// Window start (epoch ms, must not be negative); default now − 24 hours.
    pub since_ms: Option<i64>,
    /// Maximum samples to return; clamped to 1..=5000, default 1000.
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct RequestsBody {
    /// Newest first.
    requests: Vec<RequestSummary>,
    /// The effective (clamped) limit that bounded the query.
    limit: i64,
    /// More rows match the current filters after this page.
    has_more: bool,
    /// Pass these two fields back to fetch the next page.
    next_before_ms: Option<i64>,
    next_before_id: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearHistoryRequest {
    /// Must exactly match `DELETE ALL REQUESTS AND SESSIONS`.
    confirm: String,
}

/// `POST /dashboard/api/history/clear` removes the durable inference-history
/// tables in one transaction. The dashboard session and CSRF token are both
/// checked here; the typed phrase is independently enforced server-side.
#[utoipa::path(
    post,
    path = "/dashboard/api/history/clear",
    tag = "history",
    operation_id = "clear_inference_history",
    request_body = ClearHistoryRequest,
    responses(
        (status = 200, description = "Deleted request and session counts.", body = HistoryClearCounts),
        (status = 400, description = "Confirmation phrase did not match."),
        (status = 401, description = "No valid dashboard session."),
        (status = 403, description = "Administrator role or CSRF token required."),
        (status = 503, description = "Persistent history is disabled.")
    )
)]
pub async fn clear_inference_history(
    State(gateway): State<Arc<Gateway>>,
    Extension(auth): Extension<Arc<DashboardAuth>>,
    session: AuthSession,
    headers: HeaderMap,
    Json(body): Json<ClearHistoryRequest>,
) -> Response {
    let admin = session
        .user
        .as_ref()
        .map_or_else(|| session.bootstrap_admin(), |user| user.is_admin);
    if !admin {
        return json_response(
            StatusCode::FORBIDDEN,
            &error_body("administrator role required"),
        );
    }
    if !auth.verify_csrf(&headers) {
        return json_response(
            StatusCode::FORBIDDEN,
            &error_body("missing or invalid CSRF token"),
        );
    }
    if body.confirm != CLEAR_HISTORY_CONFIRMATION {
        return bad_request("confirmation phrase did not match");
    }
    if gateway.persistence_store().is_none() {
        return unavailable();
    }
    let Some(queue) = gateway.persistence_queue() else {
        return unavailable();
    };
    match queue.clear_history().await {
        Ok(counts) => {
            gateway.session_linker().clear();
            gateway.session_hub().clear();
            tracing::warn!(
                requests = counts.requests,
                sessions = counts.sessions,
                "inference history cleared by dashboard administrator"
            );
            json_response(StatusCode::OK, &counts)
        }
        Err(error) => {
            tracing::error!(error, "durable inference history clear failed");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &error_body("durable inference history clear failed"),
            )
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct RequestDetailBody {
    request: RequestSummary,
    /// Bounded/redacted hop events, at most 64 (the newest are kept).
    events: Vec<EventRow>,
    /// `true` when more than 64 events exist and older ones were dropped.
    events_truncated: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct UsageBody {
    usage: Vec<UsageBucket>,
    /// Effective window start (epoch ms).
    since_ms: i64,
    /// The effective (clamped) limit.
    limit: usize,
    /// `true` when more buckets matched than `limit`.
    truncated: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct MetricsBody {
    /// The newest `limit` samples in the window, in chronological order.
    samples: Vec<MetricSample>,
    /// Effective window start (epoch ms).
    since_ms: i64,
    /// The effective (clamped) limit.
    limit: usize,
    /// `true` when more samples existed in the window than `limit`.
    truncated: bool,
}

/// Body of every history 500: `{"error": "persistent history read failed",
/// "operation": "<static label of the failed store operation>"}`.
// Documentation-only mirror of the literal built by `internal_error`, which
// still serializes `serde_json::json!` unchanged; keep the two in step.
#[allow(dead_code)]
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct HistoryReadFailed {
    /// Always `"persistent history read failed"`; the underlying error text stays in the log.
    error: String,
    /// Static label of the store operation that failed, e.g. `"list durable requests"`.
    operation: String,
}

/// `GET /dashboard/api/history/requests`. Newest first, with server-side
/// filters and a stable (timestamp, id) cursor. No default time window.
#[utoipa::path(
    get,
    path = "/dashboard/api/history/requests",
    tag = "history",
    operation_id = "history_requests",
    params(HistoryRequestsQuery),
    responses(
        (status = 200, description = "A filtered page of durable requests, with a next-page cursor.", body = RequestsBody),
        (status = 400, description = "Malformed query string (e.g. a non-integer `limit`), rejected by the query extractor; plain-text body.", body = String, content_type = "text/plain"),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 500, description = "Store read failed; `operation` is `list durable requests`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "The store query exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_requests(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<HistoryRequestsQuery>,
) -> Response {
    history_requests_from(gateway.persistence_store(), query).await
}

async fn history_requests_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    query: HistoryRequestsQuery,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    let limit = query
        .limit
        .unwrap_or(DEFAULT_REQUEST_LIMIT)
        .clamp(1, MAX_REQUEST_LIMIT);
    if query.before_ms.is_some() != query.before_id.is_some()
        || query.before_ms.is_some_and(|value| value < 0)
    {
        return bad_request("before_ms and before_id must be supplied together");
    }
    if query.since_ms.is_some_and(|value| value < 0)
        || query.until_ms.is_some_and(|value| value < 0)
        || query
            .since_ms
            .zip(query.until_ms)
            .is_some_and(|(since, until)| since >= until)
    {
        return bad_request("invalid request time range");
    }
    let valid = |value: &Option<String>| value.as_ref().is_none_or(|v| v.len() <= MAX_ID_BYTES);
    if !valid(&query.q)
        || !valid(&query.model)
        || !valid(&query.backend)
        || !valid(&query.protocol)
        || !valid(&query.before_id)
        || !valid(&query.status)
        || !valid(&query.user_id)
        || !valid(&query.virtual_key_id)
    {
        return bad_request("request filter exceeds 256 bytes");
    }
    let parse_values = |raw: Option<String>| -> Result<Option<String>, ()> {
        let Some(raw) = raw else {
            return Ok(None);
        };
        let values: Vec<String> = serde_json::from_str(&raw).map_err(|_| ())?;
        if values.len() > 64
            || values
                .iter()
                .any(|value| value.is_empty() || value.len() > MAX_ID_BYTES)
        {
            return Err(());
        }
        if values.is_empty() {
            Ok(None)
        } else {
            serde_json::to_string(&values).map(Some).map_err(|_| ())
        }
    };
    let model_values_json = match parse_values(query.model_values) {
        Ok(v) => v,
        Err(()) => return bad_request("invalid request facet values"),
    };
    let backend_values_json = match parse_values(query.backend_values) {
        Ok(v) => v,
        Err(()) => return bad_request("invalid request facet values"),
    };
    let facets_json = match parse_facets(
        query.facets,
        &[
            "status", "model", "provider", "protocol", "user", "key", "harness", "kind", "session",
        ],
    ) {
        Ok(value) => value,
        Err(()) => return bad_request("invalid request facets"),
    };
    let normalize = |value: Option<String>| {
        value
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let filter = RequestListFilter {
        search: normalize(query.q),
        status: normalize(query.status),
        model: normalize(query.model),
        backend: normalize(query.backend),
        model_values_json,
        model_exclude: query.model_exclude.unwrap_or(false),
        backend_values_json,
        backend_exclude: query.backend_exclude.unwrap_or(false),
        protocol: normalize(query.protocol),
        user_id: normalize(query.user_id),
        virtual_key_id: normalize(query.virtual_key_id),
        facets_json,
        since_ms: query.since_ms,
        until_ms: query.until_ms,
        before_ms: query.before_ms,
        before_id: query.before_id,
    };
    if filter.before_id.as_ref().is_some_and(|v| v.is_empty()) {
        return bad_request("before_id must not be empty");
    }
    match tokio::time::timeout(
        HISTORY_QUERY_TIMEOUT,
        store.list_requests_filtered(&filter, limit + 1),
    )
    .await
    {
        Ok(Ok(mut requests)) => {
            let has_more = requests.len() > limit as usize;
            requests.truncate(limit as usize);
            let (next_before_ms, next_before_id) = if has_more {
                requests.last().map_or((None, None), |row| {
                    (Some(row.created_at_ms), Some(row.id.clone()))
                })
            } else {
                (None, None)
            };
            json_response(
                StatusCode::OK,
                &RequestsBody {
                    requests,
                    limit,
                    has_more,
                    next_before_ms,
                    next_before_id,
                },
            )
        }
        Ok(Err(error)) => internal_error("list durable requests", &error),
        Err(_) => query_timeout(),
    }
}

/// `GET /dashboard/api/history/requests/:id`. Both the terminal aggregate and
/// its bounded/redacted four-hop event records are returned. Unknown ids are 404.
#[utoipa::path(
    get,
    path = "/dashboard/api/history/requests/{id}",
    tag = "history",
    operation_id = "history_request_detail",
    params(("id" = String, Path, description = "Request id (the stable `api_call_id`); 1..=256 bytes.")),
    responses(
        (status = 200, description = "The terminal aggregate plus its newest events (at most 64).", body = RequestDetailBody),
        (status = 400, description = "`invalid request id` (empty or longer than 256 bytes).", body = crate::openapi::DashboardError),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 404, description = "`request not found`.", body = crate::openapi::DashboardError),
        (status = 500, description = "Store read failed; `operation` is `read durable request` or `read durable request events`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "The store query exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_request_detail(
    State(gateway): State<Arc<Gateway>>,
    Path(id): Path<String>,
) -> Response {
    history_request_detail_from(gateway.persistence_store(), id).await
}

async fn history_request_detail_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    id: String,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return bad_request("invalid request id");
    }
    let request = match tokio::time::timeout(HISTORY_QUERY_TIMEOUT, store.get_request(&id)).await {
        Ok(Ok(Some(request))) => request,
        Ok(Ok(None)) => {
            return json_response(StatusCode::NOT_FOUND, &error_body("request not found"));
        }
        Ok(Err(error)) => return internal_error("read durable request", &error),
        Err(_) => return query_timeout(),
    };
    match tokio::time::timeout(
        HISTORY_QUERY_TIMEOUT,
        store.request_events_limited(&id, MAX_EVENTS.saturating_add(1)),
    )
    .await
    {
        Ok(Ok(mut events)) => {
            let events_truncated = events.len() > MAX_EVENTS;
            if events_truncated {
                events.drain(..events.len() - MAX_EVENTS);
            }
            json_response(
                StatusCode::OK,
                &RequestDetailBody {
                    request,
                    events,
                    events_truncated,
                },
            )
        }
        Ok(Err(error)) => internal_error("read durable request events", &error),
        Err(_) => query_timeout(),
    }
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HistoryBodyQuery {
    /// Which hop's request body to reassemble: `client_in` (default) or `upstream_out`.
    pub hop: Option<String>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HistoryThroughputQuery {
    /// Window start (epoch ms, must not be negative); default now − 1 hour.
    pub since_ms: Option<i64>,
    /// Bucket width in seconds, 1..=86400; default 60.
    pub bucket_secs: Option<i64>,
    /// Maximum buckets to return; clamped to 1..=10000, default 2000.
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct ThroughputBody {
    buckets: Vec<crate::control_plane_store::ThroughputBucket>,
    /// Effective window start (epoch ms).
    since_ms: i64,
    /// Bucket width in milliseconds (`bucket_secs * 1000`).
    bucket_ms: i64,
    /// The effective (clamped) limit.
    limit: usize,
    /// `true` when more buckets matched than `limit`.
    truncated: bool,
}

const DEFAULT_THROUGHPUT_WINDOW_MS: i64 = 60 * 60 * 1_000;
const DEFAULT_THROUGHPUT_BUCKET_SECS: i64 = 60;
const MAX_THROUGHPUT_BUCKET_SECS: i64 = 24 * 60 * 60;
const DEFAULT_THROUGHPUT_LIMIT: usize = 2_000;
const MAX_THROUGHPUT_LIMIT: usize = 10_000;

/// `GET /dashboard/api/history/throughput?since_ms=&bucket_secs=&limit=`.
/// Gateway-side per-model/backend request + token series with the TTFT and
/// decode-time sums a consumer needs for prefill/decode throughput.
#[utoipa::path(
    get,
    path = "/dashboard/api/history/throughput",
    tag = "history",
    operation_id = "history_throughput",
    params(HistoryThroughputQuery),
    responses(
        (status = 200, description = "Per-(bucket, model, backend) request and token series.", body = ThroughputBody),
        (status = 400, description = "`since_ms must not be negative` or `bucket_secs must be between 1 and 86400` (JSON); or a malformed query string rejected by the query extractor (plain text).", body = crate::openapi::DashboardError),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 500, description = "Store read failed; `operation` is `read durable throughput series`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "The store query exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_throughput(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<HistoryThroughputQuery>,
) -> Response {
    history_throughput_from(gateway.persistence_store(), query).await
}

async fn history_throughput_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    query: HistoryThroughputQuery,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    let since_ms = match query.since_ms {
        Some(value) if value < 0 => return bad_request("since_ms must not be negative"),
        Some(value) => value,
        None => now_ms().saturating_sub(DEFAULT_THROUGHPUT_WINDOW_MS),
    };
    let bucket_secs = query.bucket_secs.unwrap_or(DEFAULT_THROUGHPUT_BUCKET_SECS);
    if !(1..=MAX_THROUGHPUT_BUCKET_SECS).contains(&bucket_secs) {
        return bad_request("bucket_secs must be between 1 and 86400");
    }
    let bucket_ms = bucket_secs * 1_000;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_THROUGHPUT_LIMIT)
        .clamp(1, MAX_THROUGHPUT_LIMIT);
    match tokio::time::timeout(
        HISTORY_QUERY_TIMEOUT,
        store.throughput_series(since_ms, bucket_ms, limit.saturating_add(1)),
    )
    .await
    {
        Ok(Ok(mut buckets)) => {
            let truncated = buckets.len() > limit;
            buckets.truncate(limit);
            json_response(
                StatusCode::OK,
                &ThroughputBody {
                    buckets,
                    since_ms,
                    bucket_ms,
                    limit,
                    truncated,
                },
            )
        }
        Ok(Err(error)) => internal_error("read durable throughput series", &error),
        Err(_) => query_timeout(),
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct ActivityBody {
    buckets: Vec<crate::control_plane_store::ActivityBucket>,
    /// Effective window start (epoch ms).
    since_ms: i64,
    /// Bucket width in milliseconds (`bucket_secs * 1000`).
    bucket_ms: i64,
    /// The effective (clamped) limit.
    limit: usize,
    /// `true` when more buckets matched than `limit`.
    truncated: bool,
}

/// `GET /dashboard/api/history/activity?since_ms=&bucket_secs=&limit=`.
/// Per-user / per-key request and token series (the activity dashboard).
#[utoipa::path(
    get,
    path = "/dashboard/api/history/activity",
    tag = "history",
    operation_id = "history_activity",
    params(HistoryThroughputQuery),
    responses(
        (status = 200, description = "Per-(bucket, user, virtual key) request and token series.", body = ActivityBody),
        (status = 400, description = "`since_ms must not be negative` or `bucket_secs must be between 1 and 86400` (JSON); or a malformed query string rejected by the query extractor (plain text).", body = crate::openapi::DashboardError),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 500, description = "Store read failed; `operation` is `read durable activity series`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "The store query exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_activity(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<HistoryThroughputQuery>,
) -> Response {
    history_activity_from(gateway.persistence_store(), query).await
}

async fn history_activity_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    query: HistoryThroughputQuery,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    let since_ms = match query.since_ms {
        Some(value) if value < 0 => return bad_request("since_ms must not be negative"),
        Some(value) => value,
        None => now_ms().saturating_sub(DEFAULT_THROUGHPUT_WINDOW_MS),
    };
    let bucket_secs = query.bucket_secs.unwrap_or(DEFAULT_THROUGHPUT_BUCKET_SECS);
    if !(1..=MAX_THROUGHPUT_BUCKET_SECS).contains(&bucket_secs) {
        return bad_request("bucket_secs must be between 1 and 86400");
    }
    let bucket_ms = bucket_secs * 1_000;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_THROUGHPUT_LIMIT)
        .clamp(1, MAX_THROUGHPUT_LIMIT);
    match tokio::time::timeout(
        HISTORY_QUERY_TIMEOUT,
        store.activity_series(since_ms, bucket_ms, limit.saturating_add(1)),
    )
    .await
    {
        Ok(Ok(mut buckets)) => {
            let truncated = buckets.len() > limit;
            buckets.truncate(limit);
            json_response(
                StatusCode::OK,
                &ActivityBody {
                    buckets,
                    since_ms,
                    bucket_ms,
                    limit,
                    truncated,
                },
            )
        }
        Ok(Err(error)) => internal_error("read durable activity series", &error),
        Err(_) => query_timeout(),
    }
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HistorySessionsQuery {
    /// Window start on `last_seen_ms` (epoch ms, must not be negative); default now − 7 days.
    pub since_ms: Option<i64>,
    /// Maximum sessions to return; clamped to 1..=500, default 100.
    pub limit: Option<usize>,
    /// `true` (default) lists only top-level nodes.
    pub roots: Option<bool>,
    /// Keyset cursor from the previous page's `next_before_ms`.
    pub before_ms: Option<i64>,
    /// Keyset cursor from the previous page's `next_before_id`.
    pub before_id: Option<String>,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HistorySessionTableQuery {
    pub q: Option<String>,
    pub user_id: Option<String>,
    pub virtual_key_id: Option<String>,
    pub harness: Option<String>,
    pub kind: Option<String>,
    /// JSON object of named `{include:[],exclude:[]}` categorical facets.
    pub facets: Option<String>,
    pub first_since_ms: Option<i64>,
    pub first_until_ms: Option<i64>,
    pub last_since_ms: Option<i64>,
    pub last_until_ms: Option<i64>,
    pub min_requests: Option<i64>,
    pub max_requests: Option<i64>,
    pub min_children: Option<i64>,
    pub max_children: Option<i64>,
    pub min_input_tokens: Option<i64>,
    pub max_input_tokens: Option<i64>,
    pub min_output_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub min_in_flight: Option<i64>,
    pub max_in_flight: Option<i64>,
    pub sort_by: Option<String>,
    pub descending: Option<bool>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/dashboard/api/history/sessions/table",
    tag = "history",
    operation_id = "history_session_table",
    params(HistorySessionTableQuery),
    responses((status = 200, body = SessionTablePage))
)]
pub async fn history_session_table(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<HistorySessionTableQuery>,
) -> Response {
    let Some(store) = gateway.persistence_store() else {
        return unavailable();
    };
    let valid = |value: &Option<String>| value.as_ref().is_none_or(|v| v.len() <= MAX_ID_BYTES);
    if ![
        &query.q,
        &query.user_id,
        &query.virtual_key_id,
        &query.harness,
        &query.kind,
    ]
    .into_iter()
    .all(valid)
        || query
            .facets
            .as_ref()
            .is_some_and(|value| value.len() > 32_768)
        || query.offset.is_some_and(|v| v < 0)
        || [
            query.first_since_ms,
            query.first_until_ms,
            query.last_since_ms,
            query.last_until_ms,
            query.min_requests,
            query.max_requests,
            query.min_children,
            query.max_children,
            query.min_input_tokens,
            query.max_input_tokens,
            query.min_output_tokens,
            query.max_output_tokens,
            query.min_in_flight,
            query.max_in_flight,
        ]
        .into_iter()
        .flatten()
        .any(|v| v < 0)
        || [
            (query.min_requests, query.max_requests),
            (query.min_children, query.max_children),
            (query.min_input_tokens, query.max_input_tokens),
            (query.min_output_tokens, query.max_output_tokens),
            (query.min_in_flight, query.max_in_flight),
        ]
        .into_iter()
        .any(|(min, max)| min.zip(max).is_some_and(|(min, max)| min > max))
    {
        return bad_request("invalid session table filter");
    }
    let normalize =
        |value: Option<String>| value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let facets_json = match parse_facets(query.facets, &["user", "key", "harness", "kind"]) {
        Ok(value) => value,
        Err(()) => return bad_request("invalid session facets"),
    };
    let filter = SessionTableFilter {
        search: normalize(query.q),
        user_id: normalize(query.user_id),
        virtual_key_id: normalize(query.virtual_key_id),
        harness: normalize(query.harness),
        kind: normalize(query.kind),
        facets_json,
        first_since_ms: query.first_since_ms,
        first_until_ms: query.first_until_ms,
        last_since_ms: query.last_since_ms,
        last_until_ms: query.last_until_ms,
        min_requests: query.min_requests,
        max_requests: query.max_requests,
        min_children: query.min_children,
        max_children: query.max_children,
        min_input_tokens: query.min_input_tokens,
        max_input_tokens: query.max_input_tokens,
        min_output_tokens: query.min_output_tokens,
        max_output_tokens: query.max_output_tokens,
        min_in_flight: query.min_in_flight,
        max_in_flight: query.max_in_flight,
        sort_by: query.sort_by.unwrap_or_else(|| "last".to_owned()),
        descending: query.descending.unwrap_or(true),
        offset: query.offset.unwrap_or(0),
        limit: query.limit.unwrap_or(100).clamp(1, 500),
    };
    match tokio::time::timeout(HISTORY_QUERY_TIMEOUT, store.list_session_table(&filter)).await {
        Ok(Ok(page)) => json_response(StatusCode::OK, &page),
        Ok(Err(error)) => internal_error("list session table", &error),
        Err(_) => query_timeout(),
    }
}

#[utoipa::path(
    get,
    path = "/dashboard/api/history/sessions/facets",
    tag = "history",
    operation_id = "history_session_facets",
    responses((status = 200, body = SessionFacets))
)]
pub async fn history_session_facets(State(gateway): State<Arc<Gateway>>) -> Response {
    let Some(store) = gateway.persistence_store() else {
        return unavailable();
    };
    match tokio::time::timeout(HISTORY_QUERY_TIMEOUT, store.session_facets()).await {
        Ok(Ok(facets)) => json_response(StatusCode::OK, &facets),
        Ok(Err(error)) => internal_error("session facets", &error),
        Err(_) => query_timeout(),
    }
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct HistorySessionQuery {
    /// Maximum requests to include; clamped to 1..=1000, default 200.
    pub limit: Option<usize>,
    /// Keyset cursor from the previous page's `next_before_ms`.
    pub before_ms: Option<i64>,
    /// Keyset cursor from the previous page's `next_before_id`.
    pub before_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct SessionNode {
    #[serde(flatten)]
    session: crate::sessions::SessionRow,
    /// Number of direct child sessions; grandchildren are counted on their own node.
    child_count: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct SessionsBody {
    /// Most recently active first.
    sessions: Vec<SessionNode>,
    /// Effective window start (epoch ms).
    since_ms: i64,
    /// The effective (clamped) limit.
    limit: usize,
    /// `true` when more sessions matched than `limit`.
    truncated: bool,
    next_before_ms: Option<i64>,
    next_before_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub(crate) struct SessionDetailBody {
    session: SessionNode,
    /// Parent chain, nearest first; at most 32 entries.
    ancestors: Vec<crate::sessions::SessionRow>,
    /// Direct child nodes.
    children: Vec<SessionNode>,
    /// A page of this node's requests, oldest first within the page.
    requests: Vec<RequestSummary>,
    /// `true` when older requests remain available on another page.
    requests_truncated: bool,
    next_before_ms: Option<i64>,
    next_before_id: Option<String>,
}

/// Default lookback for the sessions list.
const DEFAULT_SESSIONS_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const DEFAULT_SESSIONS_LIMIT: usize = 100;
const MAX_SESSIONS_LIMIT: usize = 500;
const DEFAULT_SESSION_REQUESTS: usize = 200;
const MAX_SESSION_REQUESTS: usize = 1_000;
const MAX_ANCESTORS: usize = 32;

/// `GET /dashboard/api/history/sessions?since_ms=&limit=&roots=`. The most
/// recently active session nodes; top-level only unless `roots=false`.
#[utoipa::path(
    get,
    path = "/dashboard/api/history/sessions",
    tag = "history",
    operation_id = "history_sessions",
    params(HistorySessionsQuery),
    responses(
        (status = 200, description = "The most recently active session nodes in the window.", body = SessionsBody),
        (status = 400, description = "`since_ms must not be negative` (JSON); or a malformed query string rejected by the query extractor (plain text).", body = crate::openapi::DashboardError),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 500, description = "Store read failed; `operation` is `read durable sessions`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "The store query exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_sessions(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<HistorySessionsQuery>,
) -> Response {
    history_sessions_from(gateway.persistence_store(), query).await
}

async fn history_sessions_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    query: HistorySessionsQuery,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    let since_ms = match query.since_ms {
        Some(value) if value < 0 => return bad_request("since_ms must not be negative"),
        Some(value) => value,
        None => now_ms().saturating_sub(DEFAULT_SESSIONS_WINDOW_MS),
    };
    let limit = query
        .limit
        .unwrap_or(DEFAULT_SESSIONS_LIMIT)
        .clamp(1, MAX_SESSIONS_LIMIT);
    let roots_only = query.roots.unwrap_or(true);
    if query.before_ms.is_some() != query.before_id.is_some()
        || query.before_ms.is_some_and(|value| value < 0)
        || query
            .before_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_ID_BYTES)
    {
        return bad_request("before_ms and before_id must be supplied together and valid");
    }
    let read = async {
        let mut sessions = store
            .list_sessions(
                since_ms,
                roots_only,
                query.before_ms,
                query.before_id.as_deref(),
                limit.saturating_add(1),
            )
            .await?;
        let truncated = sessions.len() > limit;
        sessions.truncate(limit);
        let (next_before_ms, next_before_id) = if truncated {
            sessions.last().map_or((None, None), |last| {
                (Some(last.last_seen_ms), Some(last.id.clone()))
            })
        } else {
            (None, None)
        };
        let ids = sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        let child_counts = store.session_child_counts(&ids).await?;
        let sessions = sessions
            .into_iter()
            .map(|session| SessionNode {
                child_count: *child_counts.get(&session.id).unwrap_or(&0),
                session,
            })
            .collect();
        Ok::<_, String>(SessionsBody {
            sessions,
            since_ms,
            limit,
            truncated,
            next_before_ms,
            next_before_id,
        })
    };
    match tokio::time::timeout(HISTORY_QUERY_TIMEOUT, read).await {
        Ok(Ok(body)) => json_response(StatusCode::OK, &body),
        Ok(Err(error)) => internal_error("read durable sessions", &error),
        Err(_) => query_timeout(),
    }
}

/// `GET /dashboard/api/history/sessions/:id?limit=&before_ms=&before_id=`.
/// One node with its ancestors (nearest first), direct children, and a page of requests.
#[utoipa::path(
    get,
    path = "/dashboard/api/history/sessions/{id}",
    tag = "history",
    operation_id = "history_session_detail",
    params(
        ("id" = String, Path, description = "Session node id; 1..=256 bytes."),
        HistorySessionQuery,
    ),
    responses(
        (status = 200, description = "The node with its ancestors (nearest first, at most 32), direct children, and a page of requests.", body = SessionDetailBody),
        (status = 400, description = "Invalid session id or cursor (JSON); or a malformed query string rejected by the query extractor (plain text).", body = crate::openapi::DashboardError),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 404, description = "`session not found`.", body = crate::openapi::DashboardError),
        (status = 500, description = "Store read failed; `operation` is `read durable session`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "The combined store reads exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_session_detail(
    State(gateway): State<Arc<Gateway>>,
    Path(id): Path<String>,
    Query(query): Query<HistorySessionQuery>,
) -> Response {
    history_session_detail_from(gateway.persistence_store(), id, query).await
}

async fn history_session_detail_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    id: String,
    query: HistorySessionQuery,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return bad_request("invalid session id");
    }
    if query.before_ms.is_some() != query.before_id.is_some()
        || query.before_ms.is_some_and(|value| value < 0)
        || query
            .before_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_ID_BYTES)
    {
        return bad_request("before_ms and before_id must be supplied together and valid");
    }
    let limit = query
        .limit
        .unwrap_or(DEFAULT_SESSION_REQUESTS)
        .clamp(1, MAX_SESSION_REQUESTS);
    let detail = async {
        let Some(session) = store.get_session(&id).await? else {
            return Ok::<_, String>(None);
        };
        let mut ancestors = Vec::new();
        let mut cursor = session.parent_id.clone();
        while let Some(parent_id) = cursor.take() {
            if ancestors.len() >= MAX_ANCESTORS {
                break;
            }
            let Some(parent) = store.get_session(&parent_id).await? else {
                break;
            };
            cursor = parent.parent_id.clone();
            ancestors.push(parent);
        }
        let children = store.session_children(&id).await?;
        let child_ids = children
            .iter()
            .map(|child| child.id.clone())
            .collect::<Vec<_>>();
        let child_counts = store.session_child_counts(&child_ids).await?;
        let child_count = i64::try_from(children.len()).unwrap_or(i64::MAX);
        let children = children
            .into_iter()
            .map(|child| SessionNode {
                child_count: *child_counts.get(&child.id).unwrap_or(&0),
                session: child,
            })
            .collect();
        let mut requests = store
            .session_requests(
                &id,
                query.before_ms,
                query.before_id.as_deref(),
                limit.saturating_add(1),
            )
            .await?;
        let requests_truncated = requests.len() > limit;
        if requests_truncated {
            requests.drain(..requests.len() - limit);
        }
        let (next_before_ms, next_before_id) = if requests_truncated {
            requests.first().map_or((None, None), |first| {
                (Some(first.created_at_ms), Some(first.id.clone()))
            })
        } else {
            (None, None)
        };
        Ok(Some(SessionDetailBody {
            session: SessionNode {
                session,
                child_count,
            },
            ancestors,
            children,
            requests,
            requests_truncated,
            next_before_ms,
            next_before_id,
        }))
    };
    match tokio::time::timeout(HISTORY_QUERY_TIMEOUT, detail).await {
        Ok(Ok(Some(body))) => json_response(StatusCode::OK, &body),
        Ok(Ok(None)) => json_response(StatusCode::NOT_FOUND, &error_body("session not found")),
        Ok(Err(error)) => internal_error("read durable session", &error),
        Err(_) => query_timeout(),
    }
}

/// `GET /dashboard/api/history/requests/:id/body?hop=client_in|upstream_out`.
/// Reassembles the full request body of one hop from its skeleton event and
/// content-addressed items. The response is the body itself (JSON), not an
/// envelope. 404 when the request or its hop body is not stored.
#[utoipa::path(
    get,
    path = "/dashboard/api/history/requests/{id}/body",
    tag = "history",
    operation_id = "history_request_body",
    params(
        ("id" = String, Path, description = "Request id (the stable `api_call_id`); 1..=256 bytes."),
        HistoryBodyQuery,
    ),
    responses(
        (status = 200, description = "The reassembled (secret-redacted) request body of the selected hop, not an envelope. Its shape is the client protocol's request for `client_in` and a chat-completions request for `upstream_out`.", body = serde_json::Value, content_type = "application/json"),
        (status = 400, description = "`invalid request id` (empty or longer than 256 bytes) or `hop must be client_in or upstream_out` (JSON); or a malformed query string rejected by the query extractor (plain text).", body = crate::openapi::DashboardError),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 404, description = "`request not found`, or `request body not stored` when the hop's skeleton event is missing or has no content.", body = crate::openapi::DashboardError),
        (status = 500, description = "Store read or reassembly failed; `operation` is one of `read durable request`, `read durable request events`, `read durable request items`, `read durable content blobs`, `assemble durable request body`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "A store query exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_request_body(
    State(gateway): State<Arc<Gateway>>,
    Path(id): Path<String>,
    Query(query): Query<HistoryBodyQuery>,
) -> Response {
    history_request_body_from(gateway.persistence_store(), id, query).await
}

async fn history_request_body_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    id: String,
    query: HistoryBodyQuery,
) -> Response {
    use crate::flow_persistence::PayloadSection;

    let Some(store) = store else {
        return unavailable();
    };
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return bad_request("invalid request id");
    }
    let section = match query.hop.as_deref().unwrap_or("client_in") {
        "client_in" => PayloadSection::InboundRequest,
        "upstream_out" => PayloadSection::UpstreamRequest,
        _ => return bad_request("hop must be client_in or upstream_out"),
    };
    let request = match tokio::time::timeout(HISTORY_QUERY_TIMEOUT, store.get_request(&id)).await {
        Ok(Ok(Some(request))) => request,
        Ok(Ok(None)) => {
            return json_response(StatusCode::NOT_FOUND, &error_body("request not found"));
        }
        Ok(Err(error)) => return internal_error("read durable request", &error),
        Err(_) => return query_timeout(),
    };
    let protocol = match section {
        PayloadSection::InboundRequest => request.client_protocol.clone(),
        _ => crate::content_store::PROTOCOL_CHAT_COMPLETIONS.to_string(),
    };
    let events = match tokio::time::timeout(
        HISTORY_QUERY_TIMEOUT,
        store.request_events_limited(&id, MAX_EVENTS),
    )
    .await
    {
        Ok(Ok(events)) => events,
        Ok(Err(error)) => return internal_error("read durable request events", &error),
        Err(_) => return query_timeout(),
    };
    let Some(skeleton_event) = events.into_iter().find(|event| event.seq == section.seq()) else {
        return json_response(
            StatusCode::NOT_FOUND,
            &error_body("request body not stored"),
        );
    };
    let skeleton = match skeleton_event
        .payload
        .as_deref()
        .and_then(|payload| serde_json::from_str::<serde_json::Value>(payload).ok())
        .and_then(|envelope| {
            envelope
                .get("content")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        }) {
        Some(skeleton) => skeleton,
        None => {
            return json_response(
                StatusCode::NOT_FOUND,
                &error_body("request body not stored"),
            );
        }
    };
    let items = match tokio::time::timeout(
        HISTORY_QUERY_TIMEOUT,
        store.request_items(&id, section.hop()),
    )
    .await
    {
        Ok(Ok(items)) => items,
        Ok(Err(error)) => return internal_error("read durable request items", &error),
        Err(_) => return query_timeout(),
    };
    let hashes = items
        .iter()
        .map(|item| item.blob_hash.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let blobs = match tokio::time::timeout(HISTORY_QUERY_TIMEOUT, store.get_blobs(&hashes)).await {
        Ok(Ok(blobs)) => blobs,
        Ok(Err(error)) => return internal_error("read durable content blobs", &error),
        Err(_) => return query_timeout(),
    };
    let lookup = blobs
        .into_iter()
        .map(|blob| (blob.hash, blob.content))
        .collect::<std::collections::HashMap<_, _>>();
    // A body may reference the same blob at several positions, so resolve by
    // clone rather than by removal.
    match crate::content_store::assemble(&protocol, &skeleton, |hash| lookup.get(hash).cloned()) {
        Ok(body) => {
            let response = (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response();
            crate::dashboard_auth::no_store(response)
        }
        Err(error) => {
            let error = error.to_string();
            internal_error("assemble durable request body", &error)
        }
    }
}

/// `GET /dashboard/api/history/usage?virtual_key_id=&since_ms=`. The key filter
/// is a stable database id, never a presented credential or its digest.
#[utoipa::path(
    get,
    path = "/dashboard/api/history/usage",
    tag = "history",
    operation_id = "history_usage",
    params(HistoryUsageQuery),
    responses(
        (status = 200, description = "Usage buckets grouped by user, virtual key, alias, resolved model and backend within the window.", body = UsageBody),
        (status = 400, description = "`invalid virtual_key_id` (blank after trimming or longer than 256 bytes) or `since_ms must not be negative` (JSON); or a malformed query string rejected by the query extractor (plain text).", body = crate::openapi::DashboardError),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 500, description = "Store read failed; `operation` is `read durable usage`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "The store query exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_usage(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<HistoryUsageQuery>,
) -> Response {
    history_usage_from(gateway.persistence_store(), query).await
}

async fn history_usage_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    query: HistoryUsageQuery,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    let virtual_key_id = match query.virtual_key_id {
        Some(value) => {
            let value = value.trim();
            if value.is_empty() || value.len() > MAX_KEY_ID_BYTES {
                return bad_request("invalid virtual_key_id");
            }
            Some(value.to_string())
        }
        None => None,
    };
    if query.since_ms.is_some_and(|since| since < 0) {
        return bad_request("since_ms must not be negative");
    }
    let limit = query
        .limit
        .unwrap_or(DEFAULT_USAGE_LIMIT)
        .clamp(1, MAX_USAGE_LIMIT);
    let since_ms = query
        .since_ms
        .unwrap_or_else(|| now_ms().saturating_sub(DEFAULT_USAGE_WINDOW_MS));
    match tokio::time::timeout(
        HISTORY_QUERY_TIMEOUT,
        store.usage_summary_limited(
            &UsageFilter {
                virtual_key_id,
                user_id: query
                    .user_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                since_ms: Some(since_ms),
            },
            limit.saturating_add(1),
        ),
    )
    .await
    {
        Ok(Ok(mut usage)) => {
            let truncated = usage.len() > limit;
            usage.truncate(limit);
            json_response(
                StatusCode::OK,
                &UsageBody {
                    usage,
                    since_ms,
                    limit,
                    truncated,
                },
            )
        }
        Ok(Err(error)) => internal_error("read durable usage", &error),
        Err(_) => query_timeout(),
    }
}

/// `GET /dashboard/api/history/metrics?since_ms=&limit=`. The store query is
/// time-bounded and the response is row-bounded. The newest `limit` samples in
/// the selected window are returned in chronological order.
#[utoipa::path(
    get,
    path = "/dashboard/api/history/metrics",
    tag = "history",
    operation_id = "history_metrics",
    params(HistoryMetricsQuery),
    responses(
        (status = 200, description = "The newest `limit` backend metric samples in the window, in chronological order.", body = MetricsBody),
        (status = 400, description = "`since_ms must not be negative` (JSON); or a malformed query string rejected by the query extractor (plain text).", body = crate::openapi::DashboardError),
        (status = 401, description = "No valid dashboard session; plain-text body `unauthorized`.", body = String, content_type = "text/plain"),
        (status = 500, description = "Store read failed; `operation` is `read durable metric history`.", body = HistoryReadFailed),
        (status = 503, description = "Persistent history is disabled (no SQL store configured).", body = crate::openapi::DashboardError),
        (status = 504, description = "The store query exceeded the 5 s history timeout.", body = crate::openapi::DashboardError),
    )
)]
pub async fn history_metrics(
    State(gateway): State<Arc<Gateway>>,
    Query(query): Query<HistoryMetricsQuery>,
) -> Response {
    history_metrics_from(gateway.persistence_store(), query).await
}

async fn history_metrics_from(
    store: Option<Arc<dyn crate::control_plane_store::PersistenceStore>>,
    query: HistoryMetricsQuery,
) -> Response {
    let Some(store) = store else {
        return unavailable();
    };
    let since_ms = query
        .since_ms
        .unwrap_or_else(|| now_ms().saturating_sub(DEFAULT_METRICS_WINDOW_MS));
    if since_ms < 0 {
        return bad_request("since_ms must not be negative");
    }
    let limit = query
        .limit
        .unwrap_or(DEFAULT_METRICS_LIMIT)
        .clamp(1, MAX_METRICS_LIMIT);
    // Fetch one sentinel row beyond the response limit so `truncated` is exact
    // while the SQL read remains bounded. The store returns the newest rows in
    // chronological order.
    match tokio::time::timeout(
        HISTORY_QUERY_TIMEOUT,
        store.backend_metrics_history_limited(since_ms, limit.saturating_add(1)),
    )
    .await
    {
        Ok(Ok(mut samples)) => {
            let truncated = samples.len() > limit;
            if truncated {
                samples.drain(..samples.len() - limit);
            }
            json_response(
                StatusCode::OK,
                &MetricsBody {
                    samples,
                    since_ms,
                    limit,
                    truncated,
                },
            )
        }
        Ok(Err(error)) => internal_error("read durable metric history", &error),
        Err(_) => query_timeout(),
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn unavailable() -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        &error_body("persistent history is disabled"),
    )
}

fn bad_request(message: &'static str) -> Response {
    json_response(StatusCode::BAD_REQUEST, &error_body(message))
}

fn query_timeout() -> Response {
    json_response(
        StatusCode::GATEWAY_TIMEOUT,
        &error_body("persistent history query timed out"),
    )
}

fn internal_error(operation: &'static str, error: &str) -> Response {
    tracing::error!(operation, error, "persistent history read failed");
    // The operation label is a static string chosen by this module; the
    // underlying error text stays in the log only.
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &serde_json::json!({ "error": "persistent history read failed", "operation": operation }),
    )
}

fn error_body(message: &'static str) -> serde_json::Value {
    serde_json::json!({ "error": message })
}

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    crate::dashboard_auth::no_store((status, Json(body)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane_store::{PersistenceStore, RequestFinish, RequestRow, SqlStore};
    use http_body_util::BodyExt;

    async fn response_json(response: Response) -> serde_json::Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect response")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("JSON response")
    }

    fn request(id: &str, created_at_ms: i64) -> RequestRow {
        RequestRow {
            id: id.to_string(),
            response_id: None,
            conversation_id: Some("conversation".to_string()),
            virtual_key_id: Some("key-id-not-secret".to_string()),
            client_protocol: "responses".to_string(),
            client_model: "public-model".to_string(),
            alias: Some("public-model".to_string()),
            backend: None,
            resolved_model: None,
            status: "running".to_string(),
            created_at_ms,
            ..RequestRow::default()
        }
    }

    async fn sqlite_store() -> Arc<dyn PersistenceStore> {
        Arc::new(
            SqlStore::connect_sqlite("sqlite::memory:")
                .await
                .expect("connect"),
        )
    }

    #[tokio::test]
    async fn historic_sessions_page_without_an_age_cutoff() {
        let store = sqlite_store().await;
        for (id, last_seen_ms) in [("old", 1), ("tie-a", 2), ("tie-b", 2)] {
            store
                .upsert_session(crate::sessions::SessionRow {
                    display_number: None,
                    id: id.to_string(),
                    parent_id: None,
                    kind: "inferred".to_string(),
                    harness: "oh-my-pi".to_string(),
                    harness_version: None,
                    external_id: None,
                    session_kind: None,
                    client_label: None,
                    virtual_key_id: None,
                    user_id: None,
                    depth: 0,
                    root_request_id: None,
                    spawned_by_request_id: None,
                    first_seen_ms: last_seen_ms,
                    last_seen_ms,
                    request_count: 1,
                })
                .await
                .unwrap();
        }
        store
            .upsert_session(crate::sessions::SessionRow {
                display_number: None,
                id: "old-child".to_string(),
                parent_id: Some("old".to_string()),
                kind: "inferred".to_string(),
                harness: "oh-my-pi".to_string(),
                harness_version: None,
                external_id: None,
                session_kind: None,
                client_label: None,
                virtual_key_id: None,
                user_id: None,
                depth: 1,
                root_request_id: None,
                spawned_by_request_id: None,
                first_seen_ms: 1,
                last_seen_ms: 1,
                request_count: 1,
            })
            .await
            .unwrap();
        let first = response_json(
            history_sessions_from(
                Some(Arc::clone(&store)),
                HistorySessionsQuery {
                    since_ms: Some(0),
                    limit: Some(1),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(first["sessions"][0]["id"], "tie-b");
        assert_eq!(first["truncated"], true);
        let second = response_json(
            history_sessions_from(
                Some(Arc::clone(&store)),
                HistorySessionsQuery {
                    since_ms: Some(0),
                    limit: Some(1),
                    before_ms: first["next_before_ms"].as_i64(),
                    before_id: first["next_before_id"].as_str().map(str::to_owned),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(second["sessions"][0]["id"], "tie-a");
        let third = response_json(
            history_sessions_from(
                Some(store),
                HistorySessionsQuery {
                    since_ms: Some(0),
                    limit: Some(1),
                    before_ms: second["next_before_ms"].as_i64(),
                    before_id: second["next_before_id"].as_str().map(str::to_owned),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(third["sessions"][0]["id"], "old");
        assert_eq!(third["truncated"], false);
        assert_eq!(third["sessions"][0]["child_count"], 1);
    }

    #[tokio::test]
    async fn session_detail_pages_all_requests_with_tied_timestamps() {
        let store = sqlite_store().await;
        store
            .upsert_session(crate::sessions::SessionRow {
                display_number: None,
                id: "root".to_string(),
                parent_id: None,
                kind: "inferred".to_string(),
                harness: "oh-my-pi".to_string(),
                harness_version: None,
                external_id: None,
                session_kind: None,
                client_label: None,
                virtual_key_id: None,
                user_id: None,
                depth: 0,
                root_request_id: None,
                spawned_by_request_id: None,
                first_seen_ms: 1,
                last_seen_ms: 2,
                request_count: 3,
            })
            .await
            .unwrap();
        for (id, timestamp) in [("old", 1), ("tie-a", 2), ("tie-b", 2)] {
            let mut row = request(id, timestamp);
            row.session_id = Some("root".to_string());
            store.begin_request(row).await.unwrap();
        }
        let mut before_ms = None;
        let mut before_id = None;
        let mut seen = Vec::new();
        for _ in 0..3 {
            let page = response_json(
                history_session_detail_from(
                    Some(Arc::clone(&store)),
                    "root".to_string(),
                    HistorySessionQuery {
                        limit: Some(1),
                        before_ms,
                        before_id,
                    },
                )
                .await,
            )
            .await;
            seen.push(page["requests"][0]["id"].as_str().unwrap().to_string());
            before_ms = page["next_before_ms"].as_i64();
            before_id = page["next_before_id"].as_str().map(str::to_owned);
        }
        assert_eq!(seen, ["tie-b", "tie-a", "old"]);
        assert!(before_ms.is_none());
        assert!(before_id.is_none());
    }

    #[tokio::test]
    async fn error_responses_never_echo_store_details() {
        let response = internal_error("unit test", "postgres://secret@host/table missing");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let body = response_json(response).await;
        assert_eq!(body["error"], "persistent history read failed");
        assert!(!body.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn disabled_history_is_an_explicit_no_store_503() {
        let response = history_requests_from(None, HistoryRequestsQuery::default()).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response_json(response).await["error"],
            "persistent history is disabled"
        );
    }

    #[tokio::test]
    async fn request_list_is_bounded_and_detail_preserves_event_order() {
        let store = sqlite_store().await;
        for (id, timestamp) in [("old", 1), ("new", 2)] {
            store
                .begin_request(request(id, timestamp))
                .await
                .expect("begin");
        }
        for seq in [2, 1] {
            store
                .append_event(EventRow {
                    request_id: "new".to_string(),
                    seq,
                    ts_ms: seq,
                    hop: "safe-hop".to_string(),
                    kind: "safe-kind".to_string(),
                    payload: Some(format!(r#"{{"seq":{seq}}}"#)),
                    bytes: None,
                })
                .await
                .expect("event");
        }

        let list = history_requests_from(
            Some(Arc::clone(&store)),
            HistoryRequestsQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(list.status(), StatusCode::OK);
        let list = response_json(list).await;
        assert_eq!(list["limit"], 1);
        assert_eq!(list["requests"].as_array().unwrap().len(), 1);
        assert_eq!(list["requests"][0]["id"], "new");
        assert_eq!(list["requests"][0]["status"], "running");

        let clamped = history_requests_from(
            Some(Arc::clone(&store)),
            HistoryRequestsQuery {
                limit: Some(i64::MAX),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(response_json(clamped).await["limit"], MAX_REQUEST_LIMIT);

        let detail = history_request_detail_from(Some(store), "new".to_string()).await;
        assert_eq!(detail.status(), StatusCode::OK);
        let detail = response_json(detail).await;
        assert_eq!(detail["events"][0]["seq"], 1);
        assert_eq!(detail["events"][1]["seq"], 2);
        assert!(detail.get("secret").is_none());
    }

    #[tokio::test]
    async fn request_list_filters_and_pages_without_a_time_window() {
        let store = sqlite_store().await;
        for (id, timestamp, model, status) in [
            ("ancient", 1, "model-a", "completed"),
            ("same-a", 2, "model-a", "failed"),
            ("same-b", 2, "model-a", "failed"),
            ("other", 3, "model-b", "completed"),
        ] {
            let mut row = request(id, timestamp);
            row.client_model = model.to_string();
            row.status = status.to_string();
            row.user_id = Some(
                if model == "model-a" {
                    "user-a"
                } else {
                    "user-b"
                }
                .to_string(),
            );
            row.virtual_key_id =
                Some(if status == "failed" { "key-f" } else { "key-c" }.to_string());
            if id == "other" {
                row.harness = Some("codex".to_string());
                row.harness_session_id = Some("human-session".to_string());
            }
            store.begin_request(row).await.expect("begin");
        }

        let first = response_json(
            history_requests_from(
                Some(Arc::clone(&store)),
                HistoryRequestsQuery {
                    limit: Some(1),
                    model: Some("model-a".to_string()),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(first["requests"][0]["id"], "same-b");
        assert_eq!(first["has_more"], true);
        let second = response_json(
            history_requests_from(
                Some(Arc::clone(&store)),
                HistoryRequestsQuery {
                    limit: Some(1),
                    model: Some("model-a".to_string()),
                    before_ms: first["next_before_ms"].as_i64(),
                    before_id: first["next_before_id"].as_str().map(str::to_owned),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(second["requests"][0]["id"], "same-a");
        let third = response_json(
            history_requests_from(
                Some(Arc::clone(&store)),
                HistoryRequestsQuery {
                    limit: Some(1),
                    model: Some("model-a".to_string()),
                    before_ms: second["next_before_ms"].as_i64(),
                    before_id: second["next_before_id"].as_str().map(str::to_owned),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(third["requests"][0]["id"], "ancient");
        assert_eq!(third["has_more"], false);

        let identities = response_json(
            history_requests_from(
                Some(Arc::clone(&store)),
                HistoryRequestsQuery {
                    user_id: Some("user-a".to_string()),
                    virtual_key_id: Some("key-f".to_string()),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(identities["requests"].as_array().unwrap().len(), 2);
        let harness = response_json(
            history_requests_from(
                Some(Arc::clone(&store)),
                HistoryRequestsQuery {
                    q: Some("human-session".to_string()),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(harness["requests"][0]["id"], "other");

        let filtered = response_json(
            history_requests_from(
                Some(Arc::clone(&store)),
                HistoryRequestsQuery {
                    q: Some("same_".to_string()),
                    status: Some("failed".to_string()),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(filtered["requests"].as_array().unwrap().len(), 0); // '_' is literal, not a wildcard
        let filtered = response_json(
            history_requests_from(
                Some(Arc::clone(&store)),
                HistoryRequestsQuery {
                    q: Some("same-".to_string()),
                    status: Some("failed".to_string()),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(filtered["requests"].as_array().unwrap().len(), 2);
        let dated = response_json(
            history_requests_from(
                Some(Arc::clone(&store)),
                HistoryRequestsQuery {
                    since_ms: Some(1),
                    until_ms: Some(2),
                    ..Default::default()
                },
            )
            .await,
        )
        .await;
        assert_eq!(dated["requests"][0]["id"], "ancient");
        assert_eq!(dated["requests"].as_array().unwrap().len(), 1);
        let invalid = history_requests_from(
            Some(store),
            HistoryRequestsQuery {
                before_ms: Some(2),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn usage_validates_key_ids_and_does_not_accept_negative_time() {
        let store = sqlite_store().await;
        let too_long = "x".repeat(MAX_KEY_ID_BYTES + 1);
        let invalid_key = history_usage_from(
            Some(Arc::clone(&store)),
            HistoryUsageQuery {
                virtual_key_id: Some(too_long),
                user_id: None,
                since_ms: None,
                limit: None,
            },
        )
        .await;
        assert_eq!(invalid_key.status(), StatusCode::BAD_REQUEST);

        let invalid_time = history_usage_from(
            Some(store),
            HistoryUsageQuery {
                virtual_key_id: None,
                user_id: None,
                since_ms: Some(-1),
                limit: None,
            },
        )
        .await;
        assert_eq!(invalid_time.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn usage_and_metrics_return_only_persistence_dtos() {
        let store = sqlite_store().await;
        store
            .begin_request(request("done", 10))
            .await
            .expect("begin");
        store
            .finish_request(
                "done",
                RequestFinish {
                    status: "completed".to_string(),
                    completed_at_ms: 20,
                    input_tokens: Some(3),
                    output_tokens: Some(4),
                    ..RequestFinish::default()
                },
            )
            .await
            .expect("finish");
        store
            .record_backend_metrics(MetricSample {
                backend: "provider".to_string(),
                ts_ms: 10,
                data: r#"{"healthy":true}"#.to_string(),
            })
            .await
            .expect("metric");
        store
            .record_backend_metrics(MetricSample {
                backend: "provider-new".to_string(),
                ts_ms: 20,
                data: r#"{"healthy":false}"#.to_string(),
            })
            .await
            .expect("metric");

        let usage = history_usage_from(
            Some(Arc::clone(&store)),
            HistoryUsageQuery {
                since_ms: Some(0),
                ..HistoryUsageQuery::default()
            },
        )
        .await;
        let usage = response_json(usage).await;
        assert_eq!(usage["usage"][0]["input_tokens"], 3);
        assert_eq!(usage["since_ms"], 0);
        assert!(usage.get("api_keys").is_none());

        let metrics = history_metrics_from(
            Some(store),
            HistoryMetricsQuery {
                since_ms: Some(0),
                limit: Some(1),
            },
        )
        .await;
        let metrics = response_json(metrics).await;
        assert_eq!(metrics["samples"].as_array().unwrap().len(), 1);
        assert_eq!(metrics["samples"][0]["backend"], "provider-new");
        assert_eq!(metrics["truncated"], true);
        assert!(metrics.get("users").is_none());
    }
}

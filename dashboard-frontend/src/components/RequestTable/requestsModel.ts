import type { ApiKeyRecord, AuthApiKey, AuthUser, DebugWsMessage, FlowDetail, FlowStatus, FlowSummary, HistoryRequest, Usage, UserRecord } from '../../api/types';
import { fmtModelPair } from '../FlowTable/format';

export interface IdentityOptions {
  users: Array<{ id: string; name: string }>;
  keys: Array<{ id: string; name: string; userId: string | null }>;
}

/** Both the current auth service and legacy dashboard accounts can own request keys. */
export function identityOptions(
  authUsers: AuthUser[] = [],
  authKeys: AuthApiKey[] = [],
  legacyUsers: UserRecord[] = [],
  legacyKeys: ApiKeyRecord[] = [],
): IdentityOptions {
  const users = new Map<string, string>();
  const keys = new Map<string, { name: string; userId: string | null }>();
  for (const user of legacyUsers) users.set(user.id, user.username);
  for (const user of authUsers) users.set(user.id, user.display_name);
  for (const key of legacyKeys) if (key.label) keys.set(key.id, { name: key.label, userId: key.user_id });
  for (const key of authKeys) keys.set(key.id, { name: key.name, userId: key.principal_id });
  const byName = (a: { name: string }, b: { name: string }) => a.name.localeCompare(b.name);
  return {
    users: [...users].map(([id, name]) => ({ id, name })).sort(byName),
    keys: [...keys].map(([id, key]) => ({ id, ...key })).sort(byName),
  };
}

export function requestIdentity(row: HistoryRequest, options: IdentityOptions): { user: string; key: string } {
  const key = options.keys.find((item) => item.id === row.virtual_key_id);
  const userId = row.user_id ?? key?.userId;
  return {
    user: options.users.find((item) => item.id === userId)?.name ?? '—',
    key: key?.name ?? '—',
  };
}

export interface RequestDisplay {
  status: string;
  decodingTokensPerSec: number | null;
  endMs: number | null;
  ttftMs: number | null;
  durationMs: number | null;
  inputTokens: number | null;
  newInputTokens: number | null;
  newInputPercent: number | null;
  outputTokens: number | null;
  model: string;
  session: string;
}

export interface RequestSignals {
  firstOutputAtMs?: number;
  /** Last relevant monitor event wins, including a new upstream round after earlier output. */
  streamingPhase?: 'prefilling' | 'decoding';
}

/** The monitor transcript preserves event order even when timestamps share a millisecond. */
export function requestSignals(messages: DebugWsMessage[]): Map<string, RequestSignals> {
  const signals = new Map<string, RequestSignals>();
  for (const message of messages) {
    if (message.type === 'event_append' && message.event.kind === 'upstream_request') {
      signals.set(message.response_id, { ...signals.get(message.response_id), streamingPhase: 'prefilling' });
    } else if (message.type === 'segment_append') {
      const prior = signals.get(message.response_id);
      signals.set(message.response_id, {
        // Reasoning and tool-argument deltas also mean generation has begun, but
        // neither is the first client-visible content token used for TTFT.
        firstOutputAtMs: prior?.firstOutputAtMs ?? (message.segment.kind === 'output' ? message.segment.timestamp_ms : undefined),
        streamingPhase: 'decoding',
      });
    }
  }
  return signals;
}

export interface UsageSample { atMs: number; completion: number }

/** Rate of actual upstream-reported token increments, never inferred from text chunks. */
export function decodingRate(samples: UsageSample[], nowMs: number): number | null {
  const latest = samples.at(-1);
  if (!latest || nowMs - latest.atMs > 10_000) return null;
  const prior = samples.find((sample) => sample.atMs >= latest.atMs - 5_000
    && latest.atMs - sample.atMs >= 200 && sample.completion < latest.completion);
  if (!prior) return null;
  return (latest.completion - prior.completion) * 1_000 / (latest.atMs - prior.atMs);
}

/** Persisted identity/history plus the live WS flow's progressively measured fields. */
export function requestDisplay(row: HistoryRequest, live: FlowSummary | null, nowMs: number, signals: RequestSignals = {}, decodingTokensPerSec: number | null = null): RequestDisplay {
  const persistedTerminal = row.status !== 'running' && row.status !== 'open';
  const liveTerminal = live && live.status !== 'open' ? live.status : null;
  const terminal = persistedTerminal || liveTerminal != null;
  const firstOutputAtMs = signals.firstOutputAtMs ?? row.first_token_at_ms ?? live?.first_content_delta_ms;
  const status = persistedTerminal ? row.status : liveTerminal ??
    (signals.streamingPhase === 'decoding' ? 'decoding'
      : signals.streamingPhase === 'prefilling' ? 'prefilling'
        : firstOutputAtMs != null ? 'decoding'
          : live?.first_upstream_byte_ms != null ? 'prefilling' : 'queued');
  const liveStartMs = live?.ingress_ms ?? live?.started_ms;
  const endMs = terminal ? row.completed_at_ms ?? live?.finished_ms ??
    (live?.elapsed_ms == null || liveStartMs == null ? null : liveStartMs + live.elapsed_ms) : null;
  const ttftMs = persistedTerminal && row.first_token_at_ms != null
    ? Math.max(0, row.first_token_at_ms - row.created_at_ms)
    : live?.first_content_delta_ms != null
      ? Math.max(0, live.first_content_delta_ms - (liveStartMs ?? row.created_at_ms))
      : firstOutputAtMs != null
        ? Math.max(0, firstOutputAtMs - (liveStartMs ?? row.created_at_ms))
        : row.first_token_at_ms == null ? null : Math.max(0, row.first_token_at_ms - row.created_at_ms);
  const durationMs = terminal
    ? row.completed_at_ms != null ? Math.max(0, row.completed_at_ms - row.created_at_ms) : live?.elapsed_ms ?? (endMs == null ? null : Math.max(0, endMs - row.created_at_ms))
    : Math.max(0, nowMs - row.created_at_ms);
  const inputTokens = persistedTerminal ? row.input_tokens : live?.usage?.prompt ?? row.input_tokens;
  const cachedTokens = persistedTerminal ? row.cached_tokens : live?.usage?.cached ?? row.cached_tokens;
  // A changed prompt prefix predicts a cache bust, but only reported cache-read
  // usage can quantify how many input tokens actually needed new prefill work.
  const newInputTokens = inputTokens != null && cachedTokens != null
    && Number.isFinite(inputTokens) && Number.isFinite(cachedTokens)
    && cachedTokens >= 0 && cachedTokens <= inputTokens
    ? inputTokens - cachedTokens : null;
  return {
    status,
    decodingTokensPerSec: status === 'decoding' ? decodingTokensPerSec : null,
    endMs,
    ttftMs,
    durationMs,
    inputTokens,
    newInputTokens,
    newInputPercent: newInputTokens != null && inputTokens != null && inputTokens > 0
      ? Math.round(newInputTokens / inputTokens * 100) : null,
    outputTokens: persistedTerminal ? row.output_tokens : live?.usage?.completion ?? row.output_tokens,
    model: fmtModelPair(row.client_model || row.alias, live?.model_served ?? row.resolved_model),
    session: row.session_display_number != null ? `S-${row.session_display_number}` : row.harness_session_id ?? row.session_id ?? '—',
  };
}

function inspectorStatus(status: string): FlowStatus {
  if (status === 'running' || status === 'open') return 'open';
  if (status === 'failed') return 'failed';
  if (status === 'cancelled') return 'cancelled';
  return 'completed';
}

function historyUsage(row: HistoryRequest): Usage | null {
  if (row.input_tokens == null || row.output_tokens == null) return null;
  return {
    prompt: row.input_tokens,
    completion: row.output_tokens,
    total: row.input_tokens + row.output_tokens,
    cached: row.cached_tokens,
  };
}

export function historyFlowSummary(row: HistoryRequest): FlowSummary {
  const uri = row.client_protocol === 'anthropic_messages' ? '/v1/messages'
    : row.client_protocol === 'chat_completions' ? '/v1/chat/completions' : '/v1/responses';
  return {
    api_call_id: row.id,
    response_id: row.response_id,
    method: 'POST',
    uri,
    model_requested: row.client_model,
    model_served: row.resolved_model,
    upstream_target: row.backend,
    usage: historyUsage(row),
    status: inspectorStatus(row.status),
    started_ms: row.created_at_ms,
    finished_ms: row.completed_at_ms,
    elapsed_ms: row.completed_at_ms == null ? null : Math.max(0, row.completed_at_ms - row.created_at_ms),
    first_content_delta_ms: row.first_token_at_ms,
    ingress_ms: row.created_at_ms,
    cost_confidence: 'unavailable',
    terminal_reason: row.terminal_reason ?? row.error,
    harness: row.harness,
    harness_version: row.harness_version,
    session_id: row.session_id,
    chain_parent_request_id: row.chain_parent_request_id,
    user_id: row.user_id,
    virtual_key_id: row.virtual_key_id,
  };
}

export function historyFlowDetail(row: HistoryRequest, inboundBody?: unknown, upstreamBody?: unknown): FlowDetail {
  const summary = historyFlowSummary(row);
  return {
    flow_seq: 0,
    api_call_id: row.id,
    response_id: row.response_id,
    inbound_body: inboundBody,
    upstream_body: upstreamBody,
    model_requested: summary.model_requested,
    model_served: summary.model_served,
    upstream_target: summary.upstream_target,
    usage: summary.usage,
    status: summary.status,
    deltas: [],
    terminal_reason: summary.terminal_reason,
    started_ms: summary.started_ms,
    finished_ms: summary.finished_ms,
    elapsed_ms: summary.elapsed_ms,
    first_content_delta_ms: summary.first_content_delta_ms,
    ingress_ms: summary.ingress_ms,
    cost: null,
    cost_confidence: 'unavailable',
  };
}

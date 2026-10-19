import type { ApiKeyRecord, AuthApiKey, AuthUser, DebugWsMessage, FlowDetail, FlowStatus, FlowSummary, HistoryRequest, Usage, UserRecord } from '../../api/types';
import {isAttempt} from '../../api/types';
import { fmtModelPair } from '../FlowTable/format';
import {firstUpstreamByteMs} from '../FlowDetail/latencyBreakdown';

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
  decodeNote: string;
  decodeQuality: 'measured' | 'derived' | 'unavailable';
  ttftNote: string;
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
        firstOutputAtMs: prior?.firstOutputAtMs ?? (message.segment.text.length > 0 ? message.segment.timestamp_ms : undefined),
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
  const stored = storedPhases(row);
  const generationFirst = stored.first_generation_delta_ms ?? live?.first_generation_delta_ms;
  const firstOutputAtMs = generationFirst ?? (persistedTerminal ? row.first_token_at_ms : null)
    ?? signals.firstOutputAtMs ?? row.first_token_at_ms ?? live?.first_content_delta_ms;
  const status = persistedTerminal ? row.status : liveTerminal ??
    (signals.streamingPhase === 'decoding' ? 'decoding'
      : signals.streamingPhase === 'prefilling' ? 'prefilling'
        : firstOutputAtMs != null ? 'decoding'
          : live?.first_upstream_byte_ms != null ? 'prefilling' : 'queued');
  const liveStartMs = live?.ingress_ms ?? live?.started_ms;
  const endMs = terminal ? row.completed_at_ms ?? live?.finished_ms ??
    (live?.elapsed_ms == null || liveStartMs == null ? null : liveStartMs + live.elapsed_ms) : null;
  const ttftStart = persistedTerminal ? stored.ingress_ms ?? row.created_at_ms : liveStartMs ?? row.created_at_ms;
  const firstByte = firstUpstreamByteMs(stored) ?? firstUpstreamByteMs(live);
  const ttftMs = firstByte == null ? null : Math.max(0, firstByte - ttftStart);
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
  const timing = stored.generation_timing_complete != null ? stored : live;
  const generationUs = timing?.generation_duration_us;
  const generationTokens = timing?.generation_tokens;
  const completeRate = terminal && timing?.generation_timing_complete === true
    && generationUs != null && generationUs > 0 && Number.isFinite(generationUs)
    && generationTokens != null && generationTokens > 0 && Number.isFinite(generationTokens)
    ? generationTokens * 1_000_000 / generationUs : null;
  // Old content-only records have a wider delivery window. Keep their provenance
  // explicit; never substitute HTTP response headers for a missing first token.
  const legacyFirst = row.first_token_at_ms ?? stored.first_content_delta_ms ?? live?.first_content_delta_ms;
  const legacyEnd = stored.stream_end_ms ?? live?.stream_end_ms;
  const outputTokens = persistedTerminal ? row.output_tokens : live?.usage?.completion ?? row.output_tokens;
  const legacyRate = terminal && timing?.generation_timing_complete == null && legacyFirst != null
    && legacyEnd != null && legacyEnd > legacyFirst && outputTokens != null && outputTokens > 1
    ? (outputTokens - 1) * 1000 / (legacyEnd - legacyFirst) : null;
  const liveRate = status === 'decoding' ? decodingTokensPerSec : null;
  const rate = completeRate ?? legacyRate ?? liveRate;
  return {
    status,
    decodingTokensPerSec: rate,
    decodeQuality: rate == null ? 'unavailable' : liveRate != null && completeRate == null && legacyRate == null ? 'measured' : 'derived',
    decodeNote: completeRate != null
      ? 'Average gateway-observed decode: reported completion tokens minus one per round, divided by summed first-to-last generated-output windows. Includes reasoning; excludes prefill and server-tool waits. Network buffering can affect this rate; it is not engine timing.'
      : legacyRate != null ? 'Historical approximation: reported output tokens minus one, divided by first content delivery to stream end. May include buffering and tool waits.'
        : liveRate != null ? 'Live rate from consecutive upstream-reported completion-token counts.'
          : 'Unavailable: no complete streamed generation window and reported token count.',
    ttftNote: firstByte != null ? 'Request ingress to first upstream response byte (response headers); excludes waiting for reasoning, content or tool arguments.'
      : 'Unavailable: no upstream first-byte timestamp was captured.',
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
    reasoning: row.reasoning_tokens,
  };
}

function storedPhases(row:HistoryRequest):Partial<FlowSummary>{
 let phases:Record<string,unknown>={};let attempts:unknown;
 try{const value:unknown=JSON.parse(row.timings_json??'{}');if(value&&typeof value==='object'&&!Array.isArray(value))phases=value as Record<string,unknown>;}catch{/* Old records may have no phase data. */}
 try{attempts=JSON.parse(row.attempts_json??'null');}catch{/* Leave invalid traces unavailable. */}
 const names=['ingress_ms','normalization_done_ms','routing_decision_ms','first_content_delta_ms','first_generation_delta_ms','generation_duration_us','generation_tokens','stream_end_ms','finalize_ms','first_upstream_byte_ms'] as const;
 return {...(typeof phases.generation_timing_complete==='boolean'?{generation_timing_complete:phases.generation_timing_complete}:{}),...Object.fromEntries(names.filter(k=>typeof phases[k]==='number'&&Number.isFinite(phases[k])&&Number(phases[k])>=0).map(k=>[k,phases[k]])),...(Array.isArray(attempts)&&attempts.every(isAttempt)?{attempts}:{})};
}

export function historyFlowSummary(row: HistoryRequest): FlowSummary {
  const phases = storedPhases(row);
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
    first_content_delta_ms: phases.first_generation_delta_ms != null ? phases.first_content_delta_ms ?? null : row.first_token_at_ms,
    ingress_ms: row.created_at_ms,
    ...phases,
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
    first_generation_delta_ms: summary.first_generation_delta_ms,
    generation_duration_us: summary.generation_duration_us,
    generation_tokens: summary.generation_tokens,
    generation_timing_complete: summary.generation_timing_complete,
    ingress_ms: summary.ingress_ms,
    normalization_done_ms:summary.normalization_done_ms,routing_decision_ms:summary.routing_decision_ms,stream_end_ms:summary.stream_end_ms,finalize_ms:summary.finalize_ms,first_upstream_byte_ms:summary.first_upstream_byte_ms,attempts:summary.attempts,
    cost: null,
    cost_confidence: 'unavailable',
  };
}

/** Explicit false/zero are values; absence does not imply a profile default. */
export function inboundSetting(row:HistoryRequest,column:string):string|number|null {
 const key={reasoningEffort:'reasoning_effort',thinking:'thinking',clearThinking:'clear_thinking',maxTokens:'max_tokens'}[column] as keyof NonNullable<HistoryRequest['inbound_settings']>|undefined;
 const value=key?row.inbound_settings?.[key]:null;
 return value==null?null:typeof value==='string'||typeof value==='number'?value:JSON.stringify(value);
}

import { describe, expect, it } from 'vitest';
import type { DebugWsMessage, FlowSummary, HistoryRequest } from '../../api/types';
import { decodingRate, identityOptions, requestDisplay, requestIdentity, requestSignals } from '../../components/RequestTable/requestsModel';

const history: HistoryRequest = {
  id: 'api_1', response_id: null, user_id: 'usr_1', virtual_key_id: 'key_1',
  client_protocol: 'responses', client_model: 'friendly-alias', alias: 'friendly-alias',
  backend: null, resolved_model: null, status: 'running', created_at_ms: 1000,
  completed_at_ms: null, first_token_at_ms: null, input_tokens: null, output_tokens: null,
  cached_tokens: null, error: null, client_label: null, harness: 'codex',
  harness_version: null, harness_session_id: 'codex-session', session_id: 'sess_internal',
  chain_parent_request_id: null, item_count: null, shared_prefix_items: null,
  divergence_kind: null, divergence_index: null, cache_bust: null,
};

const live: FlowSummary = {
  api_call_id: 'api_1', method: 'POST', uri: '/v1/responses', status: 'open',
  started_ms: 1000, cost_confidence: 'unavailable', model_served: 'provider-model',
  first_content_delta_ms: 1250,
  usage: { prompt: 80, completion: 12, total: 92 },
};

describe('durable request display', () => {
  it('shows an open request and progressively reported streaming values', () => {
    expect(requestDisplay(history, live, 2400)).toMatchObject({
      status: 'decoding', endMs: null, ttftMs: 250, durationMs: 1400,
      inputTokens: 80, outputTokens: 12, model: 'friendly-alias → provider-model',
      newInputTokens: null, newInputPercent: null,
      session: 'codex-session',
    });
  });

  it('uses the first streamed output while the phase-only flow mutation awaits a status frame', () => {
    expect(requestDisplay(history, { ...live, first_content_delta_ms: null }, 2400, { firstOutputAtMs: 1420, streamingPhase: 'decoding' }))
      .toMatchObject({ status: 'decoding', ttftMs: 420 });
  });

  it('follows dispatch and output events through another upstream round', () => {
    const beforeDispatch = { ...live, first_content_delta_ms: null, routing_decision_ms: null };
    expect(requestDisplay(history, beforeDispatch, 2400).status).toBe('queued');
    expect(requestDisplay(history, beforeDispatch, 2400, { streamingPhase: 'prefilling' }).status).toBe('prefilling');
    expect(requestDisplay(history, live, 2400, { firstOutputAtMs: 1250, streamingPhase: 'prefilling' }).status).toBe('prefilling');
    expect(requestDisplay(history, live, 2400, { firstOutputAtMs: 1250, streamingPhase: 'decoding' }, 64))
      .toMatchObject({ status: 'decoding', decodingTokensPerSec: 64 });
  });

  it('tracks the latest upstream round separately from the first output timestamp', () => {
    const dispatch = (at: number): DebugWsMessage => ({
      type: 'event_append', response_id: 'resp_1',
      event: { timestamp_ms: at, kind: 'upstream_request', summary: 'round', images: [] },
    });
    const output = (at: number): DebugWsMessage => ({
      type: 'segment_append', response_id: 'resp_1', segment: { timestamp_ms: at, kind: 'output', text: 'a' },
    });
    const first = requestSignals([dispatch(1100), output(1250)]).get('resp_1');
    expect(first).toEqual({ firstOutputAtMs: 1250, streamingPhase: 'decoding' });
    expect(requestSignals([dispatch(1100), { type: 'segment_append', response_id: 'resp_1',
      segment: { timestamp_ms: 1200, kind: 'reasoning', text: 'thinking' } }]).get('resp_1'))
      .toEqual({ firstOutputAtMs: undefined, streamingPhase: 'decoding' });
    const next = requestSignals([dispatch(1100), output(1250), dispatch(1250)]).get('resp_1');
    expect(next).toEqual({ firstOutputAtMs: 1250, streamingPhase: 'prefilling' });
  });

  it('uses only fresh, increasing upstream token counts for decoding speed', () => {
    expect(decodingRate([], 2000)).toBeNull();
    expect(decodingRate([{ atMs: 1000, completion: 50 }], 2000)).toBeNull();
    expect(decodingRate([{ atMs: 1000, completion: 50 }, { atMs: 2000, completion: 110 }], 2000)).toBe(60);
    expect(decodingRate([{ atMs: 1000, completion: 50 }, { atMs: 2000, completion: 110 }], 13000)).toBeNull();
    expect(decodingRate([{ atMs: 1000, completion: 50 }, { atMs: 2000, completion: 50 }], 2000)).toBeNull();
    expect(requestDisplay(history, live, 2400, { streamingPhase: 'prefilling' }, 60).decodingTokensPerSec).toBeNull();
  });

  it('moves a still-running SQL row to completed as soon as the live flow finishes', () => {
    expect(requestDisplay(history, {
      ...live, status: 'completed', finished_ms: 3100, elapsed_ms: 2100,
      usage: { prompt: 80, completion: 512, total: 592 },
    }, 9000)).toMatchObject({
      status: 'completed', endMs: 3100, durationMs: 2100,
      inputTokens: 80, outputTokens: 512,
    });
  });

  it('uses the live clock for live TTFT and derives End from a terminal elapsed time', () => {
    expect(requestDisplay(history, {
      ...live, started_ms: 800, ingress_ms: 700, first_content_delta_ms: 1200,
      status: 'completed', elapsed_ms: 900, finished_ms: null,
    }, 9000)).toMatchObject({ ttftMs: 500, endMs: 1600, durationMs: 900 });
  });

  it('uses measured terminal values when the SQL writer catches up', () => {
    const done = { ...history, status: 'completed', completed_at_ms: 3000, first_token_at_ms: 1300,
      input_tokens: 91, output_tokens: 15, resolved_model: 'final-model' };
    expect(requestDisplay(done, live, 9000)).toMatchObject({
      status: 'completed', endMs: 3000, ttftMs: 300, durationMs: 2000,
      inputTokens: 91, outputTokens: 15,
    });
  });

  it('shows the uncached input count only when cache-read usage is reported', () => {
    expect(requestDisplay(history, { ...live, usage: { ...live.usage!, cached: 20 } }, 2400))
      .toMatchObject({ newInputTokens: 60, newInputPercent: 75 });
    expect(requestDisplay(history, { ...live, usage: { ...live.usage!, cached: 0 } }, 2400))
      .toMatchObject({ newInputTokens: 80, newInputPercent: 100 });
    expect(requestDisplay(history, { ...live, usage: { ...live.usage!, cached: 80 } }, 2400))
      .toMatchObject({ newInputTokens: 0, newInputPercent: 0 });
    expect(requestDisplay(history, { ...live, usage: { ...live.usage!, cached: 81 } }, 2400))
      .toMatchObject({ newInputTokens: null, newInputPercent: null });
    expect(requestDisplay({ ...history, status: 'completed', input_tokens: 1200, cached_tokens: 900 }, null, 2400))
      .toMatchObject({ newInputTokens: 300, newInputPercent: 25 });
  });

  it('resolves current auth names and never substitutes opaque ids for missing names', () => {
    const options = identityOptions(
      [{ id: 'usr_1', kind: 'user', display_name: 'Alice', enabled: true, created_at: '' }],
      [{ id: 'key_1', principal_id: 'usr_1', name: 'Laptop', prefix: 'llmc', enabled: true, created_at: '', expires_at: null, last_used_at: null }],
    );
    expect(requestIdentity(history, options)).toEqual({ user: 'Alice', key: 'Laptop' });
    expect(requestIdentity({ ...history, user_id: 'missing', virtual_key_id: 'missing' }, options))
      .toEqual({ user: '—', key: '—' });
  });
});

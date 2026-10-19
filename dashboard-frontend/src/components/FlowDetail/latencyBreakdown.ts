/**
 * Request latency from measured phase timestamps. The dashboard's TTFT means
 * ingress → first upstream response byte (including response headers). Reasoning,
 * content and tool generation clocks remain separate, and drive decode timing.
 * Missing endpoints remain unavailable; no token/monitor fallback supplies a byte.
 */
import type { DebugSegment, FlowDetail, FlowSummary, Usage } from '../../api/types';

/** Provenance of a derived figure — mirrors the dashboard's measured/derived/unavailable tags. */
export type Quality = 'measured' | 'derived' | 'estimated' | 'unavailable';

/** A stable id per phase segment (drives keys, test selectors, and the color map). */
export type PhaseId =
  | 'queue' // ingress → normalization (inbound normalize / queue)
  | 'routing' // normalization → routing decision
  | 'upstream' // routing → first upstream byte (wire TTFB); unavailable when no TTFB was measured
  // first upstream byte → first content delta (MEASURED prefill); without a wire TTFB this segment
  // becomes a DERIVED, separately-labelled "routing → first token" span — never a measured prefill.
  | 'prefill'
  | 'generation' // first content delta → stream end (token streaming)
  | 'finalize'; // stream end → finalize (server-side wrap-up)

/** One segment of the waterfall. `durationMs` is meaningful ONLY when `quality !== 'unavailable'`. */
export interface PhaseSegment {
  id: PhaseId;
  /** Human label for the row/segment. */
  label: string;
  /**
   * The sub-duration in ms. `null` ⇒ UNAVAILABLE (an endpoint was unknown) — render `—`, no bar.
   * A finite `0` is a REAL measured ~0ms phase (distinct from unavailable). Never negative.
   */
  durationMs: number | null;
  /**
   * `measured` ⇔ BOTH of the segment's REAL endpoints were known; `unavailable` when one was
   * missing. `derived` is a labelled stand-in span over a KNOWN pair that is NOT the segment's true
   * endpoints (e.g. routing→first-content when the wire TTFB is absent) — shown, never claimed measured.
   */
  quality: Quality;
  /**
   * True when the segment's endpoints were observed out of order (end < start) and the duration
   * was CLAMPED to 0 rather than rendered negative. The UI flags it (a clock-skew note).
   */
  disordered: boolean;
  /** A short, human "why" for the title attribute (which two phases bound it / why unavailable). */
  detail: string;
}

/** A render-ready figure: the formatted-elsewhere value lives in the component; here we keep the raw. */
export interface Figure {
  /** The raw ms (TTFT/total) — `null` ⇒ unavailable. Never a fabricated `0` for "unmeasured". */
  valueMs: number | null;
  quality: Quality;
  /** A short provenance note for the title attribute. */
  detail: string;
}

/** Stream throughput figure (tokens/sec) — always `derived` when computable, else unavailable. */
export interface RateFigure {
  /** Tokens per second — `null` ⇒ unavailable (no completion count or no stream duration). */
  tokensPerSec: number | null;
  quality: Quality;
  detail: string;
}

/** The full latency breakdown the inspector renders. */
export interface LatencyBreakdown {
  /** Total wall-clock (ingress → latest known right edge). `unavailable` ⇒ `—`, never fabricated. */
  total: Figure;
  /** Dashboard TTFT: request ingress → first upstream response byte. */
  ttft: Figure;
  /** Wire TTFB — the served attempt's first upstream byte, relative to ingress (gap 03). */
  ttfb: Figure;
  /** Stream tok/s — completion ÷ generation duration (`derived`); unavailable ⇒ `—`. */
  rate: RateFigure;
  /** The ordered waterfall segments (always all six, each tagged; unavailable ones render `—`). */
  segments: PhaseSegment[];
  /**
   * The sum of the KNOWN (non-unavailable) segment durations — the denominator the bar uses to
   * size each segment's width. `0` when nothing is known (the bar then shows an empty rail).
   */
  knownSpanMs: number;
}

/** A flow shape carrying the spine fields — both `FlowSummary` and `FlowDetail` satisfy it. */
export type SpineFlow = Pick<
  FlowSummary & FlowDetail,
  | 'started_ms'
  | 'ingress_ms'
  | 'normalization_done_ms'
  | 'routing_decision_ms'
  | 'first_content_delta_ms'
  | 'first_generation_delta_ms'
  | 'generation_duration_us'
  | 'generation_tokens'
  | 'generation_timing_complete'
  | 'stream_end_ms'
  | 'finalize_ms'
  | 'finished_ms'
  | 'elapsed_ms'
  | 'attempts'
  | 'first_upstream_byte_ms'
  | 'usage'
>;

/** A finite, non-negative epoch-ms, else null (an absent/`0`-sentinel/garbage timestamp). */
function epoch(v: number | null | undefined): number | null {
  // A real wall-clock epoch is a large positive integer; treat a non-finite or non-positive value
  // as "unmeasured" (the Rust side never emits `0` for an occurred phase — `skip_serializing_if`).
  return typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : null;
}

/**
 * Derive a sub-duration between two phase epochs. Returns the segment tag:
 *  - both known + in order ⇒ `{ durationMs >= 0, quality }`.
 *  - both known but disordered (end < start) ⇒ clamped to 0, `disordered: true` (never negative).
 *  - either unknown ⇒ `durationMs: null, quality: 'unavailable'` (render `—`, no bar).
 */
function span(
  id: PhaseId,
  label: string,
  startMs: number | null,
  endMs: number | null,
  quality: Quality,
  okDetail: string,
  missingDetail: string,
): PhaseSegment {
  if (startMs === null || endMs === null) {
    return { id, label, durationMs: null, quality: 'unavailable', disordered: false, detail: missingDetail };
  }
  const raw = endMs - startMs;
  if (raw < 0) {
    // Clock disorder: clamp to 0 + flag, NEVER render a negative duration (DQ invariant).
    return { id, label, durationMs: 0, quality, disordered: true, detail: `${okDetail} (clock skew: clamped)` };
  }
  return { id, label, durationMs: raw, quality, disordered: false, detail: okDetail };
}

/** The serving response's header/first-byte clock, including failed-only responses. */
export function firstUpstreamByteMs(flow: Pick<FlowSummary, 'attempts' | 'first_upstream_byte_ms'> | null | undefined): number | null {
  const served = flow?.attempts?.find(attempt => attempt.status === 'served');
  const measured = epoch(served?.first_upstream_byte_ms) ?? epoch(flow?.first_upstream_byte_ms);
  if (measured != null || served) return measured;
  const responseTimes = flow?.attempts?.map(attempt => epoch(attempt.first_upstream_byte_ms))
    .filter((time): time is number => time != null) ?? [];
  return responseTimes.length ? Math.min(...responseTimes) : null;
}

/** completion tokens from usage, or null when unreported. */
function completionTokens(usage: Usage | null | undefined): number | null {
  if (!usage) return null;
  return typeof usage.completion === 'number' && Number.isFinite(usage.completion) ? usage.completion : null;
}

/**
 * Build the latency breakdown from the (merged live+frozen) flow spine + the monitor output
 * segments. `flow` is the freshest spine source (caller merges live row + frozen detail);
 * The legacy monitor argument is accepted for caller compatibility; first-byte
 * timing never uses generated output or monitor events.
 */
export function latencyBreakdown(
  flow: SpineFlow | null | undefined,
  _monitorOutputs?: DebugSegment[],
): LatencyBreakdown {
  // Anchor (ingress): prefer the measured `ingress_ms`; else `started_ms` (≈ ingress, always set).
  const startedMs = flow ? epoch(flow.started_ms) : null;
  const ingress = (flow ? epoch(flow.ingress_ms) : null) ?? startedMs;
  const normalize = flow ? epoch(flow.normalization_done_ms) : null;
  const routing = flow ? epoch(flow.routing_decision_ms) : null;
  const firstContent = flow ? epoch(flow.first_content_delta_ms) : null;
  const streamEnd = flow ? epoch(flow.stream_end_ms) : null;
  const finalize = flow ? epoch(flow.finalize_ms) : null;

  // Wire TTFB: the served attempt's first byte, else the flow-level `first_upstream_byte_ms`.
  const upstreamByte = firstUpstreamByteMs(flow);

  // ---- Segments (each from a KNOWN pair; a missing endpoint ⇒ unavailable, never 0ms) ----
  const segments: PhaseSegment[] = [
    span(
      'queue',
      'queue · normalize',
      ingress,
      normalize,
      'measured',
      'ingress → normalization',
      'unavailable — normalization not reached (errored before normalize, or unmeasured)',
    ),
    span(
      'routing',
      'routing',
      normalize,
      routing,
      'measured',
      'normalization → routing decision',
      'unavailable — routing decision not reached (never lowered to the wire, or unmeasured)',
    ),
    // Upstream wait: routing → wire TTFB (gap-03 enriched). When no TTFB was measured the segment
    // is UNAVAILABLE (we do NOT silently stretch it to first-content — that would conflate the
    // provider wait with prefill). The prefill segment then covers routing→first-content instead.
    span(
      'upstream',
      'upstream wait (TTFB)',
      routing,
      upstreamByte,
      'measured',
      'routing → first upstream byte (wire TTFB)',
      'unavailable — no upstream first-byte measured (attempt failed pre-headers, or unmeasured)',
    ),
    // Prefill→first content: the MEASURED provider prefill phase is `first upstream byte → first
    // content delta`. It is `measured` ONLY when BOTH real endpoints are known. When the wire first
    // byte is UNKNOWN we do NOT relabel a different endpoint pair as the measured prefill — instead
    // we expose a SEPARATELY-LABELLED `derived` "routing → first token" span (a known pair, but a
    // wider window that folds the unmeasured upstream wait into it), so the provider-side latency is
    // still shown without ever claiming it is the measured prefill phase (gap-10 review round 1).
    upstreamByte !== null
      ? span(
          'prefill',
          'prefill → first token',
          upstreamByte,
          firstContent,
          'measured',
          'first upstream byte → first content delta (measured)',
          'unavailable — no first content delta (errored before content, or unmeasured)',
        )
      : span(
          'prefill',
          'routing → first token',
          routing,
          firstContent,
          'derived',
          'derived: routing → first content delta (no wire TTFB measured — folds the unmeasured upstream wait into this span; NOT the measured prefill phase)',
          'unavailable — no first content delta (errored before content, or unmeasured)',
        ),
    span(
      'generation',
      'generation (stream)',
      firstContent,
      streamEnd,
      'measured',
      'first content delta → stream end',
      'unavailable — stream did not complete after first content (errored/cancelled, or unmeasured)',
    ),
    span(
      'finalize',
      'finalize',
      streamEnd,
      finalize,
      'measured',
      'stream end → finalize',
      'unavailable — finalize not reached after stream end (or unmeasured)',
    ),
  ];

  // The bar denominator = the sum of the KNOWN segment durations (unavailable ones contribute 0
  // width, NOT a fabricated slice). Never includes a negative (spans clamp to 0).
  const knownSpanMs = segments.reduce((sum, s) => sum + (s.durationMs ?? 0), 0);

  // ---- Total wall-clock (ingress → the latest KNOWN right edge) — never fabricated ----
  // Right edge precedence: finalize ≥ stream_end ≥ first_content ≥ upstreamByte ≥ routing ≥
  // normalize. We use the latest present one so an in-flight/errored flow still shows a bounded
  // span (e.g. ingress→first_content for a still-streaming turn), but only from KNOWN epochs.
  const rightEdge = finalize ?? streamEnd ?? firstContent ?? upstreamByte ?? routing ?? normalize;
  const total: Figure = ingress !== null && rightEdge !== null
    ? {
        valueMs: Math.max(0, rightEdge - ingress),
        quality: 'measured',
        detail: 'wall-clock from ingress to the latest measured phase',
      }
    : { valueMs: null, quality: 'unavailable', detail: 'total unavailable — no measured span yet' };

  // The operator-defined TTFT is first upstream byte, before reasoning/content.
  const ttft: Figure = ingress != null && upstreamByte != null
    ? {valueMs:Math.max(0,upstreamByte-ingress),quality:'measured',detail:'Request ingress to first upstream response byte (response headers); excludes waiting for reasoning, content or tool arguments.'}
    : {valueMs:null,quality:'unavailable',detail:'TTFT unavailable: no upstream first-byte timestamp was captured.'};
  const ttfb = ttft;

  // ---- Stream tok/s (derived) ----
  const rate: RateFigure = flow?.generation_timing_complete != null
    ? flow.generation_timing_complete && (flow.generation_duration_us ?? 0) > 0 && (flow.generation_tokens ?? 0) > 0
      ? {tokensPerSec:flow.generation_tokens! * 1_000_000 / flow.generation_duration_us!,quality:'derived',detail:'Average gateway-observed decode: reported tokens minus one per round, divided by summed first-to-last output windows. Includes reasoning; excludes prefill and server-tool waits. Not engine timing.'}
      : {tokensPerSec:null,quality:'unavailable',detail:'No complete generation window and reported token count.'}
    : computeRate(flow?.usage, firstContent, streamEnd);

  return { total, ttft, ttfb, rate, segments, knownSpanMs };
}

/** tok/s figure (derived): completion ÷ (stream_end − first_content) seconds; else unavailable. */
function computeRate(
  usage: Usage | null | undefined,
  firstContent: number | null,
  streamEnd: number | null,
): RateFigure {
  const completion = completionTokens(usage);
  // Need a KNOWN generation span AND a reported completion count.
  if (completion === null || firstContent === null || streamEnd === null) {
    return {
      tokensPerSec: null,
      quality: 'unavailable',
      detail: 'tok/s unavailable — needs a measured stream duration and a reported completion count',
    };
  }
  const durationMs = streamEnd - firstContent;
  if (durationMs <= 0) {
    // A zero/negative stream window cannot yield an honest rate (no division by zero / no fake ∞).
    return {
      tokensPerSec: null,
      quality: 'unavailable',
      detail: 'tok/s unavailable — stream duration is zero/disordered',
    };
  }
  return {
    tokensPerSec: (completion / durationMs) * 1000,
    quality: 'derived',
    detail: 'stream throughput: completion tokens ÷ (stream end − first content) (derived)',
  };
}

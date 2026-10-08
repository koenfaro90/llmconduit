export interface BenchmarkOptions {
  engine?: 'standalone' | 'container_standard';
  standard?: {upload:boolean;token_file?:string;privileged:boolean};
  concurrency: number[];
  contexts: number[];
  duration_seconds: number;
  max_tokens: number;
  prefill: boolean;
}
export interface BenchmarkCell {
  context_tokens: number;
  concurrency: number;
  status: "complete" | "skipped" | "failed";
  reason: string;
  capacity_limited: boolean;
  aggregate_tps: number | null;
  per_user_tps: number | null;
  client_ttft_avg: number | null;
  client_ttft_p50: number | null;
  client_ttft_p99: number | null;
  client_itl_avg: number | null;
  client_itl_p99: number | null;
  num_requests: number | null;
  num_errors: number | null;
  avg_running_reqs: number | null;
  avg_queue_reqs: number | null;
  elapsed: number | null;
  server_steps_per_s: number | null;
  server_accept_len_effective: number | null;
}
export interface BenchmarkReport {
  standard?: {result_path:string;public_url:string;artifact_available:boolean;summary?:Record<string,unknown>};
  live?: {
    context_tokens: number;
    concurrency: number;
    warmup: boolean;
    running: boolean;
    tok_per_sec: number | null;
    elapsed_seconds: number | null;
  } | null;
  samples?: {
    at_ms: number;
    tok_per_sec: number | null;
    context_tokens: number;
    concurrency: number;
  }[];
  id: string;
  status: string;
  started_at_ms: number;
  updated_at_ms: number;
  finished_at_ms: number | null;
  image: string;
  message: string;
  tool: { version: string; revision: string; repository: string };
  sequence: number;
  cells: BenchmarkCell[];
  prefill: {
    context_tokens: number;
    tok_per_sec: number | null;
    ttft: number | null;
    samples: number;
    status: string;
  }[];
  events: { at_ms: number; message: string }[];
}
export interface BenchmarkRun {
  id: string;
  deployment_id: string;
  endpoint_id: string;
  host: string;
  recipe_id: string;
  revision_digest: string;
  generation: number;
  model: string;
  options: BenchmarkOptions;
  created_at_ms: number;
  cancel_requested: boolean;
  report: BenchmarkReport | null;
  error: string | null;
}
export const PRESETS: Record<"quick" | "standard", BenchmarkOptions> = {
  quick: {
    concurrency: [1, 2],
    contexts: [0, 8192],
    duration_seconds: 5,
    max_tokens: 256,
    prefill: true,
  },
  standard: {
    concurrency: [1, 8, 16],
    contexts: [0, 65536, 131072],
    duration_seconds: 30,
    max_tokens: 2048,
    prefill: true,
  },
};
export function terminal(run: BenchmarkRun) {
  return ["complete", "failed", "cancelled", "interrupted"].includes(
    run.report?.status ?? "queued",
  );
}
export function contextLabel(n: number) {
  return n === 0 ? "0" : n % 1024 === 0 ? `${n / 1024}k` : n.toLocaleString();
}
export function parseMatrix(
  input: string,
  min: number,
  max: number,
  maxLength: number,
): number[] {
  const values = input.split(",").map((v) => {
    if (!/^\d+$/.test(v.trim()))
      throw Error("Enter comma-separated whole numbers.");
    return Number(v.trim());
  });
  if (
    !values.length ||
    values.length > maxLength ||
    new Set(values).size !== values.length ||
    values.some((n) => n < min || n > max)
  )
    throw Error(`Use up to ${maxLength} unique values from ${min} to ${max}.`);
  return values;
}
export type Metric =
  | "aggregate_tps"
  | "per_user_tps"
  | "client_ttft_p50"
  | "client_itl_avg";
export const METRICS: Record<
  Metric,
  { label: string; unit: string; factor: number; lower: boolean }
> = {
  aggregate_tps: {
    label: "Aggregate decode",
    unit: "tok/s",
    factor: 1,
    lower: false,
  },
  per_user_tps: {
    label: "Per-user decode",
    unit: "tok/s",
    factor: 1,
    lower: false,
  },
  client_ttft_p50: {
    label: "Time to first token · p50",
    unit: "ms",
    factor: 1000,
    lower: true,
  },
  client_itl_avg: {
    label: "Inter-token latency · mean",
    unit: "ms",
    factor: 1000,
    lower: true,
  },
};
export function measured(cell: BenchmarkCell | undefined, metric: Metric) {
  const n = cell?.[metric];
  return cell?.status === "complete" &&
    n != null &&
    Number.isFinite(n) &&
    n >= 0
    ? n * METRICS[metric].factor
    : null;
}
export function comparable(a: BenchmarkRun, b: BenchmarkRun) {
  return (
    a.model === b.model &&
    a.report?.tool.revision === b.report?.tool.revision &&
    JSON.stringify(a.options) === JSON.stringify(b.options)
  );
}
export function isBenchmarkRun(value: unknown): value is BenchmarkRun {
  if (!value || typeof value !== "object") return false;
  const v = value as BenchmarkRun;
  if (
    typeof v.id !== "string" ||
    typeof v.model !== "string" ||
    typeof v.host !== "string" ||
    typeof v.deployment_id !== "string" ||
    !Number.isFinite(v.created_at_ms) ||
    !v.options ||
    !Array.isArray(v.options.contexts) ||
    !Array.isArray(v.options.concurrency)
  )
    return false;
  if (v.report == null) return true;
  const r = v.report;
  if (
    typeof r.status !== "string" ||
    typeof r.message !== "string" ||
    !Number.isFinite(r.updated_at_ms) ||
    !r.tool ||
    typeof r.tool.version !== "string" ||
    !Array.isArray(r.cells) ||
    r.cells.length > 24 ||
    !Array.isArray(r.prefill) ||
    r.prefill.length > 8 ||
    !Array.isArray(r.events) ||
    r.events.length > 40
  )
    return false;
  return (
    r.cells.every(
      (c) =>
        Number.isFinite(c.context_tokens) &&
        Number.isFinite(c.concurrency) &&
        ["complete", "skipped", "failed"].includes(c.status) &&
        typeof c.reason === "string" &&
        Object.keys(METRICS).every(
          (k) => c[k as Metric] == null || Number.isFinite(c[k as Metric]),
        ),
    ) &&
    r.events.every(
      (e) => Number.isFinite(e.at_ms) && typeof e.message === "string",
    )
  );
}

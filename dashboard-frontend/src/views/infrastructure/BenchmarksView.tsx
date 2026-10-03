import { useState } from "react";
import { useStore } from "zustand";
import { authStore } from "../../store/authStore";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getConnection } from "../../api/connection";
import { Button } from "../../components/ui/Button";
import { DataTable } from "../../components/ui/DataTable";
import { Panel } from "../../components/ui/Panel";
import { StatChip } from "../../components/ui/StatChip";
import { LineChart, ChartLegend, type ChartSeries } from "../../viz/LineChart";
import { useManagement } from "./useManagement";
import { runtimeSpec, deploymentRows } from "./managementModel";
import {
  PRESETS,
  METRICS,
  contextLabel,
  parseMatrix,
  measured,
  terminal,
  type Metric,
  type BenchmarkRun,
} from "./benchmarkModel";
const field =
  "rounded-md border border-line bg-panel px-3 py-2 text-xs outline-none focus:border-accent";
const fmt = (n: number | null | undefined) =>
  n == null ? "—" : n.toLocaleString(undefined, { maximumFractionDigits: 1 });
function download(run: BenchmarkRun) {
  const blob = new Blob(
    [JSON.stringify({ schema: "fleet-lil-benchmark-v1", ...run }, null, 2)],
    { type: "application/json" },
  );
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `benchmark-${run.deployment_id}-${run.id}.json`;
  a.click();
  URL.revokeObjectURL(url);
}
export function BenchmarksView() {
  const { client } = getConnection();
  const cache = useQueryClient();
  const { state, nodes, error: managementError } = useManagement();
  const [deployment, setDeployment] = useState("");
  const [preset, setPreset] = useState("quick");
  const [concurrency, setConcurrency] = useState("1,2");
  const [contexts, setContexts] = useState("0,8192");
  const [duration, setDuration] = useState(5);
  const [tokens, setTokens] = useState(256);
  const [prefill, setPrefill] = useState(true);
  const [selected, setSelected] = useState<string | null>(()=>decodeURIComponent(window.location.hash.split('/')[2]??'')||null);
  const [metric, setMetric] = useState<Metric>("aggregate_tps");
  const [message, setMessage] = useState("");
  const runs = useQuery({
    queryKey: ["benchmarks"],
    queryFn: () => client.benchmarks(),
    refetchInterval: 2_000,
    retry: false,
  });
  const detail = useQuery({
    queryKey: ["benchmark", selected],
    queryFn: () => client.benchmark(selected!),
    enabled: !!selected,
    refetchInterval: 2_000,
    retry: false,
  });
  const me = useQuery({
    queryKey: ["benchmark-permissions"],
    queryFn: () => client.me(),
    retry: false,
  });
  const targets = deploymentRows(state, nodes).filter(
    (r) =>
      r.deployment.desired_state === "ready" &&
      r.deployment.observed?.status?.phase === "ready" &&
      runtimeSpec(r).readiness != null &&
      !/embedding|rerank/i.test(r.model),
  );
  const hostName = (r: BenchmarkRun) =>
    nodes.find((n) => n.endpoint_id === r.endpoint_id)?.host_snapshot
      ?.hostname ?? r.host;
  const target = targets.find((r) => r.id === (deployment || targets[0]?.id));
  const run = detail.data;
  const report = run?.report;
  const active = run && !terminal(run);
  const mutations = useStore(authStore, (s) => s.mutationsEnabled);
  const allowed = me.data?.is_admin === true && mutations;
  const hostBusy =
    target &&
    runs.data?.some(
      (r) => r.endpoint_id === target.deployment.endpoint_id && !terminal(r),
    );
  const start = useMutation({
    mutationFn: async () => {
      if (!target) throw Error("Select a ready chat deployment.");
      const options = {
        concurrency: parseMatrix(concurrency, 1, 32, 6),
        contexts: parseMatrix(contexts, 0, 131072, 4),
        duration_seconds: duration,
        max_tokens: tokens,
        prefill,
      };
      if (
        options.concurrency.length * options.contexts.length > 24 ||
        duration < 5 ||
        duration > 60 ||
        tokens < 32 ||
        tokens > 2048
      )
        throw Error(
          "Use at most 24 cells, 5–60 seconds and 32–2048 output tokens.",
        );
      return client.startBenchmark({
        id: crypto.randomUUID(),
        deployment_id: target.id,
        model: target.model,
        options,
      });
    },
    onSuccess: (r) => {
      setSelected(r.id);
      setMessage("");
      void cache.invalidateQueries({ queryKey: ["benchmarks"] });
    },
    onError: (e) => setMessage(String(e)),
  });
  const cancel = useMutation({
    mutationFn: () => client.cancelBenchmark(run!.id),
    onSuccess: () => {
      void cache.invalidateQueries({ queryKey: ["benchmark", selected] });
    },
    onError: (e) => setMessage(String(e)),
  });
  const cells = report?.cells ?? [];
  const allContexts = [
    ...new Set([
      ...(run?.options.contexts ?? []),
      ...cells.map((c) => c.context_tokens),
    ]),
  ].sort((a, b) => a - b);
  const levels = run?.options.concurrency ?? [];
  const values = cells
    .map((c) => measured(c, metric))
    .filter((v): v is number => v != null);
  const best = values.length
    ? METRICS[metric].lower
      ? Math.min(...values)
      : Math.max(...values)
    : null;
  const rows = allContexts.map((context) => ({ context }));
  const series: ChartSeries[] = allContexts.map((context) => ({
    key: String(context),
    label: `${contextLabel(context)} context`,
    points: levels
      .slice()
      .sort((a, b) => a - b)
      .map((c) => [
        c,
        measured(
          cells.find(
            (v) => v.context_tokens === context && v.concurrency === c,
          ),
          metric,
        ),
      ]),
  }));
  const liveSeries: ChartSeries[] = [
    {
      key: "live",
      label: "Live decode estimate · tok/s",
      points: (report?.samples ?? []).map((s) => [s.at_ms, s.tok_per_sec]),
    },
  ];
  const status = report?.status ?? (run ? "queued" : "");
  const completed = cells.length;
  const total = run
    ? run.options.contexts.length * run.options.concurrency.length
    : 0;
  return (
    <div
      className="min-w-0 flex-1 overflow-auto p-4 space-y-5"
      data-testid="benchmarks-view"
    >
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <p className="text-[10px] uppercase tracking-[0.24em] text-accent">
            Fleet · LiL inference bench
          </p>
          <h1 className="mt-1 text-2xl font-semibold">Benchmarks</h1>
          <p className="mt-1 text-xs text-text-muted">
            Measure throughput and latency on the serving host. Results appear
            as each cell completes.
          </p>
        </div>
        <span className="rounded-full border border-line px-3 py-1 text-[10px] text-text-muted">
          Local execution · private results · 2-second updates
        </span>
      </header>
      <Panel className="p-4 space-y-4">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold">New benchmark</h2>
          <span className="text-[10px] text-text-muted">One run per host</span>
        </div>
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
          <label className="space-y-1 text-xs">
            Managed deployment
            <select
              aria-label="Benchmark deployment"
              className={`${field} block w-full`}
              value={target?.id ?? ""}
              onChange={(e) => setDeployment(e.target.value)}
            >
              {targets.length === 0 && (
                <option value="">No ready chat deployments</option>
              )}
              {targets.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.model} · {t.host}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1 text-xs">
            Preset
            <select
              aria-label="Benchmark preset"
              className={`${field} block w-full`}
              value={preset}
              onChange={(e) => {
                const v = e.target.value;
                setPreset(v);
                if (v === "quick" || v === "standard") {
                  const p = PRESETS[v];
                  setConcurrency(p.concurrency.join(","));
                  setContexts(p.contexts.join(","));
                  setDuration(p.duration_seconds);
                  setTokens(p.max_tokens);
                  setPrefill(p.prefill);
                }
              }}
            >
              <option value="quick">Quick · 4 cells</option>
              <option value="standard">Standard decode matrix · 9 cells</option>
              <option value="custom">Custom matrix</option>
            </select>
          </label>
          <label className="space-y-1 text-xs">
            Concurrency
            <input
              aria-label="Benchmark concurrency"
              className={`${field} block w-full`}
              value={concurrency}
              onChange={(e) => {
                setConcurrency(e.target.value);
                setPreset("custom");
              }}
            />
          </label>
          <label className="space-y-1 text-xs">
            Context tokens
            <input
              aria-label="Benchmark contexts"
              className={`${field} block w-full`}
              value={contexts}
              onChange={(e) => {
                setContexts(e.target.value);
                setPreset("custom");
              }}
            />
          </label>
          <label className="space-y-1 text-xs">
            Seconds per cell
            <input
              aria-label="Benchmark duration"
              type="number"
              min={5}
              max={60}
              className={`${field} block w-full`}
              value={duration}
              onChange={(e) => {
                setDuration(Number(e.target.value));
                setPreset("custom");
              }}
            />
          </label>
          <label className="space-y-1 text-xs">
            Max output tokens
            <input
              aria-label="Benchmark max tokens"
              type="number"
              min={32}
              max={2048}
              className={`${field} block w-full`}
              value={tokens}
              onChange={(e) => {
                setTokens(Number(e.target.value));
                setPreset("custom");
              }}
            />
          </label>
          <label className="flex items-center gap-2 text-xs">
            <input
              type="checkbox"
              checked={prefill}
              onChange={(e) => {
                setPrefill(e.target.checked);
                setPreset("custom");
              }}
            />
            Measure prefill scouts
          </label>
          <div className="flex items-end justify-end">
            <Button
              disabled={!allowed || !target || !!hostBusy || start.isPending}
              onClick={() => start.mutate()}
            >
              {start.isPending ? "Starting…" : "Run benchmark"}
            </Button>
          </div>
        </div>
        <p className="text-[10px] text-text-muted">
          Runs put load on the selected model. Warmup and capacity checks use
          the LiL engine; limits and skipped cells stay visible. No external
          upload or privileged fabric diagnostic.
        </p>
        {hostBusy && (
          <p className="text-xs text-status-cooling">
            This host already has an active benchmark. Select it below to follow
            progress.
          </p>
        )}
        {!allowed && (
          <p className="text-xs text-text-muted">
            Administrator mutation access is required to start or stop a run.
          </p>
        )}
      </Panel>
      {(message || managementError || runs.error || detail.error) && (
        <p role="alert" className="text-xs text-status-failed">
          {message || String(managementError || runs.error || detail.error)}
        </p>
      )}
      {run && (
        <>
          <Panel className="overflow-hidden">
            <div className="flex flex-wrap items-center justify-between gap-3 p-4">
              <div>
                <div className="flex items-center gap-2">
                  <span
                    className={`h-2 w-2 rounded-full ${active ? "animate-pulse bg-accent" : status === "complete" ? "bg-status-healthy" : "bg-status-cooling"}`}
                  />
                  <h2 className="font-semibold">{run.model}</h2>
                  <span className="rounded bg-line/40 px-2 py-0.5 text-[10px] uppercase tracking-wider">
                    {status}
                  </span>
                </div>
                <p className="mt-1 text-xs text-text-muted">
                  {hostName(run)} · {run.recipe_id} · revision{" "}
                  {run.revision_digest.slice(0, 12)}
                </p>
              </div>
              <div className="flex gap-2">
                <Button
                  variant="ghost"
                  onClick={() => download({ ...run, host: hostName(run) })}
                >
                  Download JSON
                </Button>
                {active && (
                  <Button
                    variant="ghost"
                    disabled={
                      !allowed || cancel.isPending || run.cancel_requested
                    }
                    onClick={() => cancel.mutate()}
                  >
                    {run.cancel_requested ? "Stopping…" : "Stop benchmark"}
                  </Button>
                )}
              </div>
            </div>
            <div
              role="progressbar"
              aria-label="Completed decode cells"
              aria-valuenow={completed}
              aria-valuemax={total}
              className="h-1 bg-line"
            >
              <div
                className="h-full bg-accent transition-all duration-500"
                style={{
                  width: `${total ? Math.min(100, (100 * completed) / total) : 0}%`,
                }}
              />
            </div>
            <div className="flex flex-wrap">
              <StatChip
                label="Decode cells"
                value={`${completed} / ${total}`}
              />
              <StatChip
                label={`Best ${METRICS[metric].label}`}
                value={
                  best == null ? "—" : `${fmt(best)} ${METRICS[metric].unit}`
                }
                accent="accent"
              />
              <StatChip
                label="Failed / skipped"
                value={`${cells.filter((c) => c.status === "failed").length} / ${cells.filter((c) => c.status === "skipped").length}`}
              />
              <StatChip
                label="Engine"
                value={`LiL ${report?.tool.version ?? "pending"}`}
              />
              <StatChip
                label="Last update"
                value={
                  report
                    ? new Date(report.updated_at_ms).toLocaleTimeString()
                    : "Awaiting Fleet"
                }
              />
            </div>
            <p className="border-t border-line px-4 py-2 text-xs text-text-muted">
              {run.error ?? report?.message ?? "Queued for Fleet execution…"}
            </p>
          </Panel>
          <Panel className="p-4 space-y-2">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h3 className="text-sm font-semibold">
                {active ? "Live measurement" : "Recent measurement trace"}
              </h3>
              <span className="font-mono text-xs text-accent">
                {active && report?.live?.running
                  ? `${contextLabel(report.live.context_tokens)} · C${report.live.concurrency} · ${report.live.warmup ? "warming up" : `${fmt(report.live.tok_per_sec)} tok/s · ${fmt(report.live.elapsed_seconds)}s`}`
                  : "Waiting / between cells"}
              </span>
            </div>
            <LineChart series={liveSeries} width={1400} height={135} unit=" tok/s" />
            <p className="text-[10px] text-text-muted">
              Last 60 samples. Live rate is an estimate; the matrix contains
              finalized measurements.
            </p>
          </Panel>
          <div className="grid gap-4 xl:grid-cols-[3fr_2fr]">
            <Panel className="p-4 space-y-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h3 className="text-sm font-semibold">Measurement matrix</h3>
                <select
                  aria-label="Benchmark metric"
                  className={field}
                  value={metric}
                  onChange={(e) => setMetric(e.target.value as Metric)}
                >
                  {Object.entries(METRICS).map(([key, v]) => (
                    <option key={key} value={key}>
                      {v.label} · {v.unit}
                    </option>
                  ))}
                </select>
              </div>
              <DataTable interactive
                id="benchmark-matrix"
                tableTestId="benchmark-matrix"
                rows={rows}
                rowKey={(r) => String(r.context)}
                minWidth={400}
                columns={[
                  {
                    id: "context",
                    label: "Context / concurrency",
                    required: true,
                    render: (r) => <strong>{contextLabel(r.context)}</strong>,
                  },
                  ...levels.map((c) => ({
                    id: `c-${c}`,
                    label: `C${c}`,
                    align: "right" as const,
                    render: (row: { context: number }) => {
                      const cell = cells.find(
                        (v) =>
                          v.context_tokens === row.context &&
                          v.concurrency === c,
                      );
                      const v = measured(cell, metric);
                      const ratio =
                        v != null && best != null
                          ? METRICS[metric].lower
                            ? v > 0
                              ? best / v
                              : 1
                            : best > 0
                              ? v / best
                              : 0
                          : 0;
                      return (
                        <div
                          title={
                            cell?.reason ||
                            `${METRICS[metric].label} · ${METRICS[metric].unit}`
                          }
                          className={`rounded-md px-3 py-3 ${v != null ? "text-accent" : cell?.status === "failed" ? "text-status-failed" : "text-text-muted"}`}
                          style={
                            v != null
                              ? {
                                  backgroundColor: `rgba(134, 212, 162, ${0.05 + 0.2 * ratio})`,
                                }
                              : undefined
                          }
                        >
                          <span className="font-mono text-base font-semibold">
                            {v != null
                              ? fmt(v)
                              : cell?.status === "skipped"
                                ? "Skip"
                                : cell?.status === "failed"
                                  ? "Error"
                                  : active
                                    ? "…"
                                    : "—"}
                          </span>
                          {cell?.capacity_limited && (
                            <p className="text-[9px] text-status-cooling">
                              Capacity limited
                            </p>
                          )}
                        </div>
                      );
                    },
                  })),
                ]}
                emptyMessage="Awaiting the benchmark plan."
              />
              <p className="text-[10px] text-text-muted">
                {METRICS[metric].lower
                  ? "Lower latency is better."
                  : "Higher throughput is better."}{" "}
                Empty cells are unmeasured; skipped or failed cells are never
                shown as zero.
              </p>
            </Panel>
            <Panel className="p-4 space-y-3">
              <h3 className="text-sm font-semibold">
                Scaling with concurrency
              </h3>
              <LineChart
                series={series}
                height={220}
                unit={` ${METRICS[metric].unit}`}
                formatX={(n) => `C${n}`}
                showPoints
              />
              <ChartLegend series={series} />
              <p className="text-[10px] text-text-muted">
                Same context, increasing load · completed cells only
              </p>
            </Panel>
          </div>
          {!!report?.prefill.length && (
            <Panel className="p-4 space-y-2">
              <h3 className="text-sm font-semibold">Prefill scouts</h3>
              <DataTable interactive
                id="benchmark-prefill"
                rows={report.prefill}
                rowKey={(p) => String(p.context_tokens)}
                columns={[
                  {
                    id: "context",
                    label: "Context",
                    render: (p) => contextLabel(p.context_tokens),
                  },
                  {
                    id: "rate",
                    label: "Prefill tok/s",
                    render: (p) =>
                      p.status === "skipped" ? "Skip" : fmt(p.tok_per_sec),
                  },
                  {
                    id: "ttft",
                    label: "TTFT · seconds",
                    render: (p) => fmt(p.ttft),
                  },
                  { id: "samples", label: "Samples", render: (p) => p.samples },
                ]}
              />
            </Panel>
          )}
          <Panel className="p-4 space-y-3">
            <h3 className="text-sm font-semibold">
              Latency and capacity detail
            </h3>
            <DataTable interactive
              id="benchmark-details"
              rows={cells}
              rowKey={(c) => `${c.context_tokens}-${c.concurrency}`}
              minWidth={1000}
              columns={[
                {
                  id: "cell",
                  label: "Cell",
                  required: true,
                  render: (c) =>
                    `${contextLabel(c.context_tokens)} · C${c.concurrency}`,
                },
                {id:"throughput",label:"Decode tok/s · total / user",value:c=>c.aggregate_tps,render:c=>`${fmt(c.aggregate_tps)} / ${fmt(c.per_user_tps)}`},
                { id: "state", label: "Status", render: (c) => c.status },
                {
                  id: "ttft",
                  label: "TTFT p50 / p99 · ms",
                  render: (c) =>
                    `${fmt(c.client_ttft_p50 == null ? null : c.client_ttft_p50 * 1000)} / ${fmt(c.client_ttft_p99 == null ? null : c.client_ttft_p99 * 1000)}`,
                },
                {
                  id: "itl",
                  label: "ITL mean / p99 · ms",
                  render: (c) =>
                    `${fmt(c.client_itl_avg == null ? null : c.client_itl_avg * 1000)} / ${fmt(c.client_itl_p99 == null ? null : c.client_itl_p99 * 1000)}`,
                },
                {
                  id: "requests",
                  label: "Completed / errors",
                  render: (c) =>
                    `${fmt(c.num_requests)} / ${fmt(c.num_errors)}`,
                },
                {
                  id: "capacity",
                  label: "Running / queued",
                  render: (c) =>
                    `${fmt(c.avg_running_reqs)} / ${fmt(c.avg_queue_reqs)}`,
                },
                {
                  id: "steps",
                  label: "Engine steps/s",
                  render: (c) =>
                    c.server_steps_per_s ? fmt(c.server_steps_per_s) : "—",
                },
                {
                  id: "reason",
                  label: "Failure / skip reason",
                  render: (c) => c.reason || "—",
                },
              ]}
              emptyMessage="Measurements arrive after each cell."
            />
          </Panel>
          <details className="rounded-lg border border-line p-4" open={active}>
            <summary className="cursor-pointer text-sm font-semibold">
              Live event timeline{" "}
              <span className="text-xs font-normal text-text-muted">
                · latest 40 events
              </span>
            </summary>
            <div className="mt-3 max-h-56 space-y-2 overflow-auto font-mono text-[10px]">
              {report?.events
                .slice()
                .reverse()
                .map((e, i) => (
                  <div key={`${e.at_ms}-${i}`} className="flex gap-3">
                    <time className="shrink-0 text-text-muted">
                      {new Date(e.at_ms).toLocaleTimeString()}
                    </time>
                    <span>{e.message}</span>
                  </div>
                ))}
              {!report?.events.length && (
                <p className="text-text-muted">Waiting for the first phase…</p>
              )}
            </div>
          </details>
          <details className="rounded-lg border border-line p-4">
            <summary className="cursor-pointer text-xs text-text-muted">
              Pinned image, engine source and run configuration
            </summary>
            <pre className="mt-3 max-h-64 overflow-auto text-[10px]">
              {JSON.stringify(
                {
                  image: report?.image,
                  tool: report?.tool,
                  options: run.options,
                  deployment_generation: run.generation,
                  recipe_digest: run.revision_digest,
                },
                null,
                2,
              )}
            </pre>
          </details>
        </>
      )}
      <Panel className="p-4 space-y-3">
        <h2 className="text-sm font-semibold">Run history</h2>
        <DataTable interactive
          id="benchmark-history"
          tableTestId="benchmark-history"
          rows={runs.data ?? []}
          rowKey={(r) => r.id}
          minWidth={800}
          onRowClick={(r) => setSelected(r.id)}
          columns={[
            {
              id: "started",
              label: "Started",
              render: (r) => new Date(r.created_at_ms).toLocaleString(),
            },
            {
              id: "model",
              label: "Model / deployment",
              required: true,
              render: (r) => (
                <button
                  className="text-left text-accent"
                  onClick={() => setSelected(r.id)}
                >
                  {r.model}
                  <p className="text-[10px] text-text-muted">
                    {r.deployment_id}
                  </p>
                </button>
              ),
            },
            { id: "host", label: "Host", render: (r) => hostName(r) },
            {
              id: "status",
              label: "Status",
              render: (r) => r.report?.status ?? "queued",
            },
            {
              id: "matrix",
              label: "Matrix",
              render: (r) =>
                `${r.options.contexts.length} contexts × ${r.options.concurrency.length} concurrencies`,
            },
            {
              id: "recipe",
              label: "Revision",
              render: (r) => r.revision_digest.slice(0, 12),
            },
          ]}
          emptyMessage="No benchmark runs yet. Start a quick run to establish a baseline."
        />
      </Panel>
    </div>
  );
}

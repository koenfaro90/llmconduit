import { afterEach, beforeEach, it, expect, vi } from "vitest";
import { cleanup, screen, waitFor, fireEvent } from "@testing-library/react";
import { renderWithQuery, resetWorld } from "../../components/testHarness";
import { getConnection } from "../../api/connection";
import { BenchmarksView } from "./BenchmarksView";
import type { BenchmarkRun, BenchmarkCell } from "./benchmarkModel";
const run: BenchmarkRun = {
  id: "12345678-1234-1234-1234-123456789abc",
  deployment_id: "dep",
  endpoint_id: "node",
  host: "gpu03",
  recipe_id: "model",
  revision_digest: "a".repeat(64),
  generation: 1,
  model: "actual",
  created_at_ms: Date.now(),
  cancel_requested: false,
  error: null,
  options: {
    contexts: [0],
    concurrency: [1, 2],
    duration_seconds: 5,
    max_tokens: 64,
    prefill: false,
  },
  report: {
    id: "12345678-1234-1234-1234-123456789abc",
    status: "running",
    started_at_ms: Date.now(),
    updated_at_ms: Date.now(),
    finished_at_ms: null,
    image: "sha256:abc",
    tool: { version: "0.7.5", revision: "pinned", repository: "upstream" },
    sequence: 2,
    message: "Measuring C2",
    events: [{ at_ms: Date.now(), message: "cell start C=2 ctx=0" }],
    cells: [
      {
        context_tokens: 0,
        concurrency: 1,
        status: "complete",
        aggregate_tps: 123,
        per_user_tps: 123,
        client_ttft_p50: 0.25,
        reason: "",
        capacity_limited: false,
      } as BenchmarkCell,
    ],
    prefill: [],
  },
};
beforeEach(() => {
  resetWorld({ mock: true });
  const c = getConnection().client;
  vi.spyOn(c, "me").mockResolvedValue({
    user: null,
    is_admin: true,
    accounts_enabled: true,
    auth_mode: "token",
  });
  vi.spyOn(c, "management").mockResolvedValue({
    recipes: {
      model: [
        {
          revision: 1,
          digest: "a".repeat(64),
          description: "Inference",
          model_spec: {
            image: "sha256:abc",
            command: ["--served-model-name", "actual"],
            readiness: { url: "http://127.0.0.1:8000/health" },
          },
          created_at_ms: 1,
        },
      ],
    },
    deployments: {
      dep: {
        endpoint_id: "node",
        recipe_id: "model",
        revision_digest: "a".repeat(64),
        generation: 1,
        desired_state: "ready",
        updated_at_ms: 1,
        observed_at_ms: Date.now(),
        observed: { status: { phase: "ready" } },
        error: null,
      },
    },
  });
  vi.spyOn(c, "benchmarks").mockResolvedValue([run]);
  vi.spyOn(c, "benchmark").mockResolvedValue(run);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});
it('starts the image container standard with explicit public upload and a file reference',async()=>{
 const c=getConnection().client;vi.mocked(c.benchmarks).mockResolvedValue([]);
 const start=vi.spyOn(c,'startBenchmark').mockResolvedValue({...run,options:{...run.options,engine:'container_standard'}});
 renderWithQuery(<BenchmarksView/>);
 fireEvent.change(await screen.findByLabelText('Benchmark tool'),{target:{value:'container_standard'}});
 expect(screen.queryByLabelText('Benchmark duration')).not.toBeInTheDocument();
 expect(screen.queryByLabelText('LiL upload identifier file')).not.toBeInTheDocument();
 fireEvent.click(screen.getByLabelText('Upload publicly to LiL under my GitHub account'));
 fireEvent.change(screen.getByLabelText('LiL upload identifier file'),{target:{value:'/run/fleet-secrets/lil-bench.token'}});
 fireEvent.click(screen.getByLabelText('Privileged hardware diagnostics (host permission required)'));
 await waitFor(()=>expect(screen.getByRole('button',{name:'Run benchmark'})).not.toBeDisabled());
 fireEvent.click(screen.getByRole('button',{name:'Run benchmark'}));
 await waitFor(()=>expect(start).toHaveBeenCalledWith(expect.objectContaining({options:expect.objectContaining({engine:'container_standard',standard:{upload:true,token_file:'/run/fleet-secrets/lil-bench.token',privileged:true}})})));
});
it("shows partial cells and supports cancellation without inventing unfinished measurements", async () => {
  const cancel = vi
    .spyOn(getConnection().client, "cancelBenchmark")
    .mockResolvedValue({ ...run, cancel_requested: true });
  renderWithQuery(<BenchmarksView />);
  fireEvent.click(await screen.findByRole("button", { name: "actual dep" }));
  const matrix = await screen.findByTestId("benchmark-matrix");
  await waitFor(() => expect(matrix).toHaveTextContent("123"));
  expect(matrix).toHaveTextContent("…");
  expect(screen.getByText("cell start C=2 ctx=0")).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Benchmark metric"), {
    target: { value: "client_ttft_p50" },
  });
  expect(matrix).toHaveTextContent("250");
  fireEvent.click(screen.getByRole("button", { name: "Stop benchmark" }));
  await waitFor(() => expect(cancel).toHaveBeenCalledWith(run.id));
});
it("disables another run on the same host", async () => {
  renderWithQuery(<BenchmarksView />);
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Run benchmark" }),
    ).toBeDisabled(),
  );
  expect(
    await screen.findByText(/This host already has an active benchmark/),
  ).toBeInTheDocument();
});
it("validates custom matrices before sending a start command", async () => {
  const c = getConnection().client;
  vi.mocked(c.benchmarks).mockResolvedValue([]);
  const start = vi.spyOn(c, "startBenchmark");
  renderWithQuery(<BenchmarksView />);
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Run benchmark" }),
    ).not.toBeDisabled(),
  );
  fireEvent.click(screen.getByRole("button", {name:"Benchmark concurrency C1"}));
  fireEvent.click(screen.getByRole("button", {name:"Benchmark concurrency C2"}));
  fireEvent.click(screen.getByRole("button", { name: "Run benchmark" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Select at least one concurrency");
  expect(start).not.toHaveBeenCalled();
});

it("shows pending prefill depths before measurements and keeps skipped depths explicit",async()=>{
 const c=getConnection().client;const planned={...run,options:{...run.options,contexts:[32768,8192],prefill:true},report:{...run.report!,prefill:[{context_tokens:8192,tok_per_sec:null,ttft:null,samples:0,status:'skipped'}]}};
 vi.mocked(c.benchmarks).mockResolvedValue([planned]);vi.mocked(c.benchmark).mockResolvedValue(planned);
 renderWithQuery(<BenchmarksView/>);fireEvent.click(await screen.findByRole('button',{name:'actual dep'}));
 const prefill=await screen.findByTestId('prefill-depths');await waitFor(()=>expect(prefill).toHaveTextContent('skipped'));expect(prefill).toHaveTextContent('pending');expect(prefill).toHaveTextContent('8k');expect(prefill).toHaveTextContent('32k');expect(prefill).toHaveTextContent('Time to first token');
});

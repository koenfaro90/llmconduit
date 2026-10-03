# Benchmarks through Fleet

The Infrastructure → Benchmarks page starts LiL inference throughput/latency jobs
on ready managed chat deployments. Quick and standard-decode presets or a custom
bounded matrix select context lengths, concurrency, seconds per cell, output tokens
and prefill scouts. The existing tables, panels, stat chips and charts display a
colored result matrix, concurrency scaling, live rate estimates, prefill measurements,
latency/capacity detail, progress and a recent event timeline. Download JSON preserves
run provenance and partial/final measurements. Empty, failed and skipped cells are
visibly distinct. The standard decode preset uses LiL's standard matrix dimensions;
it does not claim the full standardized inventory/P2P/uploader workflow.

Admin session authentication, mutation gating and CSRF apply to start/stop. Fleet is
the execution authority: LLMConduit persists a run UUID, selected endpoint/deployment,
recipe revision and generation, then sends typed commands over the authenticated,
pinned mesh management stream. Workers use their local Fleet credential; no Docker
access, arbitrary command, arbitrary target URL or benchmark token is exposed to the
controller. The model name is checked against the host-local backend catalog.

The central reconciler follows progress every two seconds, captures completed cells
and preserves the latest report in PostgreSQL (SQLite works too). Reopening a browser
or restarting the controller resumes observation, not execution. A retried start uses
the same UUID and cannot duplicate a host run. One active benchmark per host is
admitted both centrally and locally. Fleet stores partial results durably, terminates
on cancellation/deployment changes, and marks unfinished jobs interrupted on restart.

## API

- `GET /dashboard/api/benchmarks`: bounded run summaries.
- `POST /dashboard/api/benchmarks`: `{id: UUID, deployment_id, model, options}`.
- `GET /dashboard/api/benchmarks/{id}`: latest full run report, suitable for export.
- `POST /dashboard/api/benchmarks/{id}/cancel`: record a durable cancellation request.

Options are `concurrency: number[]`, `contexts: number[]`, `duration_seconds`,
`max_tokens` and `prefill`. The bounds are 1–32 concurrency, six concurrency levels,
four contexts of 0–131072 tokens, 24 cells, 5–60 seconds and 32–2048 output tokens.
Fleet enforces a 30-minute wall limit. Up to 64 central reports are retained; a new
run evicts the oldest terminal central report. Each live report is at most 48 KiB
with 24 cells, eight prefill rows, 40 phase events and 60 live estimates. This bounds
job data without adding a general telemetry retention/downsampling service.

A disconnected host keeps its accepted benchmark running until the local deadline.
The controller shows the last report plus an unavailable/reconnecting error; it does
not fabricate new samples. Cancellation waits for the host to reconnect. Explicit
Fleet rejection marks a job failed, or cancelled if it had never started. A benchmark
loads the selected inference engine and concurrent application traffic affects results.
Only local unauthenticated HTTP chat backends are currently supported. Embedding,
rerank, image/audio/video benchmarks, accuracy datasets, P2P diagnostics, public uploads
and cross-run comparisons are outside this initial integration.

The engine is the unchanged LiL `llm_decode_bench.py`, version 0.7.5, pinned to its
source SHA and hash inside Fleet. See `~/fleet/docs/benchmarks.md` and
`~/fleet/internal/benchmark/tool/source.json`. Provider metrics are scraped by the
benchmark engine when available; there is no Prometheus server dependency.

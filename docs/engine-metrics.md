# Fleet inference-engine metrics

Models → Inference engine metrics shows each deployment's complete engine export,
including requests sent directly by LiteLLM. Choose a model/host, inspect running
and queued counts, search/sort/filter every labeled metric, and select up to eight
series to graph. Labels and histogram buckets remain separate series. Counter rates
use actual adjacent timestamp deltas; resets and missing samples remain gaps.
JSON export contains all latest metrics and the selected history. Longer windows
are paginated at full resolution rather than silently downsampled. Host settings
has an Engine metrics collection interval: opt-in, `off` by default; `1s` enables
every-second collection, up to `1m`. Machine reporting/history retain their existing
5s/15s intervals. No inference restart or Prometheus service is needed.

Fleet discovers local desired ready HTTP workloads and scrapes `/metrics`; sources
without a valid Prometheus exposition produce explicit failures. All valid series
are captured, including vLLM queue/run gauges, wait reasons, KV/prefix-cache metrics,
prompt/generated tokens, speculative acceptance, preemptions, latency histograms,
HTTP handling and Python/process metrics. Metric availability depends on the image.
Fleet timestamps each scrape; vLLM may update some metrics less often. Gauges show
engine batches, not user/client concurrency. `running=1, waiting>0` locates queueing
inside the engine; low engine counts despite waiting clients warrant checking the
LiteLLM/client path. This does not attribute direct traffic to individual users.

Samples travel through bounded authenticated Fleet management batches, separate
from heartbeats. Gzip/base64 retains names, labels, HELP/TYPE, numeric values and
explicit nonfinite values. SQL migration 0018 stores each sample idempotently by
host/deployment/time. Per-agent cursors resume a bounded five-minute ring; a restart
starts a new stream. Missing intervals remain visible as graph gaps. Capture does
not retain prompts, headers or tokens; credential-named labels are redacted.

Authenticated API: `GET /dashboard/api/hosts/{endpoint}/engine-metrics` with
`deployment_id`, `since_ms`, optional `until_ms`, `limit` (1–3600), and comma-separated
`names`. `limit=1` returns every latest series; history requires selected names
(maximum 24). `next_until_ms` loads preceding pages. Responses are capped at 8 MiB;
narrow the range/page/series selection on a bound error. Every sample also carries
its generation, image, recipe digest, duration and scrape error. No automatic
retention/downsampling was added. Repeated observations are retained because they
are useful evidence that an engine exporter has stopped updating.

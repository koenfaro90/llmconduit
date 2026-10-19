# Exact request and response capture

Admin → Observability → **Exact HTTP capture directory** enables private capture
of complete HTTP entity bodies. Production/dev use `/data/request-wire` on their
own persistent volumes. It applies to new instrumented inference requests live.

The request inspector uses separate Headers, Inbound request, Upstream request,
Upstream response and Inbound response tabs. Copy uses the original UTF-8 bytes,
including whitespace, escapes, BOM and SSE framing; Download also preserves
binary and compressed bodies. Valid JSON displays with indentation and syntax
highlighting by default, with a Show raw toggle. Display formatting changes only
whitespace between JSON tokens, preserving large numeric literals, duplicate keys
and string escapes. Copy and Download always export the original body. There is
no body-size ceiling, preview clipping, redaction or media substitution in this archive.
Header timing/usage details are collapsible at every screen size. The inspector
uses the full viewport on mobile and short desktop screens; a side dock is used
only when the viewport is at least1024px wide and900px high.
Existing request-admission and inference-protocol limits remain separate from
capture; a cancelled transport is recorded as incomplete, never as a full response.

Ingress captures precede decompression/adapters. Upstream capture reads the exact
reqwest request byte body handed to `execute` and tees the actual response before
SSE parsing. The served response is teed after protocol conversion. Every upstream
send, including retries and server-tool rounds, gets its own pair; choose the call
in the upstream tabs. Successful-stream cancellation retains normal upstream
cancellation; error bodies continue to drain past the error reader's preview.

Bodies spool to private files with backpressure, then share immutable SHA-256
byte chunks. Chunk size is an IO/deduplication unit, not a record-size limit.
Concatenating chunks recreates the original bytes. Each body has a full hash and
byte count; manifests publish atomically. Complete body exports stream from disk.
Captured header values are losslessly base64-encoded in manifests; they describe
HTTP headers exposed by the gateway/reqwest API, not original TLS packets or HTTP
frame encodings. All raw capture APIs require dashboard administrator access and
use `Cache-Control: no-store`. Capture directories/files use private permissions.
Storage failures are surfaced, not silently routed through a dropping telemetry queue.

`GET /dashboard/api/history/requests/{id}/wire` returns the manifest or `null` for
an old/uncaptured request. `GET .../wire/{part}` streams original bytes. Parts are
`client_in`, `client_out`, and `upstream_000001_out` / `upstream_000001_in` (and later
call numbers). Request IDs and part keys are validated before opening files.

SQL summaries, lineage/deduplication items and bounded diagnostic previews retain
their existing formats. They are not the original-byte authority. Older captures
cannot recover bytes previously redacted or clipped: the UI explicitly identifies
legacy reconstructed captures and offers their stored request/response contents.
Raw archives have no automatic retention/pruning; SQL history clearing does not
remove this separate archive. Recorded bodies include any credentials they carried.

Request tables include Reasoning effort, Thinking, clear_thinking and max_tokens.
These are client-supplied inbound settings, not upstream/profile defaults. Explicit
false/zero remain visible; absence is shown as `—`. The projection recognizes
`reasoning_effort` / `reasoning.effort`; top-level `thinking` / `clear_thinking`
and their `chat_template_kwargs` equivalents (including `enable_thinking`);
token limits use `max_completion_tokens`, then `max_tokens`, then
`max_output_tokens`. Thinking objects remain visible as JSON. Migration0019 stores
the small projection independently of exact capture. Historical rows backfill from
retained inbound skeleton events in one indexed batch for the requested page,
without reconstructing message blobs or sending bodies to the table. Column
sorting/filtering operates on the loaded page; the view's server filters still
search retained history.

## Request timing columns

TTFT uses request ingress to the first upstream response byte, including response
headers, as defined by the operator. It reads persisted `first_upstream_byte_ms`
and attempt timings, so historical requests with a byte timestamp also work.
Reasoning, content and tool generation clocks remain separate and do not determine
TTFT. If the byte timestamp is missing, TTFT stays unavailable. No timing is
reconstructed from raw body archives, which lack per-chunk arrival times.

After completion, Decode speed retains an average from upstream-reported completion
tokens (including reasoning, without double counting) minus one first token per
upstream round, divided by the summed monotonic first-to-last output windows.
Prefill, usage-tail latency and server-tool execution are excluded. This is a
**derived gateway-observed rate**, affected by transport buffering and speculative
chunks; it is not an engine-reported timing or a benchmark. Every generated round
must have usage and a nonzero streaming window, otherwise the rate stays unavailable.
Live speed still requires successive reported token-counter updates; final-only
usage cannot provide an instantaneous live speed.

Historical content-only records can show a clearly identified approximation using
first content delivery through stream end. Historical tool-only requests can show TTFT from their existing response-byte
timestamp independently of missing generation clocks.
New timings are flattened onto REST list/detail and WebSocket flow updates, stored
independently of bounded live-flow retention, and reused for numeric sorting and
filtering in the shared request table.

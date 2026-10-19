# Exact request and response capture

Admin → Observability → **Exact HTTP capture directory** enables private capture
of complete HTTP entity bodies. Production/dev use `/data/request-wire` on their
own persistent volumes. It applies to new instrumented inference requests live.

The request inspector uses separate Headers, Inbound request, Upstream request,
Upstream response and Inbound response tabs. Copy uses the original UTF-8 bytes,
including whitespace, escapes, BOM and SSE framing; Download also preserves
binary and compressed bodies. There is no body-size ceiling, preview clipping,
JSON pretty-printing, redaction or media substitution in this archive or viewer.
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

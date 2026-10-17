# Prepare images and checkpoints through Fleet

Deployment → **Prepare images and checkpoints** creates durable jobs for a pinned
recipe revision on selected hosts. Hosts are sorted; occupied hosts can prepare
without stopping their current model or reserving its GPUs. Fleet owns image
pulling, checkpoint fetching, verification and local cache writes. SQL keeps intent,
provenance, progress and the last safe error across controller/browser reconnects.

Recipes has typed **Checkpoint requirements** inputs for repository IDs and full
40-character HF commits. Requirements are immutable revision data. They must match
the target/draft selected by the serving command; changing this list does not
rewrite that command. LiL URL import extracts the configurator's exact Compose and
pinned target/draft requirements from its public resolved-command panel. This also
works when `/compose.yaml` is forbidden but the public page contains the source.
No page scripts run and no arbitrary source origin/redirect is accepted.

Host settings → **Checkpoint cache** selects HF_HOME, the approved private
`HF_TOKEN` environment-file reference and 1–32 download workers. The directory must
be inside an approved mount root and mounted writable into the agent. Fleet checks
that the recipe's actual HF bind maps to that directory; a mismatched cache cannot
produce a successful preparation receipt. Standard HF_HOME/hub layouts are
supported. Bootstrap changes to an agent's bind mounts still use the existing
runtime-maintenance procedure; model preparation does not use SSH.

An optional ownership repair handles checkpoint repositories previously written
by root containers. Fleet uses a fixed bounded CPU utility with only the declared
repository/lock directories mounted. It has no GPU, network or HF credentials.
New downloads are written by Fleet's UID/GID. Keep this host option off unless
legacy cache ownership needs repair.

The shared sortable/filterable job table separates **verified** bytes/files from
newly **transferred** bytes, displays average transfer rate, phase/current file and
errors, and provides Cancel / Prepare again. Host-side checksums and HTTP byte
ranges allow reuse/resume, including interrupted HF Hub downloads. Repeating a
job uses a new identifier and retains its pinned recipe revision. Cancelled/failed
jobs leave completed cache and resumable partials intact.

Start automatically waits for declared checkpoint preparation to complete. Unload
continues to work during preparation. Recipes without checkpoint requirements keep
their existing behavior; explicit preparation then pulls only the image. A completed
preparation is separate from readiness, inference smoke tests and benchmark evidence.
It proves presence/hashes at preparation time; it does not promise memory fit.

Jobs for the same host queue visibly in controller order; Fleet enforces one active
preparation locally. The mesh transports bounded typed Start/Get/Cancel commands,
never shell commands or token values. Image/host/recipe references remain pinned.
Policy changes stop incompatible active preparations. Cross-host cache copying is
not implemented and there is no SSH transfer fallback.

The controller retains at most 256 job receipts; Fleet retains 64 per host. These
bounds remove old terminal job records, not checkpoints or inference containers.
Native protocol, host permissions and failure behavior: `~/fleet/docs/preparations.md`.

# Dashboard configuration management

The dashboard manages gateway configuration in SQL. Start the controller with
`llmconduit supervise --config bootstrap.yaml --state-dir /data/managed-settings
--with-debug-ui`. The first start imports YAML and supported environment overrides;
subsequent starts read SQL. Legacy SQL routing precedence is imported once,
then managed settings own those fields. Files remain bootstrap and recovery inputs.

| Page | Configuration |
| --- | --- |
| Infrastructure → Aliases | Public names, ordered profile fallback; live SQL management |
| Infrastructure → Profiles & routing | Operational and legacy profiles, served-model remapping, ordered backend choices, templates, context/capability defaults, unknown-model handling |
| Infrastructure → Provider settings | Backends, credentials, upstream/fallback chains; staged live apply |
| Infrastructure → Providers | SQL provider discovery, create/edit/disable, credential replace/clear; live revisioned updates |
| Infrastructure → Host settings | Per-metric interval/off, RAM/VRAM admission budgets, sharing and container allowlists; Fleet applies locally |
| Admin → Access activation | Enable/disable policy-backed authorization, migrate existing virtual keys, policy database path |
| Admin → Gateway | Timeouts, request/SSE bounds, cooldown, replay/image caches, generation defaults, prefix and pricing |
| Admin → Integrations | Search and vision configuration with masked credentials |
| Admin → Authentication | Dashboard login/SSO/session/origin environment and legacy API authentication; account passwords remain on Account/Users |
| Admin → Observability | Engine metrics, native-vision probes, session detection, capture/logging and request/media retention |
| Admin → Service & storage | Listener, mesh configuration, SQL/JSONL/disabled history, database connection and persistence queue |

Existing Recipes, Deployment, Benchmarks, Traffic, Access, Accounts and mesh
administration continue using their existing APIs. Editors reuse shared panels,
tables and status icons, with mobile layouts. Settings use labeled controls, supported
choice lists, units, numeric bounds and inline help. Nested settings have dedicated
fields; profiles and providers use shared tables with add/copy/edit and ordered
backend selectors showing readable names. Copies receive fresh IDs and do not
reuse masked credentials. Existing custom fields survive structured edits.

Host settings use numeric sampling intervals with unit selectors and Collect
toggles, plus RAM/GPU budget and allowlist controls. Physical GPU totals and GPU
identity are read-only; Fleet still validates policy changes against loaded workloads.

Settings have no JSON editors. Backend extensions use named, typed values with
recursive list/object controls; client matching and extraction support header
regular expressions, body fields and nested conditions. Role rules retain their
backend wire format. Help icons explain each configuration section on hover,
keyboard focus or tap. Panel and entry actions are in top-right headers. Startup overrides are
separate from document controls; sign-in and SSO options have readable labels.
Changing a document setting removes its existing override. Server-side validation,
revision checks and stage/apply semantics remain authoritative.

## Stage and apply

Edit a field or use Add/Edit/Copy/Remove directly to start a local draft, validate
and stage, review the revision, then **Apply staged
configuration**. Apply includes all staged pages. Revision checks prevent stale
editors overwriting newer settings; the draft freezes the revision on the first
interaction, including opening an entry editor. A failed save retains the draft;
Discard restores saved values and closes unfinished entry editors.
Reads require administrator access; writes also require the existing session and
CSRF protection. Configuration administration remains available when ordinary
mutations are disabled, so an administrator can reenable them through Authentication.

The supervisor validates the complete candidate with its exact environment and
classifies its **effective** resource changes. Ordinary edits apply live: the child
prepares a complete immutable router/gateway generation and atomically publishes it
for new requests. Existing HTTP/SSE/WebSocket/media requests retain their original
configuration, clients, timeouts and routing. No drain, cancellation or gateway
restart occurs. Private revisioned acknowledgement makes a lost response retryable;
failed preparation leaves the previous generation active.

Only changes to these resources use the restart/migration path:

- Listener bind address/port.
- Database connection URL (including connection credentials).
- Storage backend/directory, persistence queue capacity, policy database path or
  mesh state database path.
- Mesh transport enablement, bind address, controller identity file or worker
  connection target/identity.

For these changes the supervisor drains the gateway child for up to 35 seconds,
starts the candidate and checks its private revision-specific readiness endpoint.
Previously connected hosts must reconnect when the controller remains enabled.
Failure restores the last applied revision. This path briefly interrupts the single
controller. The page displays the actual pending restart reasons across all staged
pages. Explicit defaults and document changes hidden by unchanged environment
overrides do not accidentally require a restart.

Live generations preserve flow history, cancellation handles, metrics, session
lineage/display counters, traffic admission/affinity state and connected mesh
sessions. Periodic collectors/reconcilers are replaced independently; old inference
tasks are never retired with them. Retention, logging/capture, session detection,
mesh heartbeat timeout and model approval rules also apply live. Mesh approvals
preserve active capacity reservations and do not reset host liveness.

Settings show document values, running scalar values where available, environment
source overrides and applied revision. Editing a matching field removes its
known environment override. Secrets are never returned: leave the marker unchanged
to keep, enter a replacement, or explicitly clear. Array entries retain IDs and
restore credentials by ID rather than position. Referenced `api_key_env` variables
are managed as provider credentials. Unknown document fields survive round trips.

## Access and data preservation

First policy activation generates a private pepper/bootstrap credential and imports
active YAML/SQL virtual-key hashes without changing their raw client secrets,
identities or model restrictions. Newly issued/rotated keys use HMAC. SQL revocation
and scope changes update migrated keys; import never undoes policy revocation.
Wildcard-like legacy model names are rejected instead of widening their scope.
Production policy enforcement stays **Off** until an administrator explicitly applies it.

Changing SQL storage copies accounts, credential hashes, aliases, deployment/recipe
state and history into an **empty** reachable destination after the child stops.
The destination transaction rejects populated databases and the source is retained.
If a later readiness check fails, the original database remains active; use a fresh
empty destination on retry. Policy and mesh state SQLite moves use native backup;
controller identity moves preserve the original bytes and reject conflicting identities.
The initial management SQL database remains the supervisor's settings authority and
must remain available after a history/database move. Keep the last SQL connection
when disabling history so account and model management continue using that database.

Listener changes can update Traefik's internal upstream through the optional
`LLMCONDUIT_MANAGED_INGRESS_FILE` deployment mount. DNS, certificates, external
published ports, OS mounts and worker bootstrap addresses remain deployment prerequisites.
Changing the public origin may require signing in at the new origin. Mesh port changes
that prevent enrolled workers reconnecting roll back; configure host transport first.
Build-time dashboard embedding and Fleet's initial host-owner remote-management opt-in
are installation settings, not runtime dashboard controls.

## Fleet ownership

Fleet exposes bearer-protected GET/PUT `/v1/settings`, forwarded over the pinned mesh.
Writes require host-owner `--allow-remote-settings`, disabled by default upstream.
The dashboard shows this capability. Fleet validates fresh hardware capacity, immutable
GPU inventory and all loaded/pending workloads before saving a revision privately next
to durable desired state. No inference container is restarted by a settings edit.
Cadence changes replace the collector timers; GPU/container helpers are supervised.
Observed revision advances after a new sample. Accepted settings are also stored in
central SQL. Offline hosts reject writes; there is no offline queued settings rollout.

Agent collection can be 100ms–60s for host/GPU/storage metrics, and 1s–60s for
container metrics. Mesh reporting remains 5s and SQL host history defaults to 15s;
fast local sampling does not retain every intermediate sample. These are distinct
cadences, clearly identified on Host settings. Remote edits supersede bootstrap
sampling and policy files on restart; use the page or central telemetry helper thereafter.

## Validation

Rust suite and OpenAPI route inventory, Clippy, SQL migration/key-scope tests,
Fleet race/vet checks, 802 frontend tests, typed/linted frontend build and desktop
page baselines cover these APIs. Browser workflows cover explicit stage/apply,
direct row actions, nested typed fields, help and header placement and all nine pages at 390px.
Native/live rollout results are recorded in the operations repository.

The native lifecycle harness is reproducible with a built release binary:
`python3 tests/managed_settings_runtime.py`. It uses a temporary private SQLite
store and fake loopback upstream; it reads no production credentials or configuration.

The native harness additionally holds one streaming and one buffered inference
request for more than 35 seconds while applying routine settings, reducing the
new-request timeout to two seconds, changing profiles/credentials and deliberately
failing resource preparation. It checks the unchanged gateway child PID, complete
old responses and the new request's settings/credentials.

Account API keys support in-place label/model/owner edits and token rotation.
See [editability audit](dashboard-editability.md) for the update contract and
remaining create-only Access records. Key changes publish live without restart.

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
| Infrastructure → Provider settings | Bootstrap backends, credentials, upstream/fallback chains; staged restart |
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
tables, status icons and local Monaco JSON editing, with mobile layouts.

## Stage and apply

Edit settings, validate and stage, review the revision, then **Apply staged
configuration**. Apply includes all staged pages. Revision checks prevent stale
editors overwriting newer settings; a failed save retains the local draft.
Reads require administrator access; writes also require the existing session and
CSRF protection. Configuration administration remains available when ordinary
mutations are disabled, so an administrator can reenable them through Authentication.

The supervisor validates the complete candidate with its exact environment,
drains the child gateway for up to 35 seconds, starts the candidate, and checks
its private revision-specific readiness endpoint. Previously connected mesh hosts
must reconnect when the controller remains enabled. Failure restores the last
applied revision. The page polls for the result after reconnecting. This is a
single-controller restart with a brief availability interruption, not HA rollout.

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
Fleet race/vet checks, 784 frontend tests, typed/linted frontend build and desktop
page baselines cover these APIs. Browser workflows cover explicit stage/apply,
Monaco add/format without accidental form submission and all nine pages at 390px.
Native/live rollout results are recorded in the operations repository.

The native lifecycle harness is reproducible with a built release binary:
`python3 tests/managed_settings_runtime.py`. It uses a temporary private SQLite
store and fake loopback upstream; it reads no production credentials or configuration.

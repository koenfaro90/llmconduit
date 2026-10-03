# Fleet machine management

Agreed direction, 2026-10-03; host telemetry implemented on development. Development work lands
on `development` in both GitLab repositories under
`de-wielick/de-wielick/llm-tools`: `llmconduit` and `fleet`.

Fleet runs on each model host and reports local hardware, CPU, memory, network,
GPU/UMA resources, inference processes, containers, readiness and inference
metrics. It is the local authority for pulling images and creating, starting,
stopping and replacing containers, validating reservations and reporting conflicts.
llmconduit owns the recipe catalog and recipe versions, desired deployments,
host/GPU placement, distributed ranks, recovery,
aliases/routing and the monitoring/dashboard views. Multi-node operations have
one central operation and per-host rank progress; no Fleet agent instructs
another host. Keep RTX deployments based on the existing `~/compose` recipes.

Measurements go directly to llmconduit. Reuse the existing PostgreSQL store where
durability is needed; no Prometheus dependency. Retention and downsampling are
out of scope. llmconduit manages recipes and durable placement intent;
live reports describe actual state. Routes are published after inference
readiness, and central disconnection leaves inference running.

## Recipe management and execution

Recipes are managed through llmconduit's API and dashboard: import, create, edit,
validate, version and select them for deployment. Persist the catalog and revisions
centrally. Git may supply imports and receive exports; it does not require operators
to edit machine-local manifests to manage recipes. A deployment references an
immutable recipe revision, so editing a recipe does not silently change running
containers. Applying a new revision is an explicit deployment operation.

Recipes describe the model/checkpoint revision, image, engine arguments and
environment, ports/mounts, resource requirements, readiness and distributed
topology. This includes context/batch/sequence limits, prefix caching, prompt-token
details and backend/kernel selection. Keep credentials as references resolved on
the host; plaintext secrets must not enter recipe exports or dashboard responses.

llmconduit resolves a selected revision against host capabilities and placement,
then sends each host an instruction containing the recipe ID/revision, resolved
local launch specification, deployment/operation ID and assigned GPUs/rank.
Fleet validates it against local policy/resources, durably records acceptance,
pulls the image and performs local container lifecycle actions. Fleet reports
the applied revision and actual state. Its cached configuration supports restart
and disconnected operation; the authoritative recipe catalog remains in llmconduit.

Import the existing Spark recipes and GPU Compose recipes without dropping their
tested settings. Preserve GPU execution paths under `~/compose` as local execution
material. Reuse sparkrun rendering/fabric knowledge while Fleet owns execution on
each machine. Central orchestration never becomes a second local Docker lifecycle
owner. Fleet's own update supervisor is part of Fleet's local authority.

Recipe management and applying resolved recipes over the control protocol are
partially implemented. Central immutable recipes, desired single-host deployments,
managed recipe acceptance, local durable state and host policy are implemented.
See [managed deployment API](../fleet-managed-deployments.md). Standalone Fleet accepts a strict machine-local manifest and
legacy mesh model switching remains supported. Further work covers imports, GPU
admission, distributed placement and route reconciliation.

## Controller updates

Image delivery is provisioned alongside the production gateway: the rootless
registry at `llmconduit.wielick.lan/fleet` serves an ARM64/AMD64 image index through
Traefik's existing HTTPS endpoint (`/v2/`). All twelve model hosts have verified
pull-only credentials. Publishing credentials remain on the deployment server;
Git SSH deploy identities are separate. The control repo records the published
source commit and immutable index in `gateway/production/fleet-image.json`.
Controller handover remains to be implemented.

llmconduit must be able to tell a selected host to start a new Fleet version.
Fleet first starts the replacement in standby and verifies it, transfers local
lifecycle ownership, then stops/removes the old controller. Updating Fleet must
leave that host's inference containers running.

The detailed local state machine, recovery and acceptance criteria are in the
[Fleet controller update contract](https://gitlab.com/de-wielick/de-wielick/llm-tools/fleet/-/blob/development/contrib/self-update.md).
Use a small machine-local supervisor for a stable loopback control URL and crash
recovery, separate candidate ports during overlap, persistent operation/adoption
state, and exclusive local lifecycle ownership. A healthy HTTP process alone is
insufficient: verify build/state/control-protocol compatibility and report the
new active generation centrally before retiring the old controller. Failed
pre-activation updates retain old; recovery after ownership transfer chooses one
controller. Preserve compatible state and the old image through rollback.

Central update operations must be durable and idempotent. Track expected active
generation, operation UUID, desired image digest/source commit, per-host phase,
active/candidate build, sanitized error and timestamps. A reconnect resumes
tracking the same operation. Do not authorize arbitrary Docker commands, mounts,
secrets or host configuration through an update request.

Use existing dashboard administrator authentication, mutations gating and CSRF
for the host update action. Add an explicitly advertised, versioned mesh control
capability to forward it to the worker's local Fleet client. Keep Fleet tokens
env/file-selected and local; they must not enter central config, mesh messages,
dashboard responses or logs. Add wire round-trip and authorization tests when
implementing the protocol. Unsupported workers should show a capability error.

## Current seams and implementation order

`src/dashboard_fleet.rs` contains the loopback-only `FleetClient` and local model
controls. `src/mesh/worker.rs` forwards a model switch to that client;
`src/mesh/protocol.rs` currently supports inference and switch-model streams.
Host telemetry, recipe management/distribution, central placement and controller
update messages are still to be built. No self-update endpoint exists in current
Fleet either.

1. Read-only inventory and direct reporting on one Spark and one RTX host;
   surface observed host/process/inference state in llmconduit.
2. Central recipe catalog/API/dashboard and immutable recipe revisions; durable
   desired-state tracking and Fleet local recipe acceptance/state/adoption,
   reservations and lifecycle operations. Handle GB10 unified memory rather
   than expecting numeric dedicated VRAM readings.
3. Controller standby, supervisor handover and central update operations, with
   failure injection at every handover phase and sustained inference testing.
4. Distributed rank coordination and automatic ready-backend route reconciliation.

gpu03 is authorized for testing and OpenJev may be taken down when needed.
At the 2026-10-03 inspection only gbspark01 was idle; there was no free Spark
pair. Recheck live occupancy before deployment tests, and do not displace the
other inference services for bootstrap work.


## First implementation: live host reports

Fleet now caches a bounded `GET /v1/host` report every five seconds. A mesh worker
reads the local bearer-authenticated loopback API and adds an optional
`host_snapshot` to its existing authenticated heartbeat. The controller validates
schema, metric ranges, strings, inventory sizes and a 32 KiB report budget before
accepting it. Legacy heartbeats remain compatible; an unavailable Fleet clears
the report, and a disconnected/stale mesh session is not projected as live.

The existing admin-only `/dashboard/api/mesh` response includes the latest
snapshot per node. Providers → Fleet hosts shows CPU/load-source observations,
host RAM, disk capacity, per-interface RX/TX rates, GPU utilization/temperature/
power and running container statistics. Samples older than 20 seconds are marked
stale; absent measurements display unavailable. GB10 has unified host RAM and
null dedicated VRAM. No Fleet tokens, container environment or launch commands
cross the mesh or dashboard API.

A worker may have `resources: []` when observing a host with no deployed models.
It still requires a pinned controller endpoint, its own identity and enrollment;
it advertises no inference route. On each host configure
`LLMCONDUIT_FLEET_URL=http://127.0.0.1:8090` and an env/file bearer token source.
Fleet's standalone `contrib/compose.observer.yaml` uses host networking, host
`/proc` and a Docker proxy that permits only reads. Execute under `~/compose`.
Both ordinary Fleet and observation-only mode expose the report endpoint.

This slice does not yet implement the central recipe catalog, applying resolved
recipes, reservations, distributed lifecycle operations, inference-engine metrics
or Fleet supervisor handover. These remain the next phases of the design above.

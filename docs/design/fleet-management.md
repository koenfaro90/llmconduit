# Fleet machine management

Agreed direction, 2026-10-03; not deployed functionality. Development work lands
on `development` in both GitLab repositories under
`de-wielick/de-wielick/llm-tools`: `llmconduit` and `fleet`.

Fleet runs on each model host and reports local hardware, CPU, memory, network,
GPU/UMA resources, inference processes, containers, readiness and inference
metrics. It validates reservations and executes local instructions. llmconduit
owns desired deployments, host/GPU placement, distributed ranks, recovery,
aliases/routing and the monitoring/dashboard views. Multi-node operations have
one central operation and per-host rank progress; no Fleet agent instructs
another host. Keep RTX deployments based on the existing `~/compose` recipes.

Measurements go directly to llmconduit. Reuse the existing PostgreSQL store where
durability is needed; no Prometheus dependency. Retention and downsampling are
out of scope. Git holds profiles; central durable state holds placement intent;
live reports describe actual state. Routes are published after inference
readiness, and central disconnection leaves inference running.

## Controller updates

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
Host telemetry, central placement and controller update messages are still to
be built. No self-update endpoint exists in current Fleet either.

1. Read-only inventory and direct reporting on one Spark and one RTX host;
   surface observed host/process/inference state in llmconduit.
2. Durable central desired-state tracking and Fleet local state/adoption,
   reservations and lifecycle operations. Handle GB10 unified memory rather
   than expecting numeric dedicated VRAM readings.
3. Controller standby, supervisor handover and central update operations, with
   failure injection at every handover phase and sustained inference testing.
4. Distributed rank coordination and automatic ready-backend route reconciliation.

gpu03 is authorized for testing and OpenJev may be taken down when needed.
At the 2026-10-03 inspection only gbspark01 was idle; there was no free Spark
pair. Recheck live occupancy before deployment tests, and do not displace the
other inference services for bootstrap work.

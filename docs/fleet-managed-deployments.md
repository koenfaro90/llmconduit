# Central recipe management and desired deployments

The authenticated admin dashboard exposes immutable recipes and per-host desired
containers, backed by the existing SQL store (SQLite or PostgreSQL). The Providers
view contains recipe creation, revision selection, deployment creation and start/stop.
Only connected workers advertising managed Fleet support appear in the host selector.
This is the single-host execution stage; automatic route publication, distributed
ranks, GPU/UMA admission, existing Compose/sparkrun import and Fleet self-update
handover remain further work. Model inference services have not been migrated.

## API

- `GET /dashboard/api/management` returns recipes and desired/observed deployments.
- `POST /dashboard/api/management/recipes/{id}/revisions` takes
  `{description, model_spec}` and returns the immutable content digest and revision.
  Repeated identical specifications return the existing revision. Images must be
  pinned with a SHA256 digest; memory_limit and user are required. Runtime/API
  configuration, privilege/capability overrides and inline credential keys are
  excluded. The specification uses Fleet model fields without the deployment ID.
- `PUT /dashboard/api/management/deployments/{id}` takes
  `{expected_generation, endpoint_id, recipe_id, revision_digest, desired_state}`.
  Use generation zero for creation. The controller stores new desired state before
  dispatch; `202` means queued, not ready. A revision change requires an observed
  unloaded container. A deployment's host remains fixed; use a new deployment ID
  for a different host. State is `ready` or `unloaded`.

Every mutation enforces administrator access, mutation policy and CSRF. State uses
SQL compare-and-swap, including across controller processes; conflicting edits fail
or retry without losing other changes. Catalog capacity is bounded to 64 recipes,
32 revisions per recipe, 64 deployments and a 4 MiB total serialized document.
Each specification is bounded to 32 KiB. No credentials are distributed through
this catalog; host-local read-only mounts provide application secrets when allowed
by host policy.

Every five seconds the controller dispatches desired state over the enrolled,
controller-pinned mesh connection with bounded concurrency and timeouts. Management
is capability-gated, uses a separate bounded control frame, and checks response IDs.
Only the worker holds the local Fleet bearer token. Fleet enforces local policy,
atomically persists acceptance and runs its existing lifecycle manager. Repeated
commands are idempotent. Both ends reject older generations. Connection failures
are visible centrally and retried; last accepted local desired state survives.
Host metrics continue over the same authenticated mesh channel.

The central record includes observed status, observation time and an error. Central
acceptance does not assert local readiness. The host's readiness check determines
whether a container becomes ready; the dashboard marks old observations stale.
A local Fleet restart adopts its accepted specifications and existing containers.
Central disconnects do not unload anything. Local managed lifecycle writes have
one source: legacy load/unload APIs are disabled in that mode.

Development execution material stays in `~/compose/fleet-observer`: immutable
Fleet image, host-owned manifest/policy, private token, durable `.managed-state`,
worker identity and configuration. See the control repository's live pilot notes
for image revisions, host placement and verification evidence. Production inference
routes and its PostgreSQL deployment remain independent of this pilot.

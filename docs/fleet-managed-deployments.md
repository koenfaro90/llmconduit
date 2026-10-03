# Central recipe management and desired deployments

The authenticated admin dashboard exposes immutable recipes and per-host desired
containers, backed by the existing SQL store (SQLite or PostgreSQL). The Providers
view contains recipe creation, revision selection, deployment creation and start/stop.
Only connected workers advertising managed Fleet support appear in the host selector.
Production inference is managed on all twelve enrolled model hosts. Compose and
sparkrun sources are retained as catalog revisions; executable imports preserve
the existing images, commands, environment, mounts and LiteLLM ports. Fleet
enforces GPU exclusivity and host memory budgets. Distributed Qwen uses two
explicit per-host rank deployments. Transactional group coordination, automatic
route reconciliation and Fleet self-update handover remain further work.

## API

- `GET /dashboard/api/management` returns recipes and desired/observed deployments.
- `POST /dashboard/api/management/recipes/{id}/revisions` takes
  `{description, model_spec}` and returns the immutable content digest and revision.
  Repeated identical specifications return the existing revision. Images must be
  pinned with a SHA256 digest; memory_limit and user are required. Runtime/API
  configuration and inline credential keys are excluded. Privilege/capability
  requests need explicit host-policy approval. The specification uses Fleet model fields without the deployment ID.
- `PUT /dashboard/api/management/deployments/{id}` takes
  `{expected_generation, endpoint_id, recipe_id, revision_digest, desired_state}`.
  Use generation zero for creation. The controller stores new desired state before
  dispatch; `202` means queued, not ready. A revision change requires an observed
  unloaded container. A deployment's host remains fixed; use a new deployment ID
  for a different host. State is `ready` or `unloaded`.

Every mutation enforces administrator access, mutation policy and CSRF. State uses
SQL compare-and-swap, including across controller processes; conflicting edits fail
or retry without losing other changes. Catalog capacity is bounded to 256 recipes,
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

Host execution material stays in `~/compose/fleet-observer`: immutable
Fleet image, host-owned manifest/policy, private token, durable `.managed-state`,
worker identity and configuration. See the control repository's management notes
for image revisions, host placement and verification evidence. The production
controller stores recipes and placements in its existing PostgreSQL deployment.
Controller restarts and host-agent upgrades retain accepted inference containers.

Imported inference may use local immutable `sha256:` image IDs, private
`environment_files`, `devices`, `group_add`, `cap_add` and `privileged` options.
The controller stores these requests; Fleet's host-owned policy grants each
exception explicitly. Numeric token-count settings are permitted; credential
keys such as HF_TOKEN remain forbidden inline. Ports belong to recipe revisions
and remain unchanged when adopting an existing LiteLLM endpoint.

Original Compose and Spark recipe sources use `model_spec` with
`source_format` (`compose` or `sparkrun`) and `source_content` (YAML, 32 KiB
maximum). Literal credentials are rejected. They are immutable editable catalog
revisions and cannot be selected for execution until rendered into a Fleet
specification. The dashboard Edit revision control loads either form into the
editor. Archived sources are retained without starting competing workloads.

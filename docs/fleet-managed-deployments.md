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

## Importing Compose and sparkrun

The recipe editor accepts pasted or uploaded Compose and sparkrun YAML. Select the
source format, enter a recipe ID and description, and choose **Preview conversion**.
Provide any missing image digest, container user, RAM budget, GPU device IDs,
target-host directories, template variables or private environment-file paths.
The preview shows the complete Fleet specification, missing inputs and warnings.
Changing the source or settings invalidates the preview. **Save converted revision**
retains both the editable source and its executable result as immutable revisions
under the same recipe ID. Select the executable revision and an enrolled host, then
use **Deploy**; saving or previewing does not start containers or change live ports.

`POST /dashboard/api/management/recipes/convert` accepts:

```json
{
  "source_format": "compose",
  "source_content": "services:\n  inference:\n    image: example:v1\n    ports: ['8001:8000']\n",
  "options": {
    "image": "example@sha256:<64 hexadecimal characters>",
    "user": "1000:1000",
    "memory_limit": "64g",
    "gpus": "device=0",
    "base_directory": "/home/koen/compose/inference",
    "home_directory": "/home/koen"
  }
}
```

The response contains `source_format`, `services`, `model_spec`, `required_inputs`
and `warnings`. Nonempty `required_inputs` prevents saving through the converted
revision control. Invalid YAML, inline credentials and invalid option values return
400 without echoing submitted content. This side-effect-free endpoint uses the
same administrator/mutation/CSRF policy as recipe writes. Sources are bounded to
32 KiB and options to 16 KiB; conversion reads no host files and runs no commands.

Compose conversion supports one selected service, YAML anchors, data-only variable
substitution, commands/entrypoints, environment, required host-local environment
files, bind mounts, fixed published ports, explicit NVIDIA device reservations,
memory/shm limits and common Docker execution settings. Container HTTP healthchecks
are translated to published host ports (for example 8000 to 8001). Additional
services must be converted and deployed separately. Build directives, named volumes,
custom network orchestration and unsupported execution settings require explicit
conversion; they are reported as blocking inputs rather than silently dropped.
Labels, Compose names and dependencies are not applied to managed containers.

Sparkrun v1/v2 conversion currently supports explicit vLLM command templates,
recipe defaults/overrides, environment and supported executor settings. It shows
Hugging Face and isolated runtime cache bind paths; `runtime_cache_directory` can
select an existing approved cache. Distributed recipes require explicit node count,
rank, head IP and this host's IP. Convert each rank separately and preserve the
required NCCL/fabric environment and devices through `environment` and `devices`.
Headless ranks use container liveness; verify inference on the head. Conversion
does not run sparkrun transfers, prepare host directories, install secrets, modify
Fleet policy, coordinate ranks atomically or create gateway model aliases.

## Dashboard pages

- **Hosts** (`#/hosts`): informational Fleet host inventory, desired workloads,
  CPU/RAM/GPU and network graphs, per-interface and container details. Shared
  `DataTable`, `LineChart`, `Panel` and `StatChip` components provide the presentation.
  Charts retain up to 360 samples per host (30 minutes at five-second refresh) while
  the page is open; gaps/unavailable measurements remain null, and old samples are
  visibly stale. This adds no metrics-retention service.
- **Recipes** (`#/recipes`): searchable/paginated immutable revision catalog and
  source/Fleet editing and conversion controls.
- **Models** (`#/models`): running/desired model containers, host, pinned recipe
  revision, observed state and expandable complete launch configuration. Request
  activity comes from SQL, not the bounded live FlowStore: last inference request
  and counts for 1 minute, 15 minutes, 1 hour, 4 hours and 24 hours. Counts aggregate
  by served model over its gateway backends and exclude health/catalog polling.
  Requests sent directly to LiteLLM/backends are outside gateway observations.
- **Deployment** (`#/deployment`): recipe/host selection and desired/observed
  deployment controls. Recipe editing is separate from placement/lifecycle.
- **Providers** retains upstream configuration, mesh enrollment/admin and legacy
  Fleet integration controls, plus routing/authorization/provider inventory.

`GET /dashboard/api/management/model-activity` returns a bounded body-free SQL
aggregate of retained inference history. It is administrator-only, has a five-second
query timeout, and returns unavailable on missing persistence or exceeded bounds.
The last-request timestamp can be older than 24 hours. Window counts are not
inferred from host heartbeats or a page's live event buffer.

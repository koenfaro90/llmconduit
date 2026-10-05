# Dashboard configuration ownership

Audit dated 2026-10-05. The dashboard should be the operator interface for all
configuration changes. SQL stores authoritative desired settings; files become
bootstrap inputs and export/restore artifacts. A form is incomplete unless its
Save/Apply action changes the running service or schedules the required restart.

## Current gaps

| Area | Current source and limitation | UI home / required behavior |
| --- | --- | --- |
| Client model aliases | Infrastructure → Aliases; SQL `model_aliases` after initial YAML import | Live create, rename, target/fallback editing and delete are implemented. YAML/config sync cannot overwrite SQL. |
| Policy-backed API authorization | Gateway `auth.mode`, `auth.store_path`, environment pepper/bootstrap credential; initialized by `AuthzService::from_config` | Admin → Access needs an activation/migration flow, not instructions to edit YAML. Import existing keys, prepare identities/policies, validate operator access, then enforce. Disabling/reenabling and changing the policy store must also be supported. |
| Model profiles and routing | `control_plane.operational` backend/profile arrays; gateway `model_profiles`, templates, `model_routes`, legacy upstream/fallback arrays | Models needs a profile/defaults editor and Providers needs routing, edit, disable and credential rotation. Include served-model remaps, ordered backend choices, unknown-model policy, context/vision/tool/role settings, prefixes and generation defaults. Recipe engine/container settings already belong to Recipes. |
| Gateway behavior | `PersistedConfig`: request/connect timeouts, body/SSE bounds, cooldowns, replay/image caches, unsupported-image behavior and completion defaults | Admin → Settings, grouped by purpose, with schema validation, effective values and source attribution. |
| Integrations and secrets | Brave/search config, image-agent vision endpoint/model, provider credentials; dashboard GitHub SSO and login settings in environment | Settings → Integrations/Authentication plus write-only credential fields in Providers. Mask stored secrets; permit replace/clear without returning their values. Preserve environment overrides or migrate them explicitly so a successful save cannot be silently ignored. |
| Host telemetry and machine policy | Fleet arguments/environment/Compose for sampling; local device, RAM/VRAM budgets, allowlists and sharing policy | Host detail → Settings: per-metric interval/off, host budgets/sharing/allowlists. Persist desired state centrally; Fleet validates and applies locally. Distinguish agent-local collection, mesh reporting and SQL history cadence. |
| Observability | Gateway metrics scrape/observer URLs, metric polling config, native-vision probe settings, harness/session detection, request capture/logging and media retention settings | Settings → Observability; expose intervals, capture and supported storage options. Fleet sampling controls belong to Hosts. |
| Service/bootstrap settings | Bind/listen addresses, mesh identity/controller/worker addresses and heartbeats, dashboard origin/session configuration, SQL connection/storage and deployment environment | Settings → Service: validate, stage, show restart impact and apply through the host's deployment operator. Connection/session changes require an explicit reconnect/result workflow. Existing identities and database contents must survive. |

Existing functional management includes Recipes/revisions/sizes and evidence,
Deployment/physical GPU placement, Benchmarks, Traffic admission policies,
mesh node/model administration, SQL-configured provider creation/removal,
dashboard users/keys, and policy-backed users/groups/roles/policies when enabled.
Host metric selection and table/filter preferences are viewing preferences,
not agent sampling or machine admission settings.

The Access page's file reference is specifically `auth.mode: enforce`.
Production's policy-backed authorization summary currently reports disabled.
That is separate from existing virtual-key API authentication and dashboard login;
adding the activation UI must not revoke valid existing clients accidentally.

## Apply contract

Every editor needs a validated, revisioned desired setting, a preview of the
change, source/override visibility, and observed applied revision. Live changes
must have an immutable per-request snapshot where routing/model policy is involved.
Cold settings need a staged restart with readiness verification and rollback;
showing a saved value without applying it is not sufficient. Host execution stays
with Fleet. Central reconciliation and rollout selection stay with LLMConduit.

Recommended implementation order: Access activation and key migration;
backend/profile management; host settings; gateway/integration/observability
settings; staged service/database/identity changes. This table records gaps,
not a claim that the missing editors or restart workflow already exist.

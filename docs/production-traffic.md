# Production admission and routing

Traffic is opt-in: **Off** preserves routing/admission behavior. **Observe** records active attempts and would-block reasons without queueing/reordering. **Enforce** applies configured limits. Policy lives in SQL (`traffic_policy_v1`), edited under Traffic with admin authentication, mutation permission, CSRF and revision compare-and-swap. Workers do not own global caller quotas.

Defaults, groups, users and API-key rules contribute limits. Priority weight uses the most specific override; concurrency ceilings intersect. Group `max_concurrent` is inherited per key; `shared_concurrent` counts all group members together. Model caps can refer to an explicit model budget shared by aliases. Traffic groups accept stable `user:<id>` and `key:<id>` members, including SQL-managed keys and owner inheritance. Existing authorization groups also contribute. Rules do not grant access: credentials and model/provider authorization are rechecked before dispatch.

Schedules use IANA timezones, ISO weekdays, overnight windows and optional calendar dates. Later matching weekly windows win; matching dated exceptions then override them. Equal start/end means all day. Active requests drain when limits are reduced or set to zero; queued work is reevaluated against current policy and time.

Admission acquires caller and backend budgets together, without occupying one while waiting for the other. Limits cover actual HTTP attempts until upstream streams/binary bodies end or are cancelled, including raw embeddings, reranking and media tasks. Known asynchronous video jobs retain capacity until terminal status and restore it from SQL after restart. Ambiguous job creation (accepted upstream but no usable/stored job handle) retains an in-process lease; its unknown remote lifetime cannot be recovered after restart. Mesh chat also applies caller limits and an optional aggregate `mesh://workers` budget; per-worker mesh capacity remains in the mesh registry.

The bounded queue defaults to disabled, 512 requests, 32 MiB estimated serialized request bytes, 32 waiters per key, 30 seconds wait and 10 seconds aging. Hard bounds: 4,096 requests, 256 MiB and five minutes. Metadata contains no prompts or session header values. Priority-weighted dispatch shares progress among group/user flows; aged work progresses and quota-ineligible waiters cannot block eligible work. Caller rejection is HTTP 429, backend admission rejection HTTP 503, with Retry-After. Queued requests can be removed in Traffic. Optional cancellation grace (0 by default, at most 30 seconds) conservatively holds backend slots after stream cancellation; it does not prove remote GPU work has stopped.

Backend caps are keyed by canonical HTTP base URL. Multiple URLs reaching one engine must share `resource`; the strictest cap and any pause apply to that physical resource. Map `deployment_id` to reserve/drain that engine for Fleet benchmarks when enforcing admission. Direct LiteLLM/backend calls bypass gateway accounting.

Balancing and affinity require an explicit compatible replica list per model/alias. Only providers advertising the same resolved model ID participate; ordinary routing providers remain independent. Balance by outstanding attempts relative to configured capacity. `Prefer` tries the session's pinned backend, waits its configured interval and can spill over before output. `Strict` waits/rejects on that backend instead of moving the session. Pins are scoped by caller and model budget, expire by TTL and commit only after successful output. Supply `X-LLMConduit-Session-Id` or `X-LLM-Session-Id`, or `prompt_cache_key`. Session values are hashed. Policy changes to backend/model routing clear pins. Failover still stops after first output. Routing cannot guarantee retained engine KV cache.

Gateway runtime metadata and affinity pins are bounded, in memory, and restart-local. One production gateway controller owns these counters; multiple active controllers would need distributed admission coordination before their limits could be global.

## Engine observations

Existing vLLM metrics report aggregate running/waiting requests, KV-cache usage and preemption counters. Completed request metrics are separately opt-in (`capture_engine_metrics`); supported vLLM builds need `--enable-per-request-metrics` and usage-bearing stream chunks. Latest 128 valid measurements show engine queue time, TTFT, generation time, mean ITL and output rate. Missing values are unavailable, never invented zeros. See [vLLM per-request metrics](https://docs.vllm.ai/en/latest/features/per_request_metrics/).

Fleet's optional scheduler observer adds privacy-filtered live per-request observations on compatible vLLM V1 recipes. These phases derive from scheduler progress, not GPU execution timestamps. Enablement and version requirements are documented in Fleet `docs/scheduler-observer.md`.

## Mobile

Compact mobile layout is an optional per-browser toggle, with a configurable server default. Navigation becomes a selector; shared tables keep essential columns and expandable details. Desktop views retain their full layout.

# Dashboard editability audit

Checked 2026-10-18 against the source routes, SQL methods and rendered actions.
The Account API-key editor is deployed with the revision recorded in the control
repository's `LLMCONDUIT.md`. Routine key changes apply live without restarting
the gateway or stopping active inference.

| Surface | Editable now | Remaining gaps |
|---|---|---|
| Account → API keys | Label, allowed client-facing models/aliases, owner (administrators); rotate and revoke tokens | Tokens cannot be recovered: rotation returns a new token once. Key IDs and creation times stay stable. |
| Account → users | Create/delete, reset password, change administrator role | Existing login usernames cannot be renamed. |
| Access → API keys | Create, rotate, revoke | Existing name, owning principal and expiry lack update routes/actions. This is the separate opt-in policy-backed system. |
| Access → users/service accounts | Create | Display name, kind and enabled state lack update actions/routes. |
| Access → groups | Create with initial members | Renaming and membership changes lack update actions/routes. |
| Access → roles | Create with initial permissions | Renaming, permission changes and disabling lack update actions/routes. |
| Access → policies | Create allow/deny and management policies | Existing rules, subjects, limits and schedules lack update actions/routes. “Save policy” creates a policy. |
| Infrastructure → aliases | Names and ordered target profiles | IDs are stable identities. |
| Settings pages, providers and profiles | Typed field and collection editors, live stage/preflight/apply | Listener, database, storage and mesh transport changes still require a supervised restart. |
| Hosts | Policy budgets, allowlists and sampling | Detected hardware identity/totals are observations, not editable settings. |
| Recipes | Create/edit/copy through new immutable revisions | Existing deployment pins do not change when a new revision is saved. |
| Deployment | Select recipes/sizes and ordered host/GPU placement, start/stop | Placement changes require observed unload; deployment IDs are stable identities. |
| Requests, telemetry, benchmark results and audit | Inspect/export; supported cancellation/lifecycle actions | Historical measurements and outcomes are evidence, not editable configuration. |

## Account key update contract

`PUT /dashboard/api/keys/{id}` accepts complete metadata:

```json
{
  "label": "Document worker",
  "user_id": "owner-user-id",
  "allowed_models": ["wielick-small"],
  "expected_updated_at_ms": 1792320000000
}
```

`label: null` clears the label; administrators can use `user_id: null` to remove
ownership. An empty `allowed_models` list allows any model, matching creation.
Owners can edit their own keys; administrators can edit any key and transfer
ownership. CSRF is required. A stale timestamp returns 409 and preserves the
current record; deleted keys are never recreated by editing or rotation.

`POST /dashboard/api/keys/{id}/rotate` accepts `expected_updated_at_ms` and returns
the existing key metadata plus a fresh `secret` once, with `Cache-Control: no-store`.
The old token immediately stops authenticating new requests. History, key-specific
traffic settings, owner and model limits retain the same key ID. Metadata-only
updates preserve the existing token and return no secret or digest. Mutations and
live registry publication are serialized; existing requests retain their auth
snapshot. Concurrent SQL edits use compare-and-swap on the current timestamp.

The remaining Access update gaps above are not implemented by the Account editor;
those features require explicit mutation contracts, delegated permission checks
and atomic policy snapshot publication in the Access backend.

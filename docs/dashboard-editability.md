# Dashboard editability audit

Checked 2026-10-18 against the source routes, SQL methods and rendered actions.
Account and Access editors are deployed with the revision recorded in the control
repository's `LLMCONDUIT.md`. Changes apply live for new requests; active inference
and session leases keep their current authorization snapshot. Access remains opt-in.

| Surface | Editable now | Boundaries |
|---|---|---|
| Account → API keys | Label, allowed client-facing models/aliases, owner (administrators); rotate and revoke tokens | Tokens cannot be recovered: rotation returns a new token once. Key IDs and creation times stay stable. |
| Account → users | Create/delete, reset password, administrator role and login username | Administrator-only username changes preserve account ID, passwords, keys and existing sessions. GitHub names belong to the identity provider. |
| Access → API keys | Create, edit name/owner/expiry, rotate, revoke | Metadata edits retain the token, key ID and history. Revoked keys stay revoked. |
| Access → users/service accounts | Create, edit display name/kind/enabled state | The bootstrap identity must remain enabled. |
| Access → groups | Create, rename, enable/disable and replace membership | Disabled groups stop contributing access on new requests. |
| Access → roles | Create, rename, enable/disable, permissions and assignments to identities/groups | Disabled roles stop contributing access on new requests. |
| Access → policies | Create or edit allow/deny decisions, subjects, endpoint/model/provider/route scope, management permissions, limits and multiple UTC schedule windows | Existing rows have an explicit Edit action; creation remains separate. |
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

## Account username updates

`PUT /dashboard/api/users/{id}/username` accepts `username` and the frozen
`expected_updated_at_ms`. Administrator authentication and CSRF are required.
Duplicate names and stale versions return 409. The account ID, creation time,
password and key ownership remain unchanged. Existing signed-in sessions continue
working, and `/dashboard/api/me` resolves the current username by account ID.
Password sign-in uses the new name immediately. Deleted accounts cannot be renamed.

## Access record updates

The separate policy-backed console provides `PUT /dashboard/api/auth/{collection}/{id}`
for `users`, `groups`, `roles`, `policies` and `api-keys`. Each accepts the complete
creation fields plus `expected_revision`; users/groups/roles/policies also accept
`enabled`, and roles accept `bindings` (`subject_kind`: principal/group, `subject_id`).
GET rows include a frozen global policy `revision`, groups include full `members`,
and roles include full `bindings`. Any intervening Access mutation makes an older
draft stale (409). Refresh/discard before retrying; the editor retains failed drafts.

Writes transact the record, its relationships, epoch and audit event together, then
publish the new immutable authorization snapshot. Unknown relationships, invalid
matchers/windows and zero limits are rejected without modifying stored state.
Names are bounded; duplicate group/role names return 409. Existing IDs and key
secrets remain stable. No raw credential or digest is returned by metadata edits.

User/group/role/policy edits require their corresponding `auth.*.write` permission;
key metadata adds the explicit `auth.keys.write` permission, independent of create,
rotate or revoke. The backend rechecks delegated authority from current stored grants
while holding its mutation lock. Bootstrap administrators can change grants,
membership, enabled state, ownership and expiry. Delegated editors can rename
records; role permission changes must stay within their own current grant and
cannot change assignments. The UI disables controls unavailable to the actor.

Enabled state is honored for identities, groups and roles. Disabling an identity
revokes its delegated dashboard sessions. Changing a key owner revokes dashboard
sessions authenticated with that key; current inference leases continue. Key expiry
is checked against the clock during authentication, so expiry needs no later edit
or reload. UTC policy windows use weekday/time controls and optional absolute UTC
bounds; key expiry uses the administrator's local time, clearly labeled.

This does not activate Access authorization. When Access is Off, its empty read
model remains visible and mutations are unavailable; Account API keys continue
working under the existing authentication mode.

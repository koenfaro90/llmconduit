//! In-place Access edits keep identities and credentials stable. One SQLite
//! transaction checks the frozen read revision, changes relationships and audits.
use super::*;
use crate::dashboard_access::{
    UpdateAccessGroup, UpdateAccessKey, UpdateAccessPolicy, UpdateAccessRole, UpdateAccessUser,
};

fn invalid(message: &str) -> AccessError {
    AccessError::new(StatusCode::UNPROCESSABLE_ENTITY, message)
}
fn missing() -> AccessError {
    AccessError::new(StatusCode::NOT_FOUND, "Access record not found")
}
fn grant_change(actor: &ManagementActor, changed: bool) -> Result<(), AccessError> {
    if changed && !matches!(actor, ManagementActor::Bootstrap) {
        return Err(AccessError::new(
            StatusCode::FORBIDDEN,
            "Changing access grants, membership or ownership requires a bootstrap administrator",
        ));
    }
    Ok(())
}
fn edit_tx(store: &mut AuthStore, expected: u64) -> Result<Transaction<'_>, AccessError> {
    let tx = store
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(access_db)?;
    if current_epoch(&tx).map_err(access_internal)? != expected {
        return Err(AccessError::new(
            StatusCode::CONFLICT,
            "Access settings changed; discard and reopen this editor",
        ));
    }
    Ok(tx)
}
fn finish(
    tx: Transaction<'_>,
    actor: &ManagementActor,
    action: &str,
    id: &str,
) -> Result<(), AccessError> {
    bump_epoch(&tx).map_err(access_internal)?;
    audit(&tx, &actor_name(actor), action, id, "ok", BTreeMap::new()).map_err(access_internal)?;
    tx.commit().map_err(access_db)
}
fn name(value: &str) -> Result<&str, AccessError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 128 || value.chars().any(char::is_control) {
        return Err(invalid("Enter a name of 1–128 characters"));
    }
    Ok(value)
}
pub(super) fn exists(tx: &Transaction<'_>, kind: &str, id: &str) -> Result<(), AccessError> {
    let sql = match kind {
        "principal" => "SELECT 1 FROM auth_principals WHERE id=?1",
        "group" => "SELECT 1 FROM auth_groups WHERE id=?1",
        "role" => "SELECT 1 FROM auth_roles WHERE id=?1",
        "key" => "SELECT 1 FROM auth_api_keys WHERE id=?1",
        _ => return Err(invalid("Invalid subject kind")),
    };
    if tx
        .query_row(sql, [id], |_| Ok(()))
        .optional()
        .map_err(access_db)?
        .is_none()
    {
        return Err(invalid(
            "A selected identity, group, role or key no longer exists",
        ));
    }
    Ok(())
}
fn sorted<T: Ord + Clone>(v: &[T]) -> Vec<T> {
    let mut v = v.to_vec();
    v.sort();
    v.dedup();
    v
}

impl AuthStore {
    pub(super) fn update_access_user(
        &mut self,
        actor: &ManagementActor,
        id: &str,
        body: UpdateAccessUser,
    ) -> Result<(), AccessError> {
        let old = self
            .list_users()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(missing)?;
        if !matches!(body.record.kind.as_str(), "user" | "service_account") {
            return Err(invalid("Invalid identity kind"));
        }
        grant_change(
            actor,
            old.enabled != body.enabled || old.kind != body.record.kind,
        )?;
        if id == "usr_bootstrap" && !body.enabled {
            return Err(invalid("The bootstrap identity must remain enabled"));
        }
        let label = name(&body.record.display_name)?;
        let tx = edit_tx(self, body.expected_revision)?;
        tx.execute(
            "UPDATE auth_principals SET display_name=?2,kind=?3,enabled=?4 WHERE id=?1",
            params![id, label, body.record.kind, body.enabled],
        )
        .map_err(access_db)?;
        if !body.enabled {
            tx.execute("UPDATE auth_dashboard_sessions SET revoked_at=?2,revoked_reason='principal_disabled' WHERE principal_id=?1 AND revoked_at IS NULL",params![id,Utc::now().timestamp()]).map_err(access_db)?;
        }
        finish(tx, actor, "principal.updated", id)
    }
    pub(super) fn update_access_group(
        &mut self,
        actor: &ManagementActor,
        id: &str,
        body: UpdateAccessGroup,
    ) -> Result<(), AccessError> {
        let old = self
            .list_groups()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(missing)?;
        if body.record.members.len() > 512 {
            return Err(invalid("Too many group members"));
        }
        grant_change(
            actor,
            old.enabled != body.enabled || sorted(&old.members) != sorted(&body.record.members),
        )?;
        let label = name(&body.record.name)?;
        let tx = edit_tx(self, body.expected_revision)?;
        for member in &body.record.members {
            exists(&tx, "principal", member)?;
        }
        let taken = tx
            .query_row(
                "SELECT 1 FROM auth_groups WHERE name=?1 AND id<>?2",
                params![label, id],
                |_| Ok(()),
            )
            .optional()
            .map_err(access_db)?
            .is_some();
        if taken {
            return Err(AccessError::new(
                StatusCode::CONFLICT,
                "Group name already exists",
            ));
        }
        tx.execute(
            "UPDATE auth_groups SET name=?2,enabled=?3 WHERE id=?1",
            params![id, label, body.enabled],
        )
        .map_err(access_db)?;
        tx.execute("DELETE FROM auth_group_members WHERE group_id=?1", [id])
            .map_err(access_db)?;
        for member in sorted(&body.record.members) {
            tx.execute(
                "INSERT INTO auth_group_members(group_id,principal_id) VALUES(?1,?2)",
                params![id, member],
            )
            .map_err(access_db)?;
        }
        finish(tx, actor, "group.updated", id)
    }
    pub(super) fn update_access_role(
        &mut self,
        actor: &ManagementActor,
        id: &str,
        body: UpdateAccessRole,
    ) -> Result<(), AccessError> {
        let old = self
            .list_roles()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(missing)?;
        // Compare owned tuples (the editor may reorder assignments).
        let old_bindings: HashSet<_> = old
            .bindings
            .iter()
            .map(|b| (b.subject_kind.clone(), b.subject_id.clone()))
            .collect();
        let new_bindings: HashSet<_> = body
            .bindings
            .iter()
            .map(|b| (b.subject_kind.clone(), b.subject_id.clone()))
            .collect();
        grant_change(
            actor,
            old.enabled != body.enabled || old_bindings != new_bindings,
        )?;
        if let ManagementActor::Delegated { permissions, .. } = actor
            && old
                .permissions
                .iter()
                .chain(&body.record.permissions)
                .any(|p| !permissions.contains(p))
        {
            return Err(AccessError::new(
                StatusCode::FORBIDDEN,
                "Cannot change role permissions outside your own grant",
            ));
        }
        if body.bindings.len() > 512 {
            return Err(invalid("Too many role assignments"));
        }
        let label = name(&body.record.name)?;
        let tx = edit_tx(self, body.expected_revision)?;
        for b in &body.bindings {
            if !matches!(b.subject_kind.as_str(), "principal" | "group") {
                return Err(invalid("Roles can be assigned to identities or groups"));
            }
            exists(&tx, &b.subject_kind, &b.subject_id)?;
        }
        if tx
            .query_row(
                "SELECT 1 FROM auth_roles WHERE name=?1 AND id<>?2",
                params![label, id],
                |_| Ok(()),
            )
            .optional()
            .map_err(access_db)?
            .is_some()
        {
            return Err(AccessError::new(
                StatusCode::CONFLICT,
                "Role name already exists",
            ));
        }
        tx.execute(
            "UPDATE auth_roles SET name=?2,enabled=?3 WHERE id=?1",
            params![id, label, body.enabled],
        )
        .map_err(access_db)?;
        tx.execute("DELETE FROM auth_role_permissions WHERE role_id=?1", [id])
            .map_err(access_db)?;
        for permission in &body.record.permissions {
            tx.execute(
                "INSERT OR IGNORE INTO auth_role_permissions(role_id,permission) VALUES(?1,?2)",
                params![id, wire_permission_name(*permission)],
            )
            .map_err(access_db)?;
        }
        tx.execute("DELETE FROM auth_role_bindings WHERE role_id=?1", [id])
            .map_err(access_db)?;
        for (kind, subject) in new_bindings {
            tx.execute(
                "INSERT INTO auth_role_bindings(role_id,subject_kind,subject_id) VALUES(?1,?2,?3)",
                params![id, kind, subject],
            )
            .map_err(access_db)?;
        }
        finish(tx, actor, "role.updated", id)
    }
    pub(super) fn update_access_key(
        &mut self,
        actor: &ManagementActor,
        id: &str,
        body: UpdateAccessKey,
    ) -> Result<(), AccessError> {
        let old = self
            .list_access_keys()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(missing)?;
        grant_change(
            actor,
            old.principal_id != body.record.principal_id
                || old.expires_at != body.record.expires_at,
        )?;
        let expiry = body
            .record
            .expires_at
            .as_deref()
            .map(|v| {
                DateTime::parse_from_rfc3339(v)
                    .map(|d| d.timestamp())
                    .map_err(|_| invalid("Choose a valid expiration date"))
            })
            .transpose()?;
        if body.record.expires_at != old.expires_at
            && expiry.is_some_and(|t| t <= Utc::now().timestamp())
        {
            return Err(invalid("New expiry must be in the future"));
        }
        let label = name(&body.record.name)?;
        let tx = edit_tx(self, body.expected_revision)?;
        exists(&tx, "principal", &body.record.principal_id)?;
        tx.execute(
            "UPDATE auth_api_keys SET name=?2,principal_id=?3,expires_at=?4 WHERE id=?1",
            params![id, label, body.record.principal_id, expiry],
        )
        .map_err(access_db)?;
        if old.principal_id != body.record.principal_id {
            tx.execute("UPDATE auth_dashboard_sessions SET revoked_at=?2,revoked_reason='key_owner_changed' WHERE key_id=?1 AND revoked_at IS NULL",params![id,Utc::now().timestamp()]).map_err(access_db)?;
        }
        finish(tx, actor, "key.updated", id)
    }
    pub(super) fn update_access_policy(
        &mut self,
        actor: &ManagementActor,
        id: &str,
        body: Box<UpdateAccessPolicy>,
    ) -> Result<(), AccessError> {
        name(&body.record.name)?;
        let old = self
            .list_policies()?
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(missing)?;
        let mut old_value =
            serde_json::to_value(&old).map_err(|e| access_internal(e.to_string()))?;
        for key in ["id", "revision", "name", "models"] {
            old_value.as_object_mut().unwrap().remove(key);
        }
        let mut value =
            serde_json::to_value(&body.record).map_err(|e| access_internal(e.to_string()))?;
        value.as_object_mut().unwrap().remove("name");
        value.as_object_mut().unwrap().remove("models");
        value["enabled"] = body.enabled.into();
        grant_change(actor, old_value != value)?;
        self.write_policy(
            actor,
            body.record,
            Some((id, body.enabled, body.expected_revision)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_requests_round_trip_all_fields() {
        let values = [
            serde_json::json!({"expected_revision":4,"enabled":true,"display_name":"New","kind":"service_account"}),
            serde_json::json!({"expected_revision":4,"enabled":false,"name":"group","members":["usr_a"]}),
            serde_json::json!({"expected_revision":4,"enabled":true,"name":"role","permissions":["auth.keys.write"],"bindings":[{"subject_kind":"group","subject_id":"grp_a"}]}),
            serde_json::json!({"expected_revision":4,"name":"key","principal_id":"usr_a","expires_at":null}),
        ];
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<UpdateAccessUser>(values[0].clone()).unwrap()
            )
            .unwrap(),
            values[0]
        );
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<UpdateAccessGroup>(values[1].clone()).unwrap()
            )
            .unwrap(),
            values[1]
        );
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<UpdateAccessRole>(values[2].clone()).unwrap()
            )
            .unwrap(),
            values[2]
        );
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<UpdateAccessKey>(values[3].clone()).unwrap()
            )
            .unwrap(),
            values[3]
        );
        let policy = serde_json::json!({"expected_revision":4,"enabled":false,"name":"policy","effect":"deny","subjects":["group:grp_a"],"endpoints":["chat"],"models":[],"requested_models":["wielick-*"],"served_models":["upstream-*"],"providers":["local-*"],"routes":["production"],"time_windows":[{"weekday_mask":5,"start_minute":1320,"end_minute":300,"absolute_start_ms":1792320000123_i64,"absolute_end_ms":null}],"max_concurrent_sessions":4,"max_daily_session_starts":20,"management_permissions":["auth.keys.write"]});
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<UpdateAccessPolicy>(policy.clone()).unwrap()
            )
            .unwrap(),
            policy
        );
    }
}

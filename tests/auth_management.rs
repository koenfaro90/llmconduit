use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use llmconduit::authz::{AuthFailure, AuthzService};
use llmconduit::config::{AuthConfig, AuthMode};
use llmconduit::dashboard_access::{ManagementActor, ManagementPermission, routes};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

static AUTH_ENV_LOCK: Mutex<()> = Mutex::new(());

fn enforced_service() -> (AuthzService, PathBuf) {
    let _guard = AUTH_ENV_LOCK.lock().expect("auth env lock");
    let path = std::env::temp_dir().join(format!(
        "llmconduit-management-lifecycle-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    let bootstrap = format!("llmc_{}", uuid::Uuid::new_v4().simple());
    let old_pepper = std::env::var_os("LLMCONDUIT_AUTH_PEPPER");
    let old_bootstrap = std::env::var_os("LLMCONDUIT_AUTH_BOOTSTRAP_KEY");
    // SAFETY: this integration-test binary serializes its environment mutation and restores
    // both variables before releasing the lock. Production code never mutates these values.
    unsafe {
        std::env::set_var("LLMCONDUIT_AUTH_PEPPER", "management-test-pepper-never-log");
        std::env::set_var("LLMCONDUIT_AUTH_BOOTSTRAP_KEY", &bootstrap);
    }
    let service = AuthzService::from_config(&AuthConfig {
        mode: AuthMode::Enforce,
        store_path: path.clone(),
    })
    .expect("enforced auth service");
    // SAFETY: paired with the serialized mutation above.
    unsafe {
        match old_pepper {
            Some(value) => std::env::set_var("LLMCONDUIT_AUTH_PEPPER", value),
            None => std::env::remove_var("LLMCONDUIT_AUTH_PEPPER"),
        }
        match old_bootstrap {
            Some(value) => std::env::set_var("LLMCONDUIT_AUTH_BOOTSTRAP_KEY", value),
            None => std::env::remove_var("LLMCONDUIT_AUTH_BOOTSTRAP_KEY"),
        }
    }
    (service, path)
}

fn delegated(permission: ManagementPermission) -> ManagementActor {
    ManagementActor::Delegated {
        session_id: "session_test".into(),
        principal_id: "principal_admin".into(),
        key_id: "key_admin".into(),
        permissions: Arc::from([permission]),
    }
}

async fn json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("bounded response body");
    serde_json::from_slice(&bytes).expect("JSON response")
}

fn remove_store(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

fn assert_store_files_do_not_contain(path: &Path, secret: &str) {
    for candidate in [
        path.to_path_buf(),
        path.with_extension("sqlite3-wal"),
        path.with_extension("sqlite3-shm"),
    ] {
        let Ok(bytes) = std::fs::read(&candidate) else {
            continue;
        };
        assert!(
            !bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()),
            "raw key leaked into {}",
            candidate.display()
        );
    }
}

#[tokio::test]
async fn management_key_list_and_revoke_are_permissioned_and_never_recover_raw_secret() {
    let (service, path) = enforced_service();
    let service = Arc::new(service);
    let users = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(ManagementActor::Bootstrap))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/dashboard/api/auth/users")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"display_name":"management lifecycle principal","kind":"service_account"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(users.status(), StatusCode::OK);
    let users = json(users).await;
    let principal_id = users["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|user| user["display_name"] == "management lifecycle principal")
        .expect("created principal is returned")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let create_body = serde_json::json!({
        "principal_id": principal_id,
        "name": "management lifecycle key",
        "expires_at": null
    })
    .to_string();

    let denied_create = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::KeysRead,
        )))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/dashboard/api/auth/api-keys")
                .header("content-type", "application/json")
                .body(Body::from(create_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied_create.status(), StatusCode::FORBIDDEN);

    let created = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::KeysCreate,
        )))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/dashboard/api/auth/api-keys")
                .header("content-type", "application/json")
                .body(Body::from(create_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json(created).await;
    let created_key_id = created["id"].as_str().unwrap().to_owned();
    let created_raw = created["raw_key"].as_str().unwrap().to_owned();
    assert!(created_raw.starts_with("llmc_"));
    assert!(created.get("api_key").is_none(), "created key is flattened");

    let list_response = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::KeysRead,
        )))
        .oneshot(
            Request::builder()
                .uri("/dashboard/api/auth/api-keys")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let listed = json(list_response).await;
    let listed_wire = listed.to_string();
    assert!(
        listed["api_keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key["id"] == created_key_id)
    );
    assert!(!listed_wire.contains(&created_raw));
    assert!(!listed_wire.contains("raw_key"));

    let mut headers = HeaderMap::new();
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(&created_raw).expect("generated key is a valid header"),
    );
    assert!(service.authenticate(&headers).unwrap().is_some());

    let rotated = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::KeysRotate,
        )))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/dashboard/api/auth/api-keys/{created_key_id}/rotate"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rotated.status(), StatusCode::OK);
    let rotated = json(rotated).await;
    let key_id = rotated["id"].as_str().unwrap().to_owned();
    let raw = rotated["raw_key"].as_str().unwrap().to_owned();
    assert_ne!(key_id, created_key_id);
    assert_ne!(raw, created_raw);
    assert!(matches!(
        service.authenticate(&headers),
        Err(AuthFailure::Invalid)
    ));
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(&raw).expect("rotated key is a valid header"),
    );
    assert!(service.authenticate(&headers).unwrap().is_some());

    let denied = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::KeysRead,
        )))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/dashboard/api/auth/api-keys/{key_id}/revoke"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let revoked = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::KeysRevoke,
        )))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/dashboard/api/auth/api-keys/{key_id}/revoke"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::OK);
    let revoked_body = json(revoked).await;
    let revoked_key = revoked_body["api_keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|key| key["id"] == key_id)
        .expect("revoked key remains listable");
    assert_eq!(revoked_key["enabled"], false);
    assert!(!revoked_body.to_string().contains(&raw));
    assert!(matches!(
        service.authenticate(&headers),
        Err(AuthFailure::Invalid)
    ));

    let denied_audit = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::KeysRead,
        )))
        .oneshot(
            Request::builder()
                .uri("/dashboard/api/auth/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied_audit.status(), StatusCode::FORBIDDEN);

    let audit = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::AuditRead,
        )))
        .oneshot(
            Request::builder()
                .uri("/dashboard/api/auth/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audit.status(), StatusCode::OK);
    let audit = json(audit).await;
    let audit_wire = audit.to_string();
    assert!(!audit_wire.contains(&created_raw));
    assert!(!audit_wire.contains(&raw));

    drop(service);
    assert_store_files_do_not_contain(&path, &created_raw);
    assert_store_files_do_not_contain(&path, &raw);
    remove_store(&path);
}

#[tokio::test]
async fn delegated_role_writer_cannot_grant_permissions_it_does_not_hold() {
    let (service, path) = enforced_service();
    let service = Arc::new(service);

    let denied = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::RolesWrite,
        )))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/dashboard/api/auth/roles")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"escalated role","permissions":["auth.keys.revoke"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert!(
        json(denied).await["error"]
            .as_str()
            .unwrap()
            .contains("does not possess")
    );

    let allowed = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(delegated(
            ManagementPermission::RolesWrite,
        )))
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/dashboard/api/auth/roles")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"bounded role","permissions":["auth.roles.write"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    let allowed = json(allowed).await;
    let roles = allowed["roles"].as_array().unwrap();
    assert!(roles.iter().any(|role| role["name"] == "bounded role"));
    assert!(!roles.iter().any(|role| role["name"] == "escalated role"));

    let audit = routes::<()>(service.access_backend())
        .layer(axum::extract::Extension(ManagementActor::Bootstrap))
        .oneshot(
            Request::builder()
                .uri("/dashboard/api/auth/audit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audit.status(), StatusCode::OK);
    let events = json(audit).await;
    assert!(events["events"].as_array().unwrap().iter().any(|event| {
        event["action"] == "role.create"
            && event["outcome"] == "denied"
            && event["metadata"]["reason"] == "requested permissions exceed actor"
    }));

    drop(service);
    remove_store(&path);
}

async fn edit_call(
    service: &AuthzService,
    actor: ManagementActor,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(
            body.map(|v| Body::from(v.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let response = routes::<()>(Arc::new(service.clone()).access_backend())
        .layer(axum::extract::Extension(actor))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    (status, json(response).await)
}
async fn edit_row(service: &AuthzService, collection: &str, id: &str) -> Value {
    let (status, value) = edit_call(
        service,
        ManagementActor::Bootstrap,
        "GET",
        &format!("/dashboard/api/auth/{collection}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    value[if collection == "api-keys" {
        "api_keys"
    } else {
        collection
    }]
    .as_array()
    .unwrap()
    .iter()
    .find(|row| row["id"] == id)
    .unwrap()
    .clone()
}
fn update_body(row: &Value) -> Value {
    let mut row = row.clone();
    row["expected_revision"] = row["revision"].clone();
    for field in [
        "id",
        "revision",
        "created_at",
        "last_used_at",
        "prefix",
        "member_count",
    ] {
        row.as_object_mut().unwrap().remove(field);
    }
    row
}

#[tokio::test]
async fn access_edits_preserve_identity_apply_membership_and_reject_stale_grants() {
    let (service, path) = enforced_service();
    let (_, users) = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "POST",
        "/dashboard/api/auth/users",
        Some(serde_json::json!({"display_name":"editable identity","kind":"user"})),
    )
    .await;
    let uid = users["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["display_name"] == "editable identity")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let row = edit_row(&service, "users", &uid).await;
    let mut body = update_body(&row);
    body["display_name"] = "Edited identity".into();
    body["kind"] = "service_account".into();
    let (status, _) = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "PUT",
        &format!("/dashboard/api/auth/users/{uid}"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        edit_row(&service, "users", &uid).await["created_at"],
        row["created_at"]
    );
    assert_eq!(
        edit_call(
            &service,
            ManagementActor::Bootstrap,
            "PUT",
            &format!("/dashboard/api/auth/users/{uid}"),
            Some(body)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, groups) = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "POST",
        "/dashboard/api/auth/groups",
        Some(serde_json::json!({"name":"editable group","members":[uid]})),
    )
    .await;
    let gid = groups["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == "editable group")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_,roles)=edit_call(&service,ManagementActor::Bootstrap,"POST","/dashboard/api/auth/roles",Some(serde_json::json!({"name":"editable role","permissions":["auth.groups.write","auth.keys.write"]}))).await;
    let rid = roles["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "editable role")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut body = update_body(&edit_row(&service, "roles", &rid).await);
    body["name"] = "Edited role".into();
    body["bindings"] = serde_json::json!([{"subject_kind":"group","subject_id":gid}]);
    assert_eq!(
        edit_call(
            &service,
            ManagementActor::Bootstrap,
            "PUT",
            &format!("/dashboard/api/auth/roles/{rid}"),
            Some(body)
        )
        .await
        .0,
        StatusCode::OK
    );
    let policy = serde_json::json!({"name":"editable policy","effect":"allow","subjects":[format!("role:{rid}")],"endpoints":["chat"],"models":[],"requested_models":["model-a"],"served_models":[],"providers":[],"routes":[],"time_windows":[],"max_concurrent_sessions":2,"max_daily_session_starts":null,"management_permissions":[]});
    let (status, policies) = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "POST",
        "/dashboard/api/auth/policies",
        Some(policy),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pid = policies["policies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "editable policy")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, created) = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "POST",
        "/dashboard/api/auth/api-keys",
        Some(serde_json::json!({"principal_id":uid,"name":"editable key","expires_at":null})),
    )
    .await;
    let kid = created["id"].as_str().unwrap().to_owned();
    let raw = created["raw_key"].as_str().unwrap().to_owned();
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_str(&raw).unwrap());
    let context = service.authenticate(&headers).unwrap().unwrap();
    assert!(context.allows_model("chat", "model-a"));
    let actor = context.management_actor();
    let lease = service.acquire_session(&context).await.unwrap().unwrap();
    // A delegated group editor may rename but cannot give a group new members.
    let row = edit_row(&service, "groups", &gid).await;
    let mut body = update_body(&row);
    body["members"] = serde_json::json!(["usr_bootstrap"]);
    assert_eq!(
        edit_call(
            &service,
            actor.clone(),
            "PUT",
            &format!("/dashboard/api/auth/groups/{gid}"),
            Some(body)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let mut body = update_body(&row);
    body["name"] = "Edited group".into();
    assert_eq!(
        edit_call(
            &service,
            actor.clone(),
            "PUT",
            &format!("/dashboard/api/auth/groups/{gid}"),
            Some(body)
        )
        .await
        .0,
        StatusCode::OK
    );
    // Metadata edits do not rotate or expose the raw key.
    let row = edit_row(&service, "api-keys", &kid).await;
    let mut body = update_body(&row);
    body.as_object_mut().unwrap().remove("enabled");
    body["name"] = "Edited key".into();
    let (status, result) = edit_call(
        &service,
        actor.clone(),
        "PUT",
        &format!("/dashboard/api/auth/api-keys/{kid}"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!result.to_string().contains(&raw));
    assert_eq!(
        edit_row(&service, "api-keys", &kid).await["prefix"],
        row["prefix"]
    );
    assert!(service.authenticate(&headers).unwrap().is_some());
    let row = edit_row(&service, "policies", &pid).await;
    let mut body = update_body(&row);
    body["models"] = serde_json::json!([]);
    body["requested_models"] = serde_json::json!(["model-b"]);
    body["name"] = "Edited policy".into();
    body["time_windows"] = serde_json::json!([
        {"weekday_mask":0,"start_minute":0,"end_minute":0,"absolute_start_ms":null,"absolute_end_ms":null},
        {"weekday_mask":31,"start_minute":480,"end_minute":1080,"absolute_start_ms":null,"absolute_end_ms":null}
    ]);
    assert_eq!(
        edit_call(
            &service,
            ManagementActor::Bootstrap,
            "PUT",
            &format!("/dashboard/api/auth/policies/{pid}"),
            Some(body)
        )
        .await
        .0,
        StatusCode::OK
    );
    let context = service.authenticate(&headers).unwrap().unwrap();
    assert!(!context.allows_model("chat", "model-a"));
    assert!(context.allows_model("chat", "model-b"));
    // Bad rules roll back without changing the previous policy or live authority.
    let mut bad = update_body(&edit_row(&service, "policies", &pid).await);
    bad["endpoints"] = serde_json::json!(["unknown-endpoint"]);
    assert_eq!(
        edit_call(
            &service,
            ManagementActor::Bootstrap,
            "PUT",
            &format!("/dashboard/api/auth/policies/{pid}"),
            Some(bad)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(
        service
            .authenticate(&headers)
            .unwrap()
            .unwrap()
            .allows_model("chat", "model-b")
    );
    for (collection, id) in [("groups", &gid), ("roles", &rid), ("policies", &pid)] {
        let row = edit_row(&service, collection, id).await;
        let mut disabled = update_body(&row);
        disabled["enabled"] = false.into();
        assert_eq!(
            edit_call(
                &service,
                ManagementActor::Bootstrap,
                "PUT",
                &format!("/dashboard/api/auth/{collection}/{id}"),
                Some(disabled)
            )
            .await
            .0,
            StatusCode::OK
        );
        assert!(
            !service
                .authenticate(&headers)
                .unwrap()
                .unwrap()
                .allows_model("chat", "model-b")
        );
        let mut enabled = update_body(&edit_row(&service, collection, id).await);
        enabled["enabled"] = true.into();
        assert_eq!(
            edit_call(
                &service,
                ManagementActor::Bootstrap,
                "PUT",
                &format!("/dashboard/api/auth/{collection}/{id}"),
                Some(enabled)
            )
            .await
            .0,
            StatusCode::OK
        );
        assert!(
            service
                .authenticate(&headers)
                .unwrap()
                .unwrap()
                .allows_model("chat", "model-b")
        );
    }
    let preserved = edit_row(&service, "policies", &pid).await;
    assert_eq!(preserved["served_models"], serde_json::json!([]));
    assert_eq!(preserved["time_windows"].as_array().unwrap().len(), 2);
    // Membership removal takes effect live and an old management actor cannot save afterward.
    let mut group = update_body(&edit_row(&service, "groups", &gid).await);
    group["members"] = serde_json::json!([]);
    assert_eq!(
        edit_call(
            &service,
            ManagementActor::Bootstrap,
            "PUT",
            &format!("/dashboard/api/auth/groups/{gid}"),
            Some(group)
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut renamed = update_body(&edit_row(&service, "groups", &gid).await);
    renamed["name"] = "stale delegated actor".into();
    assert_eq!(
        edit_call(
            &service,
            actor,
            "PUT",
            &format!("/dashboard/api/auth/groups/{gid}"),
            Some(renamed)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let sessions = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "GET",
        "/dashboard/api/auth/sessions",
        None,
    )
    .await
    .1;
    assert!(
        sessions["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == lease.session_id())
    );
    drop(lease);
    let mut principal = update_body(&edit_row(&service, "users", &uid).await);
    principal["enabled"] = false.into();
    assert_eq!(
        edit_call(
            &service,
            ManagementActor::Bootstrap,
            "PUT",
            &format!("/dashboard/api/auth/users/{uid}"),
            Some(principal)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(service.authenticate(&headers).is_err());
    let audit = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "GET",
        "/dashboard/api/auth/audit",
        None,
    )
    .await
    .1;
    assert!(
        audit["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "policy.updated")
    );
    assert_store_files_do_not_contain(&path, &raw);
    remove_store(&path);
}

#[tokio::test]
async fn access_key_owner_and_expiry_updates_preserve_token_and_expire_without_reload() {
    let (service, path) = enforced_service();
    let (_, users) = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "POST",
        "/dashboard/api/auth/users",
        Some(serde_json::json!({"display_name":"first owner","kind":"user"})),
    )
    .await;
    let uid = users["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["display_name"] == "first owner")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, users) = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "POST",
        "/dashboard/api/auth/users",
        Some(serde_json::json!({"display_name":"next owner","kind":"service_account"})),
    )
    .await;
    let second = users["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["display_name"] == "next owner")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, created) = edit_call(
        &service,
        ManagementActor::Bootstrap,
        "POST",
        "/dashboard/api/auth/api-keys",
        Some(serde_json::json!({"principal_id":uid,"name":"keep same token","expires_at":null})),
    )
    .await;
    let id = created["id"].as_str().unwrap().to_owned();
    let raw = created["raw_key"].as_str().unwrap().to_owned();
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_str(&raw).unwrap());
    let before = edit_row(&service, "api-keys", &id).await;
    let mut body = update_body(&before);
    body.as_object_mut().unwrap().remove("enabled");
    body["principal_id"] = second.clone().into();
    body["expires_at"] = (chrono::Utc::now() + chrono::Duration::seconds(2))
        .to_rfc3339()
        .into();
    assert_eq!(
        edit_call(
            &service,
            ManagementActor::Bootstrap,
            "PUT",
            &format!("/dashboard/api/auth/api-keys/{id}"),
            Some(body)
        )
        .await
        .0,
        StatusCode::OK
    );
    let current = service.authenticate(&headers).unwrap().unwrap();
    assert_eq!(current.principal_id, second);
    let after = edit_row(&service, "api-keys", &id).await;
    assert_eq!(after["prefix"], before["prefix"]);
    assert_eq!(after["created_at"], before["created_at"]);
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    assert!(
        service.authenticate(&headers).is_err(),
        "expiry must take effect without another reload"
    );
    remove_store(&path);
}

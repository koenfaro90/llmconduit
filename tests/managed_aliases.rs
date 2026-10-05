//! SQL persistence, immutable per-request routes, real HTTP dispatch and alias retirement.
use axum::{body::Body, http::Request};
use futures::StreamExt;
use llmconduit::{
    AppOptions, ControlPlaneRuntime,
    config::{Config, PersistedConfig},
    control_plane::{OperationalProviderPlan, OperationalRoutePlan, UnknownModelPolicy},
    control_plane_store::{PersistenceStore, SqlStore},
    managed_aliases::{AliasMapping, STORE_KEY, SaveAliases},
    models::chat::ChatCompletionRequest,
    upstream::BackendChatRequest,
};
use serde_json::{Map, json};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};
fn profile(id: Uuid, name: &str, server: &MockServer) -> OperationalRoutePlan {
    OperationalRoutePlan {
        route_id: id,
        name: name.into(),
        primary_profile_id: id,
        primary_profile_name: name.into(),
        profile_ids: vec![id],
        providers: vec![OperationalProviderPlan {
            backend_id: Uuid::new_v4(),
            backend_name: name.into(),
            profile_id: id,
            profile_name: name.into(),
            base_url: format!("{}/v1/", server.uri()).parse().unwrap(),
            api_key: Some("private-upstream-secret".into()),
            request_log_path: None,
            upstream_model: Some(format!("served-{name}")),
            upstream_chat_kwargs: Map::new(),
        }],
    }
}
fn build(
    store: Arc<SqlStore>,
    routes: Vec<OperationalRoutePlan>,
) -> (axum::Router, Arc<llmconduit::engine::Gateway>) {
    let config = Config::from_persisted(&PersistedConfig {
        upstream_base_url: "http://127.0.0.1:9/v1/".into(),
        ..Default::default()
    })
    .unwrap();
    llmconduit::build_app_with_gateway_control_plane_runtime(
        config,
        None,
        AppOptions::default(),
        routes,
        UnknownModelPolicy::Passthrough,
        None,
        ControlPlaneRuntime {
            persistence_store: Some(store),
            ..Default::default()
        },
    )
    .unwrap()
}
async fn serve(server: &MockServer, name: &str, status: u16) {
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[{"id":format!("served-{name}"),"context_window":16384}]}),
        ))
        .mount(server)
        .await;
    let response = if status == 200 {
        ResponseTemplate::new(200).insert_header("content-type","text/event-stream").set_body_string(format!("data: {{\"id\":\"test\",\"model\":\"served-{name}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{name}\"}},\"finish_reason\":null}}]}}\n\ndata: [DONE]\n\n"))
    } else {
        ResponseTemplate::new(status)
    };
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({"model":format!("served-{name}")})))
        .respond_with(response)
        .mount(server)
        .await;
}
async fn generate(g: &Arc<llmconduit::engine::Gateway>, model: &str) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !g.traffic().snapshot().ready {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let r: ChatCompletionRequest = serde_json::from_value(
        json!({"model":model,"messages":[{"role":"user","content":"test"}],"stream":true}),
    )
    .unwrap();
    let mut stream = g
        .upstream_client()
        .stream_chat_completion(&BackendChatRequest::new(r, None, None, None))
        .await
        .unwrap();
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        for c in chunk.unwrap().choices {
            text.push_str(c.delta.content.as_deref().unwrap_or_default());
        }
    }
    text
}
#[tokio::test]
async fn aliases_persist_across_config_sync_and_pin_routes_and_replay() {
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    serve(&a, "a", 200).await;
    serve(&b, "b", 200).await;
    let aid = Uuid::new_v4();
    let bid = Uuid::new_v4();
    let id = Uuid::new_v4();
    let mut routes = vec![profile(aid, "a", &a), profile(bid, "b", &b)];
    let mut alias = routes[0].clone();
    alias.route_id = id;
    alias.name = "public-model".into();
    routes.push(alias);
    let store = Arc::new(SqlStore::connect_sqlite("sqlite::memory:").await.unwrap());
    let (app, g) = build(store.clone(), routes.clone());
    let registry = g.alias_registry().unwrap();
    let original_replay = registry.snapshot().await.unwrap().replay.clone();
    original_replay
        .insert(llmconduit::replay::ReplayRecord {
            model: "public-model".into(),
            instructions: "marker".into(),
            visible_history: vec![],
            internal_messages: vec![],
        })
        .await;
    let original = g.request_snapshot().await.unwrap();
    assert_eq!(generate(&original, "public-model").await, "a");
    let mapping = AliasMapping {
        id,
        name: "public-model".into(),
        profiles: vec![bid],
    };
    let encoded = serde_json::to_value(&mapping).unwrap();
    assert_eq!(
        serde_json::from_value::<AliasMapping>(encoded).unwrap(),
        mapping
    );
    registry
        .save(SaveAliases {
            revision: 0,
            aliases: vec![mapping.clone()],
        })
        .await
        .unwrap();
    assert!(
        registry
            .snapshot()
            .await
            .unwrap()
            .replay
            .longest_prefix_match("public-model", "marker", &[])
            .await
            .is_none()
    );
    assert!(
        original_replay
            .longest_prefix_match("public-model", "marker", &[])
            .await
            .is_some()
    );
    let updated = g.request_snapshot().await.unwrap();
    assert_eq!(generate(&updated, "public-model").await, "b");
    let r: ChatCompletionRequest =
        serde_json::from_value(json!({"model":"public-model","messages":[]})).unwrap();
    let mut backend = BackendChatRequest::new(r, None, None, None);
    backend.authorization =
        llmconduit::upstream::AuthorizationScope::restricted(|_, _, model, _| model == "served-a");
    let error = updated
        .upstream_client()
        .stream_chat_completion(&backend)
        .await
        .err()
        .unwrap();
    assert_eq!(error.status_code(), 403);

    assert_eq!(generate(&original, "public-model").await, "a");
    // A restart/config sync must not re-import the original YAML target.
    let (_, restarted) = build(store.clone(), routes.clone());
    assert_eq!(
        generate(&restarted.request_snapshot().await.unwrap(), "public-model").await,
        "b"
    );
    assert!(
        registry
            .save(SaveAliases {
                revision: 0,
                aliases: vec![]
            })
            .await
            .is_err()
    );
    let bad = AliasMapping {
        name: "a".into(),
        ..mapping.clone()
    };
    assert!(
        registry
            .save(SaveAliases {
                revision: 1,
                aliases: vec![bad]
            })
            .await
            .is_err()
    );
    let duplicate = AliasMapping {
        profiles: vec![bid, bid],
        ..mapping.clone()
    };
    assert!(
        registry
            .save(SaveAliases {
                revision: 1,
                aliases: vec![duplicate]
            })
            .await
            .is_err()
    );
    let current = registry.view().await.unwrap();
    assert_eq!(current.revision, 1);
    assert!(
        !serde_json::to_string(&current)
            .unwrap()
            .contains("private-upstream-secret")
    );
    let renamed = AliasMapping {
        name: "renamed-model".into(),
        ..mapping
    };
    registry
        .save(SaveAliases {
            revision: 1,
            aliases: vec![renamed],
        })
        .await
        .unwrap();
    let r = app
        .clone()
        .oneshot(
            Request::post("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"model":"public-model","messages":[{"role":"user","content":"test"}]})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let catalog = g.upstream_client().supported_model_catalog().await.unwrap();
    assert!(!catalog.iter().any(|m| m.id == "public-model"));
    assert!(catalog.iter().any(|m| m.id == "renamed-model"));
    registry
        .save(SaveAliases {
            revision: 2,
            aliases: vec![],
        })
        .await
        .unwrap();
    let (_, restarted) = build(store.clone(), routes);
    let frozen = restarted.request_snapshot().await.unwrap();
    assert!(frozen.validate_operational_model("renamed-model").is_err());
    assert!(frozen.validate_operational_model("public-model").is_err());
    let persisted = store.get_setting(STORE_KEY).await.unwrap().unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(&persisted).unwrap()["aliases"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn ordered_fallbacks_and_missing_targets_are_preserved_and_fail_closed() {
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    serve(&a, "a", 503).await;
    serve(&b, "b", 200).await;
    let aid = Uuid::new_v4();
    let bid = Uuid::new_v4();
    let id = Uuid::new_v4();
    let routes = vec![profile(aid, "a", &a), profile(bid, "b", &b)];
    let store = Arc::new(SqlStore::connect_sqlite("sqlite::memory:").await.unwrap());
    let (_, g) = build(store.clone(), routes.clone());
    g.alias_registry()
        .unwrap()
        .save(SaveAliases {
            revision: 0,
            aliases: vec![AliasMapping {
                id,
                name: "chain".into(),
                profiles: vec![aid, bid],
            }],
        })
        .await
        .unwrap();
    let frozen = g.request_snapshot().await.unwrap();
    assert_eq!(generate(&frozen, "chain").await, "b");
    let (_, missing) = build(store, vec![routes[0].clone()]);
    let view = missing.alias_registry().unwrap().view().await.unwrap();
    assert_eq!(view.aliases[0].mapping.profiles, vec![aid, bid]);
    assert!(view.aliases[0].error.is_some());
    let frozen = missing.request_snapshot().await.unwrap();
    let r: ChatCompletionRequest =
        serde_json::from_value(json!({"model":"chain","messages":[]})).unwrap();
    let error = frozen
        .upstream_client()
        .stream_chat_completion(&BackendChatRequest::new(r, None, None, None))
        .await
        .err()
        .unwrap();
    assert_eq!(error.status_code(), 503);
}
#[tokio::test]
async fn corrupt_sql_aliases_do_not_fall_back_to_bootstrap_or_default_provider() {
    let server = MockServer::start().await;
    let store = Arc::new(SqlStore::connect_sqlite("sqlite::memory:").await.unwrap());
    store
        .compare_exchange_setting(STORE_KEY, None, "invalid json")
        .await
        .unwrap();
    let (_, g) = build(store, vec![profile(Uuid::new_v4(), "a", &server)]);
    assert!(g.request_snapshot().await.is_err());
    assert!(g.upstream_client().supported_model_catalog().await.is_err());
}

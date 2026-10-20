//! Explicit media capabilities survive provider discovery, profile overrides and live aliases.
use axum::{body::Body, http::Request};
use llmconduit::{
    AppOptions, ControlPlaneRuntime,
    config::{Config, PersistedConfig, PersistedModelProfile},
    control_plane::{OperationalProviderPlan, OperationalRoutePlan, UnknownModelPolicy},
    managed_aliases::{AliasMapping, SaveAliases},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn config(server: &MockServer) -> Config {
    Config::from_persisted(&PersistedConfig {
        upstream_base_url: format!("{}/v1/", server.uri()),
        ..Default::default()
    })
    .unwrap()
}

async fn models(app: axum::Router) -> (Value, Option<String>) {
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let etag = response
        .headers()
        .get("etag")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (serde_json::from_slice(&bytes).unwrap(), etag)
}

async fn provider(entries: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"provider-v1\"")
                .set_body_json(json!({"object":"list","data":entries})),
        )
        .mount(&server)
        .await;
    server
}

fn route(id: Uuid, name: &str, server: &MockServer) -> OperationalRoutePlan {
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
            api_key: None,
            request_log_path: None,
            upstream_model: Some(format!("served-{name}")),
            upstream_chat_kwargs: Default::default(),
        }],
    }
}

#[test]
fn video_capability_round_trips_as_a_profile_field_not_a_request_kwarg() {
    for name in [
        "image_generation",
        "image_editing",
        "audio_transcription",
        "speech_generation",
        "video_generation",
    ] {
        for supported in [true, false] {
            let input = json!({"capabilities":{name:{"supported":supported}}});
            let profile: PersistedModelProfile = serde_json::from_value(input).unwrap();
            let output = serde_json::to_value(&profile).unwrap();
            assert_eq!(output["capabilities"][name]["supported"], supported);
            assert!(!profile.upstream_chat_kwargs.contains_key("capabilities"));
            assert_eq!(
                serde_json::from_value::<PersistedModelProfile>(output).unwrap(),
                profile
            );
        }
    }
}

#[tokio::test]
async fn configured_media_metadata_changes_etag_without_inventing_capabilities() {
    let server = provider(json!([
        {"id":"configured","capabilities":{"image_input":{"supported":true},"vendor_feature":{"supported":true}}},
        {"id":"video-sounding-name"}
    ])).await;
    let original = models(llmconduit::build_app(config(&server))).await;
    assert_eq!(original.1.as_deref(), Some("\"provider-v1\""));
    let mut persisted = PersistedConfig {
        upstream_base_url: format!("{}/v1/", server.uri()),
        ..Default::default()
    };
    persisted.model_profiles.insert(
        "configured".into(),
        serde_json::from_value(json!({
            "capabilities":{"video_generation":{"supported":true},"image_input":{"supported":false}}
        }))
        .unwrap(),
    );
    let changed = models(llmconduit::build_app(
        Config::from_persisted(&persisted).unwrap(),
    ))
    .await;
    let capabilities = &changed.0["data"][0]["capabilities"];
    assert_eq!(capabilities["video_generation"]["supported"], true);
    assert_eq!(capabilities["image_input"]["supported"], false);
    assert_eq!(capabilities["vendor_feature"]["supported"], true);
    assert!(changed.0["data"][1].get("capabilities").is_none());
    assert!(changed.1.is_some());
    assert_ne!(original.1, changed.1);
    assert_eq!(
        changed.1,
        models(llmconduit::build_app(
            Config::from_persisted(&persisted).unwrap()
        ))
        .await
        .1
    );
}

#[tokio::test]
async fn managed_catalog_preserves_explicit_upstream_capabilities() {
    let server = provider(json!([{"id":"provider-media","capabilities":{
        "video_generation":{"supported":true},"image_generation":{"supported":false},
        "speech_generation":{"supported":true}
    }}]))
    .await;
    let (app, _) = llmconduit::build_app_with_gateway_control_plane_runtime(
        config(&server),
        None,
        AppOptions::default(),
        vec![],
        UnknownModelPolicy::Passthrough,
        None,
        ControlPlaneRuntime {
            persistence_store: Some(std::sync::Arc::new(
                llmconduit::control_plane_store::SqlStore::connect_sqlite("sqlite::memory:")
                    .await
                    .unwrap(),
            )),
            ..Default::default()
        },
    )
    .unwrap();
    let (body, _) = models(app).await;
    assert_eq!(
        body["data"][0]["capabilities"],
        json!({
            "video_generation":{"supported":true},"image_generation":{"supported":false},
            "speech_generation":{"supported":true}
        })
    );
}

#[tokio::test]
async fn media_catalogue_keeps_client_auth_and_model_filtering() {
    let server = provider(json!([
        {"id":"allowed","capabilities":{"video_generation":{"supported":true}}},
        {"id":"private","capabilities":{"video_generation":{"supported":true}}}
    ]))
    .await;
    let auth = llmconduit::client_auth::ClientAuth::from_specs(
        true,
        vec![llmconduit::client_auth::VirtualKeySpec {
            id: "fixture".into(),
            label: None,
            owner_id: None,
            secret_hash: llmconduit::client_auth::hash_secret("fixture-secret"),
            allowed_models: vec!["allowed".into()],
        }],
    )
    .unwrap();
    let (app, _) = llmconduit::build_app_with_gateway_control_plane(
        config(&server),
        None,
        AppOptions::default(),
        vec![],
        UnknownModelPolicy::Passthrough,
        Some(auth),
    )
    .unwrap();
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .header("authorization", "Bearer fixture-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("etag").is_none());
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["id"], "allowed");
    assert_eq!(
        body["data"][0]["capabilities"]["video_generation"]["supported"],
        true
    );
}

#[tokio::test]
async fn aliases_require_all_enabled_targets_and_follow_live_mapping_changes() {
    let server = provider(json!([])).await;
    let supported = Uuid::new_v4();
    let unknown = Uuid::new_v4();
    let disabled = Uuid::new_v4();
    let alias_id = Uuid::new_v4();
    let mut supported_route = route(supported, "capable", &server);
    let unknown_route = route(unknown, "unknown", &server);
    let mut alias = supported_route.clone();
    alias.route_id = alias_id;
    alias.name = "public-media".into();
    // The first profile has no enabled provider; the actual chain contains
    // only the explicitly capable second profile.
    alias.primary_profile_id = disabled;
    alias.primary_profile_name = "disabled".into();
    alias.profile_ids = vec![disabled, supported];
    let mut persisted = PersistedConfig {
        upstream_base_url: format!("{}/v1/", server.uri()),
        ..Default::default()
    };
    persisted.model_profiles.insert(
        "capable".into(),
        serde_json::from_value(json!({
            "capabilities":{"video_generation":{"supported":true},"image_input":{"supported":true}}
        }))
        .unwrap(),
    );
    // A global fallback must not certify an unknown member of a managed
    // alias chain: the enabled target profiles are its explicit contract.
    persisted.model_profiles.insert(
        "*".into(),
        serde_json::from_value(json!({
            "capabilities":{"video_generation":{"supported":true}}
        }))
        .unwrap(),
    );
    supported_route.providers[0].upstream_model = Some("opaque-provider-id".into());
    let (app, gateway) = llmconduit::build_app_with_gateway_control_plane_runtime(
        Config::from_persisted(&persisted).unwrap(),
        None,
        AppOptions::default(),
        vec![supported_route, unknown_route, alias],
        UnknownModelPolicy::Passthrough,
        None,
        ControlPlaneRuntime {
            persistence_store: Some(std::sync::Arc::new(
                llmconduit::control_plane_store::SqlStore::connect_sqlite("sqlite::memory:")
                    .await
                    .unwrap(),
            )),
            ..Default::default()
        },
    )
    .unwrap();
    let (before, before_etag) = models(app.clone()).await;
    let entry = |body: &Value| {
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "public-media")
            .unwrap()
            .clone()
    };
    assert_eq!(
        entry(&before)["capabilities"]["video_generation"]["supported"],
        true
    );
    assert_eq!(
        entry(&before)["capabilities"]["image_input"]["supported"],
        true
    );
    let registry = gateway.alias_registry().unwrap();
    registry
        .save(SaveAliases {
            revision: 0,
            aliases: vec![AliasMapping {
                id: alias_id,
                name: "public-media".into(),
                profiles: vec![supported, unknown],
            }],
        })
        .await
        .unwrap();
    let (after, after_etag) = models(app).await;
    assert_ne!(
        entry(&after)["capabilities"]["video_generation"]["supported"],
        true
    );
    assert_ne!(before_etag, after_etag);
}

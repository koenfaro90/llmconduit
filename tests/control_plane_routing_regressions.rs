//! Regression coverage for the public control-plane dependency-injection seam.
//!
//! These checks intentionally exercise the exported builder rather than only
//! validating the underlying config types: embedders can call this seam without
//! going through the binary's startup validation.

use axum::body::Body;
use http::Request;
use llmconduit::AppOptions;
use llmconduit::ControlPlaneRuntime;
use llmconduit::config::{Config, ModelRoute, PersistedConfig};
use llmconduit::control_plane::{
    OperationalProviderPlan, OperationalRoutePlan, UnknownModelPolicy,
};
use serde_json::{Map as JsonMap, json};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base_config(upstream_base_url: &str) -> Config {
    Config::from_persisted(&PersistedConfig {
        upstream_base_url: upstream_base_url.to_string(),
        ..PersistedConfig::default()
    })
    .expect("test config")
}

fn operational_route(name: &str, base_url: &str) -> OperationalRoutePlan {
    let profile_id = Uuid::new_v4();
    OperationalRoutePlan {
        route_id: Uuid::new_v4(),
        name: name.to_string(),
        primary_profile_id: profile_id,
        primary_profile_name: format!("{name}-profile"),
        profile_ids: vec![profile_id],
        providers: vec![OperationalProviderPlan {
            backend_id: Uuid::new_v4(),
            backend_name: format!("{name}-backend"),
            profile_id,
            profile_name: format!("{name}-profile"),
            base_url: Url::parse(base_url).expect("provider URL"),
            api_key: None,
            request_log_path: None,
            upstream_model: None,
            upstream_chat_kwargs: JsonMap::new(),
        }],
    }
}

fn builder_error_for_header(header: &str) -> String {
    let result = llmconduit::build_app_with_gateway_control_plane_runtime(
        base_config("http://127.0.0.1:9/v1/"),
        None,
        AppOptions::default(),
        Vec::new(),
        UnknownModelPolicy::Passthrough,
        None,
        ControlPlaneRuntime {
            conversation_id_header: header.to_string(),
            ..ControlPlaneRuntime::default()
        },
    );
    match result {
        Ok(_) => panic!("header {header:?} unexpectedly passed builder validation"),
        Err(error) => error,
    }
}

#[test]
fn public_builder_rejects_sensitive_conversation_headers() {
    for header in ["authorization", "Authorization", "x-api-key", "api-key"] {
        let error = builder_error_for_header(header);
        assert!(
            error.contains("sensitive credential carrier"),
            "unexpected error for {header:?}: {error}"
        );
    }
}

#[test]
fn public_builder_rejects_invalid_conversation_header_syntax() {
    let error = builder_error_for_header("x-conversation id");
    assert!(
        error.contains("invalid conversation id header name"),
        "unexpected error: {error}"
    );
}

#[test]
fn public_builder_rejects_exact_config_and_operational_route_collision() {
    let mut config = base_config("http://127.0.0.1:9/v1/");
    config.model_routes.push(ModelRoute {
        name: "shared-model".to_string(),
        glob: None,
        upstream_base_url: Url::parse("http://127.0.0.1:10/v1/").expect("route URL"),
        upstream_model: None,
    });

    let result = llmconduit::build_app_with_gateway_control_plane(
        config,
        None,
        AppOptions::default(),
        vec![operational_route("SHARED-MODEL", "http://127.0.0.1:11/v1/")],
        UnknownModelPolicy::Passthrough,
        None,
    );
    let error = match result {
        Ok(_) => panic!("case-insensitive exact route collision was accepted"),
        Err(error) => error,
    };
    assert!(
        error.contains(
            "operational route 'SHARED-MODEL' conflicts with configured model route 'shared-model'"
        ),
        "unexpected error: {error}"
    );
}

fn chat_sse_body(id: &str, content: &str) -> String {
    let chunk = json!({
        "id": id,
        "choices": [{
            "index": 0,
            "delta": {"content": content},
            "finish_reason": "stop"
        }],
        "usage": null
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

#[tokio::test]
async fn operational_routes_preserve_primary_passthrough_for_unmatched_models() {
    let primary = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "passthrough-model"}]
        })))
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(chat_sse_body("chat-primary", "served by primary")),
        )
        .expect(1)
        .mount(&primary)
        .await;

    let config = base_config(&format!("{}/v1/", primary.uri()));
    assert!(
        config.upstreams.is_empty(),
        "fixture must remain routes-only"
    );
    assert!(
        config.model_routes.is_empty(),
        "only the operational route should engage routing mode"
    );
    let (app, _gateway) = llmconduit::build_app_with_gateway_control_plane(
        config,
        None,
        AppOptions::default(),
        vec![operational_route(
            "managed-alias",
            "http://127.0.0.1:11/v1/",
        )],
        UnknownModelPolicy::Passthrough,
        None,
    )
    .expect("routes-only control-plane app");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": "passthrough-model",
                        "stream": false,
                        "messages": [{"role": "user", "content": "hello"}]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("gateway response");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("response body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("response JSON");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "served by primary"
    );
}

#[tokio::test]
async fn operational_alias_precedes_global_upstream_model_remap() {
    let primary = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "primary-catalog-model"}]
        })))
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&primary)
        .await;

    let operational = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({"model": "operational-target"})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(chat_sse_body(
                    "chat-operational",
                    "served by operational backend",
                )),
        )
        .expect(1)
        .mount(&operational)
        .await;

    let mut config = base_config(&format!("{}/v1/", primary.uri()));
    config.upstream_model = Some("primary-remap".to_string());
    let mut route = operational_route("managed-model", &format!("{}/v1/", operational.uri()));
    route.providers[0].upstream_model = Some("operational-target".to_string());
    let (app, _gateway) = llmconduit::build_app_with_gateway_control_plane(
        config,
        None,
        AppOptions::default(),
        vec![route],
        UnknownModelPolicy::Passthrough,
        None,
    )
    .expect("operational alias app");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": "managed-model",
                        "stream": false,
                        "messages": [{"role": "user", "content": "hello"}]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("gateway response");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("response body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("response JSON");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "served by operational backend"
    );

    let primary_chat_requests = primary
        .received_requests()
        .await
        .expect("primary requests")
        .into_iter()
        .filter(|request| {
            request.method.as_str() == "POST" && request.url.path() == "/v1/chat/completions"
        })
        .count();
    assert_eq!(
        primary_chat_requests, 0,
        "global upstream_model must not steal an operational alias"
    );
}

/// Send one chat request carrying an image to an operational alias whose
/// provider rewrites the model, after recording `probe_verdict` for the
/// UPSTREAM model id the way the background vision probe does, and return the
/// chat body the backend actually received.
async fn chat_with_image_through_probed_alias(probe_verdict: bool) -> serde_json::Value {
    // Mirrors production: profile/route `chandra-ocr-2` served by vLLM under
    // the id `datalab-to/chandra-ocr-2`, no profile `native_vision` override.
    const ALIAS: &str = "chandra-ocr-2";
    const UPSTREAM_MODEL: &str = "datalab-to/chandra-ocr-2";
    let primary = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "primary-catalog-model"}]
        })))
        .mount(&primary)
        .await;

    let operational = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({"model": UPSTREAM_MODEL})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(chat_sse_body("chat-ocr", "ocr text")),
        )
        .expect(1)
        .mount(&operational)
        .await;

    let mut route = operational_route(ALIAS, &format!("{}/v1/", operational.uri()));
    route.providers[0].upstream_model = Some(UPSTREAM_MODEL.to_string());
    let backend_name = route.providers[0].backend_name.clone();
    let (app, gateway) = llmconduit::build_app_with_gateway_control_plane(
        base_config(&format!("{}/v1/", primary.uri())),
        None,
        AppOptions::default(),
        vec![route],
        UnknownModelPolicy::Passthrough,
        None,
    )
    .expect("operational alias app");
    // The probe targets come from `targets_from_routes`, which key on the
    // provider's upstream model id, so that is the id the verdict lands under.
    gateway
        .native_vision_cache()
        .record(&backend_name, UPSTREAM_MODEL, probe_verdict);

    let image_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": ALIAS,
                        "stream": false,
                        "messages": [{"role": "user", "content": [
                            {"type": "image_url", "image_url": {"url": image_url}},
                            {"type": "text", "text": "OCR this page"}
                        ]}]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("gateway response");
    assert_eq!(response.status(), http::StatusCode::OK);

    let requests = operational
        .received_requests()
        .await
        .expect("operational requests");
    let chat = requests
        .iter()
        .find(|request| request.url.path() == "/v1/chat/completions")
        .expect("backend received the chat request");
    serde_json::from_slice(&chat.body).expect("upstream chat JSON")
}

#[tokio::test]
async fn operational_alias_passes_images_when_probe_accepted_upstream_model() {
    let upstream = chat_with_image_through_probed_alias(true).await;
    let serialized = upstream.to_string();
    assert!(
        serialized.contains("data:image/png;base64,"),
        "probe said the upstream model takes images, so the image must reach it: {serialized}"
    );
    assert!(
        !serialized.contains("image omitted"),
        "image must not be degraded to a placeholder: {serialized}"
    );
}

#[tokio::test]
async fn operational_alias_strips_images_when_probe_rejected_upstream_model() {
    let upstream = chat_with_image_through_probed_alias(false).await;
    let serialized = upstream.to_string();
    assert!(
        !serialized.contains("data:image/png;base64,"),
        "a probe rejection must keep image bytes away from the backend: {serialized}"
    );
}

/// vLLM reports a failure after the response started as an SSE `{"error": {...}}` event (HTTP
/// 200 already sent). The gateway must relay that message, not a chunk-parse error
/// (`missing field 'id'`) that hides e.g. an EngineCore crash.
#[tokio::test]
async fn upstream_sse_error_event_is_relayed_with_its_message() {
    let primary = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "passthrough-model"}]
        })))
        .mount(&primary)
        .await;
    let error_event = json!({"error": {
        "message": "EngineCore encountered an issue. See stack trace (above) for the root cause.",
        "type": "InternalServerError", "param": null, "code": 500
    }});
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {error_event}\n\n")),
        )
        .mount(&primary)
        .await;

    let config = base_config(&format!("{}/v1/", primary.uri()));
    let (app, _gateway) = llmconduit::build_app_with_gateway_control_plane(
        config,
        None,
        AppOptions::default(),
        vec![operational_route(
            "managed-alias",
            "http://127.0.0.1:11/v1/",
        )],
        UnknownModelPolicy::Passthrough,
        None,
    )
    .expect("routes-only control-plane app");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": "passthrough-model",
                        "stream": false,
                        "messages": [{"role": "user", "content": "hello"}]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("gateway response");

    assert!(!response.status().is_success());
    let body = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .expect("response body");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("EngineCore encountered an issue"), "{text}");
    assert!(!text.contains("missing field"), "{text}");
}

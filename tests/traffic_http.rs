use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Response},
    routing::post,
};
use futures::StreamExt;
use llmconduit::{
    models::chat::ChatCompletionRequest,
    traffic::{BackendPolicy, Mode, TrafficConfig, TrafficController},
    upstream::{BackendChatRequest, ReqwestUpstreamClient, UpstreamClient},
};
use std::sync::Arc;

#[tokio::test]
async fn admission_lease_covers_http_stream_and_uses_gateway_owned_correlation_id() {
    let release = Arc::new(tokio::sync::Notify::new());
    let header = Arc::new(std::sync::Mutex::new(String::new()));
    let route = {
        let release = release.clone();
        let header = header.clone();
        post(move |headers: HeaderMap| {
            let release = release.clone();
            let header = header.clone();
            async move {
                *header.lock().unwrap() = headers
                    .get("x-request-id")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                let body = async_stream::stream! {yield Ok::<_,std::io::Error>("data: {\"id\":\"chat\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n");release.notified().await;yield Ok("data: [DONE]\n\n");};
                Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from_stream(body))
                    .unwrap()
            }
        })
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/v1/chat/completions", route))
            .await
            .unwrap();
    });
    let url = format!("http://{addr}/v1");
    let traffic = TrafficController::new();
    let mut config = TrafficConfig {
        mode: Mode::Enforce,
        ..Default::default()
    };
    config.backends.insert(
        url.clone(),
        BackendPolicy {
            max_concurrent: Some(1),
            ..Default::default()
        },
    );
    traffic.install(config, 1).unwrap();
    let client = ReqwestUpstreamClient::new(
        reqwest::Client::new(),
        url.parse().unwrap(),
        None,
        None,
        false,
        0,
    )
    .with_traffic(traffic.clone());
    let request:ChatCompletionRequest=serde_json::from_value(serde_json::json!({"model":"test","messages":[{"role":"user","content":"hello"}],"stream":true})).unwrap();
    let backend = BackendChatRequest::new(request, None, None, None);
    let mut stream = client.stream_chat_completion(&backend).await.unwrap();
    assert_eq!(traffic.snapshot().active.len(), 1);
    assert_eq!(
        uuid::Uuid::parse_str(&header.lock().unwrap())
            .unwrap()
            .to_string(),
        traffic.snapshot().active[0].id
    );
    assert!(stream.next().await.unwrap().is_ok());
    assert_eq!(
        client
            .stream_chat_completion(&backend)
            .await
            .err()
            .unwrap()
            .status
            .as_u16(),
        503
    );
    drop(stream);
    assert!(traffic.snapshot().active.is_empty());
    assert_eq!(traffic.snapshot().clients, 0);
    let mut stream = client.stream_chat_completion(&backend).await.unwrap();
    assert!(stream.next().await.unwrap().is_ok());
    release.notify_one();
    while stream.next().await.is_some() {}
    assert!(traffic.snapshot().active.is_empty());
    server.abort();
}

mod common;
#[tokio::test]
async fn ordinary_providers_stay_independent_until_an_explicit_replica_pool_is_enabled() {
    use llmconduit::{config::UpstreamConfig, traffic::ModelPolicy};
    use tower::ServiceExt;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let a = MockServer::start().await;
    let b = MockServer::start().await;
    for server in [&a, &b] {
        Mock::given(method("GET")).and(path("/v1/models")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"object":"list","data":[{"id":"same-model","object":"model","owned_by":"fixture"}]}))).mount(server).await;
    }
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_json(serde_json::json!({"error":{"message":"fixture failure"}})),
        )
        .mount(&a)
        .await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(ResponseTemplate::new(200).insert_header("content-type","text/event-stream").set_body_string("data: {\"id\":\"chat\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")).mount(&b).await;
    let mut config = common::test_config();
    config.brave_api_key = None;
    config.upstream_failure_cooldown_secs = 0;
    config.upstreams = [("a", &a), ("b", &b)]
        .into_iter()
        .map(|(name, server)| UpstreamConfig {
            name: name.into(),
            upstream_base_url: format!("{}/v1", server.uri()).parse().unwrap(),
            upstream_api_key: None,
            upstream_model: None,
            upstream_chat_kwargs: Default::default(),
            upstream_request_log_path: None,
            fallback_upstreams: vec![],
        })
        .collect();
    config.model_routes = vec![llmconduit::config::ModelRoute {
        name: "public-alias".into(),
        glob: None,
        upstream_base_url: format!("{}/v1", a.uri()).parse().unwrap(),
        upstream_model: Some("same-model".into()),
    }];
    let (app, gateway) = llmconduit::build_app_with_gateway_and_options(
        config,
        None,
        llmconduit::AppOptions {
            with_debug_ui: false,
        },
    );
    let request = || {
        axum::http::Request::builder().method("POST").uri("/v1/chat/completions").header("content-type","application/json").body(Body::from(r#"{"model":"same-model","messages":[{"role":"user","content":"hi"}],"stream":false,"max_tokens":32}"#)).unwrap()
    };
    let response = app.clone().oneshot(request()).await.unwrap();
    assert!(!response.status().is_success());
    assert!(
        b.received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method != "POST")
    );
    let mut policy = TrafficConfig {
        mode: Mode::Enforce,
        ..Default::default()
    };
    policy.models.insert(
        "public-alias".into(),
        ModelPolicy {
            replicas: vec!["a".into(), "b".into()],
            balance: true,
            ..Default::default()
        },
    );
    gateway.traffic().install(policy, 1).unwrap();
    let alias_request=axum::http::Request::builder().method("POST").uri("/v1/chat/completions").header("content-type","application/json").body(Body::from(r#"{"model":"public-alias","messages":[{"role":"user","content":"hi"}],"stream":false,"max_tokens":32}"#)).unwrap();
    let response = app.oneshot(alias_request).await.unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let body = axum::body::to_bytes(response.into_body(), 10000)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("hello"));
    assert_eq!(
        b.received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
}

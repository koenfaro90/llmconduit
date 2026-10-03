//! Task APIs preserve vendor fields and binary responses; video job handles pin
//! follow-up requests to the original backend and authenticating principal.
use crate::{
    engine::Gateway,
    error::{AppError, AppResult},
    upstream::{InferenceEndpoint, ProxyCompletionsRequest},
};
use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, ops::Range, sync::Arc};

#[derive(Debug, Clone)]
pub struct MultipartModel {
    pub model: String,
    pub value_range: Range<usize>,
}
#[derive(Debug, Clone)]
pub struct PreparedMultipart {
    pub body: Bytes,
    pub content_type: HeaderValue,
    pub model: MultipartModel,
}
pub fn is_media_endpoint(endpoint: InferenceEndpoint) -> bool {
    !matches!(
        endpoint,
        InferenceEndpoint::Responses
            | InferenceEndpoint::ChatCompletions
            | InferenceEndpoint::Messages
            | InferenceEndpoint::CountTokens
            | InferenceEndpoint::Completions
            | InferenceEndpoint::Embeddings
            | InferenceEndpoint::Rerank
    )
}
pub fn endpoint(path: &str) -> Option<InferenceEndpoint> {
    Some(match path {
        "/v1/images/generations" => InferenceEndpoint::Images,
        "/v1/images/edits" => InferenceEndpoint::ImageEdits,
        "/v1/videos" => InferenceEndpoint::Videos,
        "/v1/videos/sync" => InferenceEndpoint::VideoSync,
        "/v1/audio/speech" => InferenceEndpoint::AudioSpeech,
        "/v1/audio/generate" => InferenceEndpoint::AudioGenerate,
        "/v1/audio/transcriptions" => InferenceEndpoint::AudioTranscriptions,
        "/v1/audio/translations" => InferenceEndpoint::AudioTranslations,
        _ => return None,
    })
}
pub fn is_media_path(path: &str) -> bool {
    endpoint(path).is_some() || path.starts_with("/v1/videos/")
}
fn quoted(text: &str) -> AppResult<String> {
    if text.chars().any(|c| c.is_control()) {
        return Err(AppError::bad_request("invalid multipart field metadata"));
    }
    Ok(text.replace('\\', "\\\\").replace('"', "\\\""))
}
/// Parse MIME using multer, then normalize once to a boundary and a known model
/// byte range. Rewrites replace only this range, preserving uploaded file bytes.
pub async fn prepare_multipart(headers: &HeaderMap, body: Bytes) -> AppResult<PreparedMultipart> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| AppError::bad_request("multipart content type is required"))?;
    let boundary = multer::parse_boundary(content_type)
        .map_err(|_| AppError::bad_request("invalid multipart boundary"))?;
    let cap = body.len().saturating_add(64 * 1024);
    let stream = futures::stream::once(async move { Ok::<_, std::io::Error>(body) });
    let mut multipart = multer::Multipart::new(stream, boundary);
    let boundary = format!("llmconduit-{}", uuid::Uuid::new_v4().simple());
    let mut output = Vec::new();
    let mut model = None;
    let mut count = 0;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::bad_request("malformed multipart body"))?
    {
        count += 1;
        if count > 128 {
            return Err(AppError::bad_request(
                "multipart requests support at most 128 fields",
            ));
        }
        let name = field
            .name()
            .ok_or_else(|| AppError::bad_request("multipart fields require a name"))?
            .to_owned();
        let filename = field.file_name().map(str::to_owned);
        let mime = field.content_type().map(ToString::to_string);
        let bytes = field
            .bytes()
            .await
            .map_err(|_| AppError::bad_request("malformed multipart field"))?;
        output.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"",
                quoted(&name)?
            )
            .as_bytes(),
        );
        if let Some(filename) = &filename {
            output.extend_from_slice(format!("; filename=\"{}\"", quoted(filename)?).as_bytes());
        }
        output.extend_from_slice(b"\r\n");
        if let Some(mime) = mime {
            output.extend_from_slice(format!("Content-Type: {mime}\r\n").as_bytes());
        }
        output.extend_from_slice(b"\r\n");
        let start = output.len();
        output.extend_from_slice(&bytes);
        if name == "model" {
            if model.is_some() || filename.is_some() || bytes.len() > 1024 {
                return Err(AppError::bad_request("one textual model field is required"));
            }
            let value = std::str::from_utf8(&bytes)
                .map_err(|_| AppError::bad_request("model must be UTF-8"))?
                .trim();
            if value.is_empty() {
                return Err(AppError::bad_request("a model is required"));
            }
            model = Some(MultipartModel {
                model: value.to_owned(),
                value_range: start..output.len(),
            });
        }
        output.extend_from_slice(b"\r\n");
        if output.len() > cap {
            return Err(AppError::bad_request(
                "multipart normalization exceeds the request bound",
            ));
        }
    }
    output.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(PreparedMultipart {
        body: Bytes::from(output),
        content_type: HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}"))
            .unwrap(),
        model: model.ok_or_else(|| AppError::bad_request("a model is required"))?,
    })
}

pub async fn generate(
    State(gateway): State<Arc<Gateway>>,
    auth: Option<Extension<crate::authz::AuthContext>>,
    api_id: Option<Extension<crate::dashboard_flow::ApiCallId>>,
    multipart: Option<Extension<PreparedMultipart>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    let endpoint =
        endpoint(uri.path()).ok_or_else(|| AppError::bad_request("unknown media endpoint"))?;
    if endpoint == InferenceEndpoint::Videos && gateway.persistence_store().is_none() {
        return Err(AppError::upstream(
            "SQL persistence is required for asynchronous video jobs",
        ));
    }
    if endpoint == InferenceEndpoint::Videos && jobs(&gateway).await?.1.len() >= 2048 {
        return Err(AppError::upstream(
            "video job limit reached; delete completed jobs before creating more",
        ));
    }
    let mut request = ProxyCompletionsRequest::new(headers, body).with_endpoint(endpoint);
    if let Some(Extension(multipart)) = multipart {
        request.multipart = Some(multipart.model);
    }
    crate::http::proxy_prepared_request(gateway, auth, request, api_id.map(|id| id.0.0), true).await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VideoJob {
    upstream_id: String,
    model: String,
    base_url: String,
    owner: String,
    created_at_ms: i64,
}
const JOBS_KEY: &str = "media.video-jobs.v1";
pub(crate) fn owner(auth: Option<&crate::authz::AuthContext>, headers: &HeaderMap) -> String {
    use sha2::{Digest, Sha256};
    if let Some(auth) = auth {
        return format!("principal:{}", auth.principal_id);
    }
    // Legacy virtual-key auth is enforced by ingress middleware. Hashing its
    // credential keeps jobs private without persisting the credential itself.
    if let Some(value) = crate::authz::presented_key(headers) {
        return format!("legacy:{:x}", Sha256::digest(value.as_bytes()));
    }
    "auth-disabled".into()
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}
async fn jobs(gateway: &Gateway) -> AppResult<(Option<String>, BTreeMap<String, VideoJob>)> {
    let store = gateway
        .persistence_store()
        .ok_or_else(|| AppError::upstream("SQL persistence is required for video jobs"))?;
    let raw = store
        .get_setting(JOBS_KEY)
        .await
        .map_err(|_| AppError::upstream("video job storage unavailable"))?;
    let jobs = raw
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| AppError::internal("invalid video job state"))?
        .unwrap_or_default();
    Ok((raw, jobs))
}
pub(crate) async fn register_video(
    gateway: &Gateway,
    response: reqwest::Response,
    requested_model: &str,
    request_owner: String,
) -> AppResult<Response> {
    let status = response.status();
    let mut headers = response.headers().clone();
    let base_url = headers
        .get("x-llmconduit-upstream")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::internal("video response is missing backend attribution"))?
        .to_owned();
    headers.remove("content-length");
    headers.remove("x-llmconduit-upstream");
    let mut value = crate::media_api::read_job_json(response).await?;
    let upstream_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or_else(|| AppError::upstream("video backend returned an invalid job id"))?
        .to_owned();
    let id = format!("video_{}", uuid::Uuid::new_v4().simple());
    let job = VideoJob {
        upstream_id,
        model: requested_model.to_owned(),
        base_url,
        owner: request_owner,
        created_at_ms: crate::mesh::store::now_ms(),
    };
    let store = gateway.persistence_store().unwrap();
    let mut saved = false;
    for _ in 0..4 {
        let (raw, mut map) = jobs(gateway).await?;
        if map.len() >= 2048 {
            return Err(AppError::upstream(
                "video job limit reached; delete completed jobs before creating more",
            ));
        }
        map.insert(id.clone(), job.clone());
        let encoded = serde_json::to_string(&map)
            .map_err(|_| AppError::internal("cannot encode video jobs"))?;
        if store
            .compare_exchange_setting(JOBS_KEY, raw.as_deref(), &encoded)
            .await
            .map_err(|_| AppError::upstream("video job storage unavailable"))?
        {
            saved = true;
            break;
        }
    }
    if !saved {
        return Err(AppError::upstream(
            "video job state changed; backend may have accepted the generation",
        ));
    }
    value["id"] = Value::String(id);
    let mut out = (status, Json(value)).into_response();
    headers.remove("content-type");
    crate::http::copy_proxy_response_headers(&headers, out.headers_mut());
    Ok(out)
}
pub(crate) async fn read_job_json(response: reqwest::Response) -> AppResult<Value> {
    use futures::StreamExt;
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AppError::upstream("failed to read video job response"))?;
        if bytes.len() + chunk.len() > 512 * 1024 {
            return Err(AppError::upstream("video job JSON exceeds 512 KiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::upstream("video job response must be JSON"))
}
pub async fn video_job(
    State(gateway): State<Arc<Gateway>>,
    auth: Option<Extension<crate::authz::AuthContext>>,
    legacy: Option<Extension<crate::client_auth::ClientIdentity>>,
    Path(id): Path<String>,
    uri: axum::http::Uri,
    method: axum::http::Method,
    headers: HeaderMap,
) -> AppResult<Response> {
    if !valid_id(&id) {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let (_, map) = jobs(&gateway).await?;
    let Some(job) = map
        .get(&id)
        .filter(|j| j.owner == owner(auth.as_ref().map(|a| &a.0), &headers))
    else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if legacy.as_ref().is_some_and(|a| !a.allows_model(&job.model)) {
        return Err(AppError::forbidden(
            "the API key is not authorized for this video model",
        ));
    }
    let content = uri.path().ends_with("/content");
    let mut request = ProxyCompletionsRequest::new(
        headers,
        Bytes::from(serde_json::to_vec(&serde_json::json!({"model":job.model})).unwrap()),
    )
    .with_endpoint(InferenceEndpoint::VideoJob);
    request.resource_path = Some(format!(
        "videos/{}{}",
        job.upstream_id,
        if content { "/content" } else { "" }
    ));
    request.method = method.clone();
    request.pinned_base_url = Some(job.base_url.clone());
    // Follow-ups are authorized again, including current model/provider policy.
    let response =
        crate::http::proxy_prepared_request(gateway.clone(), auth, request, None, false).await?;
    if content || !response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let mut headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 512 * 1024)
        .await
        .map_err(|_| AppError::upstream("video job JSON exceeds bound"))?;
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::upstream("invalid video job response"))?;
    value
        .as_object_mut()
        .ok_or_else(|| AppError::upstream("video job response must be an object"))?
        .insert("id".into(), Value::String(id.clone()));
    if method == axum::http::Method::DELETE {
        let store = gateway.persistence_store().unwrap();
        for _ in 0..4 {
            let (raw, mut map) = jobs(&gateway).await?;
            map.remove(&id);
            let encoded = serde_json::to_string(&map).unwrap();
            if store
                .compare_exchange_setting(JOBS_KEY, raw.as_deref(), &encoded)
                .await
                .map_err(|_| AppError::upstream("video job storage unavailable"))?
            {
                break;
            }
        }
    }
    headers.remove("content-length");
    headers.remove("content-type");
    let mut out = (status, Json(value)).into_response();
    crate::http::copy_proxy_response_headers(&headers, out.headers_mut());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn multipart_model_rewrites_preserve_binary_and_vendor_fields() {
        let binary = b"\x00\xff\r\nmodel=alias\x00";
        let mut body=b"--test\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nalias\r\n--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"sound.wav\"\r\nContent-Type: audio/wav\r\n\r\n".to_vec();
        body.extend_from_slice(binary);
        body.extend_from_slice(b"\r\n--test\r\nContent-Disposition: form-data; name=\"temperature\"\r\n\r\n0.3\r\n--test--\r\n");
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            HeaderValue::from_static("multipart/form-data; boundary=test"),
        );
        let prepared = prepare_multipart(&headers, Bytes::from(body))
            .await
            .unwrap();
        headers.insert("content-type", prepared.content_type);
        let mut request = ProxyCompletionsRequest::new(headers, prepared.body);
        request.multipart = Some(prepared.model);
        request.set_model("actual-longer-name");
        request.set_model("actual");
        assert_eq!(request.model().as_deref(), Some("actual"));
        assert!(
            request
                .body
                .windows(binary.len())
                .any(|part| part == binary)
        );
        assert!(request.body.windows(3).any(|part| part == b"0.3"));
        let parsed = prepare_multipart(&request.headers, request.body)
            .await
            .unwrap();
        assert_eq!(parsed.model.model, "actual");
    }
    #[tokio::test]
    async fn duplicate_and_file_model_fields_are_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            HeaderValue::from_static("multipart/form-data; boundary=test"),
        );
        let duplicate=b"--test\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\na\r\n--test\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nb\r\n--test--\r\n";
        assert!(
            prepare_multipart(&headers, Bytes::from_static(duplicate))
                .await
                .is_err()
        );
    }
    #[test]
    fn video_job_metadata_round_trips_and_ids_are_paths() {
        let job = VideoJob {
            upstream_id: "video_backend".into(),
            model: "alias".into(),
            base_url: "http://localhost/v1".into(),
            owner: "principal:user".into(),
            created_at_ms: 1,
        };
        let restored: VideoJob =
            serde_json::from_value(serde_json::to_value(&job).unwrap()).unwrap();
        assert_eq!(restored.owner, job.owner);
        assert_eq!(restored.base_url, job.base_url);
        assert!(!valid_id("../other"));
    }
}

macro_rules! generation_handler {
    ($name:ident,$path:literal,$content_type:literal) => {
        #[utoipa::path(post,path=$path,request_body(content=Value,content_type=$content_type),responses((status=200,description="Upstream JSON, binary media or SSE; vendor fields are preserved"),(status=400),(status=401),(status=403),(status=413)),tag="media")]
        pub async fn $name(state:State<Arc<Gateway>>,auth:Option<Extension<crate::authz::AuthContext>>,api_id:Option<Extension<crate::dashboard_flow::ApiCallId>>,multipart:Option<Extension<PreparedMultipart>>,uri:axum::http::Uri,headers:HeaderMap,body:Bytes)->AppResult<Response>{
            generate(state,auth,api_id,multipart,uri,headers,body).await
        }
    };
}
generation_handler!(
    generate_images,
    "/v1/images/generations",
    "application/json"
);
generation_handler!(edit_images, "/v1/images/edits", "multipart/form-data");
generation_handler!(generate_videos, "/v1/videos", "multipart/form-data");
generation_handler!(
    generate_video_sync,
    "/v1/videos/sync",
    "multipart/form-data"
);
generation_handler!(generate_speech, "/v1/audio/speech", "application/json");
generation_handler!(generate_audio, "/v1/audio/generate", "application/json");
generation_handler!(
    transcribe_audio,
    "/v1/audio/transcriptions",
    "multipart/form-data"
);
generation_handler!(
    translate_audio,
    "/v1/audio/translations",
    "multipart/form-data"
);

macro_rules! job_handler {
    ($name:ident,$method:ident,$path:literal) => {
        #[utoipa::path($method,path=$path,params(("id"=String,Path)),responses((status=200,description="Video job JSON or binary video content"),(status=401),(status=403),(status=404)),tag="media")]
        pub async fn $name(state:State<Arc<Gateway>>,auth:Option<Extension<crate::authz::AuthContext>>,legacy:Option<Extension<crate::client_auth::ClientIdentity>>,id:Path<String>,uri:axum::http::Uri,method:axum::http::Method,headers:HeaderMap)->AppResult<Response>{
            video_job(state,auth,legacy,id,uri,method,headers).await
        }
    };
}
job_handler!(get_video, get, "/v1/videos/{id}");
job_handler!(delete_video, delete, "/v1/videos/{id}");
job_handler!(download_video, get, "/v1/videos/{id}/content");

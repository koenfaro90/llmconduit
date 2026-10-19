//! Exact HTTP entity bodies, independent of capped/redacted diagnostic previews.
//!
//! Bodies stream to private files without a size ceiling. Each actual upstream
//! send gets a separate exchange, including retries and server-tool rounds. Disk
//! writes apply backpressure; no lossy telemetry queue carries these bytes.
use axum::body::Body;
use axum::body::Bytes;
use axum::http::{HeaderMap, Response};
use futures::StreamExt;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct WireHeader {
    pub name: String,
    /// Base64 is lossless even for a non-UTF8 HTTP header value.
    pub value_base64: String,
}
fn headers(values: &HeaderMap) -> Vec<WireHeader> {
    use base64::Engine;
    values
        .iter()
        .map(|(name, value)| WireHeader {
            name: name.to_string(),
            value_base64: base64::engine::general_purpose::STANDARD.encode(value.as_bytes()),
        })
        .collect()
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct WireChunk {
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct WirePart {
    pub file: String,
    #[serde(default)]
    pub chunks: Vec<WireChunk>,
    pub headers: Vec<WireHeader>,
    pub method: Option<String>,
    pub target: Option<String>,
    pub status: Option<u16>,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub complete: bool,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct WireManifest {
    pub schema: String,
    pub api_call_id: String,
    pub parts: BTreeMap<String, WirePart>,
}
#[derive(Debug)]
pub struct WireTurn {
    dir: PathBuf,
    manifest: AsyncMutex<WireManifest>,
    next_exchange: std::sync::atomic::AtomicU64,
    /// Serialize publishes so an older manifest can never replace a newer one.
    publish: AsyncMutex<()>,
}
struct Writer {
    file: tokio::fs::File,
    bytes: u64,
    hash: Sha256,
}
pub struct WireBody {
    turn: Arc<WireTurn>,
    key: String,
    writer: AsyncMutex<Option<Writer>>,
}
impl std::fmt::Debug for WireBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireBody")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}
impl WireTurn {
    pub async fn start(
        root: &Path,
        id: &str,
        method: &str,
        target: &str,
        values: &HeaderMap,
        raw: &[u8],
    ) -> std::io::Result<Arc<Self>> {
        let dir = root.join(id);
        tokio::fs::create_dir_all(&dir).await?;
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).await?;
        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).await?;
        let turn = Arc::new(Self {
            dir,
            manifest: AsyncMutex::new(WireManifest {
                schema: "llmconduit-exact-http/v1".into(),
                api_call_id: id.into(),
                parts: BTreeMap::new(),
            }),
            next_exchange: std::sync::atomic::AtomicU64::new(0),
            publish: AsyncMutex::new(()),
        });
        let body = turn
            .body("client_in", values, Some(method), Some(target), None)
            .await?;
        body.write(raw).await?;
        body.finish(true, None).await?;
        Ok(turn)
    }
    async fn publish(&self) -> std::io::Result<()> {
        let _lock = self.publish.lock().await;
        let data = serde_json::to_vec(&*self.manifest.lock().await)?;
        let path = self.dir.join("manifest.tmp");
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .await?;
        file.write_all(&data).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(path, self.dir.join("manifest.json")).await
    }
    async fn body(
        self: &Arc<Self>,
        key: &str,
        values: &HeaderMap,
        method: Option<&str>,
        target: Option<&str>,
        status: Option<u16>,
    ) -> std::io::Result<Arc<WireBody>> {
        let name = format!("{key}.body");
        let file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(self.dir.join(&name))
            .await?;
        self.manifest.lock().await.parts.insert(
            key.into(),
            WirePart {
                file: name,
                chunks: Vec::new(),
                headers: headers(values),
                method: method.map(str::to_owned),
                target: target.map(str::to_owned),
                status,
                bytes: 0,
                sha256: None,
                complete: false,
                error: None,
            },
        );
        self.publish().await?;
        Ok(Arc::new(WireBody {
            turn: Arc::clone(self),
            key: key.into(),
            writer: AsyncMutex::new(Some(Writer {
                file,
                bytes: 0,
                hash: Sha256::new(),
            })),
        }))
    }
    pub async fn upstream(
        self: &Arc<Self>,
        request: &reqwest::Request,
    ) -> std::io::Result<(String, Arc<WireBody>)> {
        let number = self
            .next_exchange
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let key = format!("upstream_{number:06}");
        let body = self
            .body(
                &(key.clone() + "_out"),
                request.headers(),
                Some(request.method().as_str()),
                Some(request.url().as_str()),
                None,
            )
            .await?;
        // Chat requests are reqwest's finalized JSON byte body. Capture the same
        // allocation handed to execute, rather than serializing the value again.
        let raw = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .ok_or_else(|| std::io::Error::other("upstream request is not a byte body"))?;
        body.write(raw).await?;
        body.finish(true, None).await?;
        Ok((key, body))
    }
    pub async fn response(
        self: &Arc<Self>,
        key: &str,
        status: u16,
        values: &HeaderMap,
    ) -> std::io::Result<Arc<WireBody>> {
        self.body(key, values, None, None, Some(status)).await
    }
}
impl WireBody {
    async fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut guard = self.writer.lock().await;
        let writer = guard
            .as_mut()
            .ok_or_else(|| std::io::Error::other("wire body already closed"))?;
        writer.file.write_all(bytes).await?;
        writer.hash.update(bytes);
        writer.bytes += bytes.len() as u64;
        Ok(())
    }
    pub async fn finish(&self, complete: bool, error: Option<String>) -> std::io::Result<()> {
        let Some(mut writer) = self.writer.lock().await.take() else {
            return Ok(());
        };
        writer.file.flush().await?;
        writer.file.sync_all().await?;
        // The chunk size is an IO unit, never a body limit. Concatenating these
        // immutable blobs reproduces every original byte, including whitespace.
        let raw_path = self.turn.dir.join(format!("{}.body", self.key));
        let chunks = deduplicate(&raw_path, self.turn.dir.parent().expect("capture root")).await?;
        let mut manifest = self.turn.manifest.lock().await;
        let part = manifest
            .parts
            .get_mut(&self.key)
            .expect("wire part registered");
        part.bytes = writer.bytes;
        part.chunks = chunks;
        part.sha256 = Some(format!("{:x}", writer.hash.finalize()));
        part.complete = complete;
        part.error = error;
        drop(manifest);
        self.turn.publish().await?;
        if writer.bytes > 0 {
            tokio::fs::remove_file(raw_path).await?;
        }
        Ok(())
    }
}
/// Finalize a cancelled reader honestly. No captured file is deleted.
struct BodyGuard {
    body: Arc<WireBody>,
    clean: bool,
}
impl Drop for BodyGuard {
    fn drop(&mut self) {
        if !self.clean {
            let body = Arc::clone(&self.body);
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = body
                        .finish(false, Some("HTTP body did not reach clean EOF".into()))
                        .await;
                });
            }
        }
    }
}
/// Drain the network independently of the parser. A failed parse, context retry
/// or dropped consumer must not discard the rest of an actual HTTP response.
pub async fn upstream_response(
    response: reqwest::Response,
    turn: Arc<WireTurn>,
    key: String,
) -> std::io::Result<reqwest::Response> {
    let status = response.status();
    let version = response.version();
    let values = response.headers().clone();
    let body = turn
        .response(&(key + "_in"), status.as_u16(), &values)
        .await?;
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        let mut guard = BodyGuard {
            body: Arc::clone(&body),
            clean: false,
        };
        let mut stream = std::pin::pin!(response.bytes_stream());
        let mut sender = Some(tx);
        loop {
            // Preserve normal inference cancellation. Error HTTP bodies are
            // drained after the error reader's preview ends; a cancelled success
            // stream stops its transport rather than generating unused tokens.
            let item = if status.is_success() {
                let Some(tx) = sender.as_ref() else { return };
                tokio::select! {item=stream.next()=>item,_=tx.closed()=>{return;}}
            } else {
                stream.next().await
            };
            let Some(item) = item else { break };
            if let Ok(bytes) = &item {
                if let Err(error) = body.write(bytes).await {
                    let _ = body.finish(false, Some(error.to_string())).await;
                    return;
                }
            } else {
                let _ = body
                    .finish(false, Some("upstream HTTP transport error".into()))
                    .await;
            }
            let failed = item.is_err();
            if let Some(tx) = &sender
                && tx.send(item).await.is_err()
            {
                sender = None;
            }
            if failed {
                return;
            }
        }
        if body.finish(true, None).await.is_ok() {
            guard.clean = true;
        }
    });
    let mut out = Response::builder()
        .status(status)
        .version(version)
        .body(reqwest::Body::wrap_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .map_err(std::io::Error::other)?;
    *out.headers_mut() = values;
    Ok(reqwest::Response::from(out))
}
/// The client-facing HTTP body, before any display/parsing/adapters touch it.
pub async fn served_response(
    response: axum::response::Response,
    turn: Arc<WireTurn>,
) -> std::io::Result<axum::response::Response> {
    let (mut parts, body) = response.into_parts();
    if !parts
        .headers
        .contains_key(axum::http::header::CONTENT_LENGTH)
        && let Some(size) = http_body::Body::size_hint(&body).exact()
    {
        parts
            .headers
            .insert(axum::http::header::CONTENT_LENGTH, size.into());
    }
    let exact_length = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let capture = turn
        .response("client_out", parts.status.as_u16(), &parts.headers)
        .await?;
    // Own the guard before first poll: dropping an unpolled response body must
    // still close its archive as incomplete rather than leave it "recording".
    if exact_length == Some(0) {
        capture.finish(true, None).await?;
    }
    let guard = BodyGuard {
        body: Arc::clone(&capture),
        clean: exact_length == Some(0),
    };
    let stream = async_stream::stream! {
        let mut guard=guard;
        let mut body=body;let mut captured=0u64;
        while let Some(frame)=body.frame().await {
            if let Ok(frame)=&frame && let Some(data)=frame.data_ref() && let Err(error)=capture.write(data).await {yield Err::<http_body::Frame<Bytes>,_>(std::io::Error::other(error));return;}
            if let Ok(frame)=&frame && let Some(data)=frame.data_ref() {
                captured+=data.len()as u64;
                // Hyper may stop polling exactly at Content-Length and never
                // request EOF. Commit the complete entity before yielding its
                // last data frame; lack of a subsequent poll is not data loss.
                if exact_length==Some(captured) {match capture.finish(true,None).await {Ok(())=>guard.clean=true,Err(error)=>{yield Err(std::io::Error::other(error));return;}}}
            }
            match frame {Ok(frame)=>yield Ok(frame),Err(error)=>{yield Err(std::io::Error::other(error));return;}}
        }
        match capture.finish(true,None).await {Ok(())=>guard.clean=true,Err(error)=>yield Err(std::io::Error::other(error))}
    };
    Ok(Response::from_parts(
        parts,
        Body::new(http_body_util::StreamBody::new(stream)),
    ))
}

pub fn valid_id(id: &str) -> bool {
    id.starts_with("api_")
        && id.len() <= 128
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

#[derive(Debug, Clone, Default)]
pub struct Registry(Arc<Mutex<std::collections::HashMap<String, std::sync::Weak<WireTurn>>>>);
impl Registry {
    pub fn insert(&self, id: String, turn: &Arc<WireTurn>) {
        let mut turns = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        turns.retain(|_, v| v.strong_count() > 0);
        turns.insert(id, Arc::downgrade(turn));
    }
    pub fn get(&self, id: &str) -> Option<Arc<WireTurn>> {
        self.0.lock().ok()?.get(id)?.upgrade()
    }
}

async fn deduplicate(path: &Path, root: &Path) -> std::io::Result<Vec<WireChunk>> {
    let directory = root.join("blobs");
    tokio::fs::create_dir_all(&directory).await?;
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).await?;
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut chunks = Vec::new();
    loop {
        let mut used = 0;
        while used < buffer.len() {
            let n = file.read(&mut buffer[used..]).await?;
            if n == 0 {
                break;
            }
            used += n;
        }
        if used == 0 {
            break;
        }
        let raw = &buffer[..used];
        let hash = format!("{:x}", Sha256::digest(raw));
        let target = directory.join(&hash);
        if !tokio::fs::try_exists(&target).await? {
            let temporary = directory.join(format!(".{}", uuid::Uuid::new_v4()));
            let mut output = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)
                .await?;
            output.write_all(raw).await?;
            output.sync_all().await?;
            drop(output);
            tokio::fs::rename(temporary, target).await?;
        }
        chunks.push(WireChunk {
            sha256: hash,
            bytes: used as u64,
        });
    }
    Ok(chunks)
}

/// Earlier collectors waited for an EOF poll even after the whole declared
/// entity was committed. Correct only that status; never rewrite archived bytes.
fn reconcile_fixed_length(part: &mut WirePart) {
    if part.error.as_deref() != Some("HTTP body did not reach clean EOF") || part.sha256.is_none() {
        return;
    }
    use base64::Engine;
    let expected = part
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
        .and_then(|h| {
            base64::engine::general_purpose::STANDARD
                .decode(&h.value_base64)
                .ok()
        })
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if expected == Some(part.bytes) {
        part.complete = true;
        part.error = None;
    }
}
fn admin(session: &crate::dashboard_auth::AuthSession) -> bool {
    session
        .user
        .as_ref()
        .map_or_else(|| session.bootstrap_admin(), |u| u.is_admin)
}
#[allow(clippy::result_large_err)]
fn capture_root(
    gateway: &crate::engine::Gateway,
    id: &str,
) -> Result<PathBuf, axum::response::Response> {
    use axum::response::IntoResponse;
    if !valid_id(id) {
        return Err(axum::http::StatusCode::BAD_REQUEST.into_response());
    }
    gateway
        .config()
        .wire_capture_dir
        .clone()
        .ok_or_else(|| axum::http::StatusCode::NOT_FOUND.into_response())
}
#[utoipa::path(get,path="/dashboard/api/history/requests/{id}/wire",tag="history",params(("id"=String,Path)),responses((status=200,body=Option<WireManifest>),(status=401),(status=403,description="Raw HTTP captures require an administrator")))]
pub async fn manifest_api(
    axum::extract::State(gateway): axum::extract::State<Arc<crate::engine::Gateway>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    session: crate::dashboard_auth::AuthSession,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !admin(&session) {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    let root = match capture_root(&gateway, &id) {
        Ok(root) => root,
        Err(response) if response.status() == axum::http::StatusCode::NOT_FOUND => {
            return crate::dashboard_auth::no_store(
                axum::Json(Option::<WireManifest>::None).into_response(),
            );
        }
        Err(response) => return response,
    };
    match tokio::fs::read(root.join(&id).join("manifest.json")).await {
        Ok(bytes) => {
            let mut manifest: WireManifest = match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(_) => return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            for part in manifest.parts.values_mut() {
                reconcile_fixed_length(part);
            }
            crate::dashboard_auth::no_store(axum::Json(manifest).into_response())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::dashboard_auth::no_store(
                axum::Json(Option::<WireManifest>::None).into_response(),
            )
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
#[utoipa::path(get,path="/dashboard/api/history/requests/{id}/wire/{part}",tag="history",params(("id"=String,Path),("part"=String,Path)),responses((status=200,body=String,content_type="application/octet-stream",description="Original HTTP entity bytes, concatenated losslessly from stored chunks; no formatting, redaction or capture size ceiling"),(status=401),(status=403),(status=404)))]
pub async fn body_api(
    axum::extract::State(gateway): axum::extract::State<Arc<crate::engine::Gateway>>,
    axum::extract::Path((id, part)): axum::extract::Path<(String, String)>,
    session: crate::dashboard_auth::AuthSession,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !admin(&session) {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    let root = match capture_root(&gateway, &id) {
        Ok(root) => root,
        Err(response) => return response,
    };
    let manifest = match tokio::fs::read(root.join(&id).join("manifest.json"))
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<WireManifest>(&b).ok())
    {
        Some(manifest) => manifest,
        None => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };
    let Some(mut record) = manifest.parts.get(&part).cloned() else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    if record.file != format!("{part}.body")
        || !part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }
    // Open a live spool before handing control to the streaming body. Finalizing
    // a part can publish deduplicated chunks and unlink that spool concurrently.
    let mut opened = None;
    if record.chunks.is_empty() {
        match tokio::fs::File::open(root.join(&id).join(&record.file)).await {
            Ok(file) => opened = Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let latest = tokio::fs::read(root.join(&id).join("manifest.json"))
                    .await
                    .ok()
                    .and_then(|b| serde_json::from_slice::<WireManifest>(&b).ok())
                    .and_then(|m| m.parts.get(&part).cloned());
                match latest {
                    Some(latest) if !latest.chunks.is_empty() => record = latest,
                    _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
                }
            }
            Err(_) => return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
    let mut response = Response::new(Body::from_stream(body_stream_with_file(
        root,
        id,
        record.clone(),
        opened,
    )));
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    response.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{part}.body\"")
            .parse()
            .unwrap(),
    );
    if record.complete {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_LENGTH, record.bytes.into());
    }
    crate::dashboard_auth::no_store(response)
}
#[cfg(test)]
fn body_stream(
    root: PathBuf,
    id: String,
    record: WirePart,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send {
    body_stream_with_file(root, id, record, None)
}
fn body_stream_with_file(
    root: PathBuf,
    id: String,
    record: WirePart,
    mut opened: Option<tokio::fs::File>,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::try_stream! {
        let paths=if record.chunks.is_empty() {vec![(root.join(id).join(&record.file),None)]} else {
            record.chunks.into_iter().map(|chunk|{
                if chunk.sha256.len()!=64||!chunk.sha256.bytes().all(|b|b.is_ascii_hexdigit()){return Err(std::io::Error::other("invalid body chunk"));}
                Ok((root.join("blobs").join(chunk.sha256),Some(chunk.bytes)))
            }).collect::<Result<Vec<_>,std::io::Error>>()?
        };
        for (path,expected) in paths {
            let mut file=if let Some(file)=opened.take() {file} else {tokio::fs::File::open(path).await?};
            if let Some(expected)=expected && file.metadata().await?.len()!=expected {Err(std::io::Error::other("wire chunk length mismatch"))?;}
            let mut buffer=vec![0u8;64*1024];
            loop {let n=file.read(&mut buffer).await?;if n==0 {break;}yield Bytes::copy_from_slice(&buffer[..n]);}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryStreamExt;
    async fn stored(root: &Path, id: &str, key: &str) -> Vec<u8> {
        let manifest: WireManifest = serde_json::from_slice(
            &tokio::fs::read(root.join(id).join("manifest.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        body_stream(root.to_path_buf(), id.into(), manifest.parts[key].clone())
            .try_fold(Vec::new(), |mut all, b| async move {
                all.extend_from_slice(&b);
                Ok(all)
            })
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn exact_capture_preserves_large_bodies_secrets_binary_and_deduplicates() {
        let temp = tempfile::tempdir().unwrap();
        let mut raw = br#" { "api_key" : "unaltered-secret", "input": "#.to_vec();
        raw.extend(vec![b'x'; 17 * 1024 * 1024]);
        raw.extend_from_slice(b"\r\n\0\xfflast-byte");
        let mut values = HeaderMap::new();
        values.insert("authorization", "Bearer exact-header".parse().unwrap());
        let turn = WireTurn::start(
            temp.path(),
            "api_large",
            "POST",
            "/v1/responses",
            &values,
            &raw,
        )
        .await
        .unwrap();
        assert_eq!(stored(temp.path(), "api_large", "client_in").await, raw);
        let manifest = turn.manifest.lock().await.clone();
        assert!(manifest.parts["client_in"].complete);
        assert_eq!(manifest.parts["client_in"].bytes, raw.len() as u64);
        assert_eq!(
            manifest.parts["client_in"].sha256,
            Some(format!("{:x}", Sha256::digest(&raw)))
        );
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&manifest.parts["client_in"].headers[0].value_base64)
                .unwrap(),
            b"Bearer exact-header"
        );
        let mut blobs = tokio::fs::read_dir(temp.path().join("blobs"))
            .await
            .unwrap();
        let mut count = 0;
        while blobs.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert!(
            count < 10,
            "repeated byte chunks share storage, not a cap on the body"
        );
        let copy = WireTurn::start(
            temp.path(),
            "api_second",
            "POST",
            "/v1/responses",
            &values,
            &raw,
        )
        .await
        .unwrap();
        assert_eq!(
            copy.manifest.lock().await.parts["client_in"].chunks,
            manifest.parts["client_in"].chunks
        );
        let encoded = serde_json::to_vec(&manifest).unwrap();
        assert_eq!(
            serde_json::from_slice::<WireManifest>(&encoded).unwrap(),
            manifest
        );
    }
    #[tokio::test]
    async fn every_upstream_send_is_kept_and_response_drains_after_consumer_drop() {
        let temp = tempfile::tempdir().unwrap();
        let turn = WireTurn::start(
            temp.path(),
            "api_retries",
            "POST",
            "/v1/responses",
            &HeaderMap::new(),
            b"{}",
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        for (index, raw) in [b" {\"attempt\": 1}\r\n".as_slice(), b"{ \"attempt\":2 }\n"]
            .into_iter()
            .enumerate()
        {
            let request = client
                .post("http://localhost/v1/chat/completions")
                .body(raw.to_vec())
                .build()
                .unwrap();
            let (key, _) = turn.upstream(&request).await.unwrap();
            assert_eq!(key, format!("upstream_{:06}", index + 1));
            assert_eq!(
                stored(temp.path(), "api_retries", &(key.clone() + "_out")).await,
                raw
            );
            let response = reqwest::Response::from(
                Response::builder()
                    .status(500)
                    .body(reqwest::Body::from(
                        b"data: {\"secret\":\"keep\"}\r\n\r\ndata: [DONE]\r\n\r\n".to_vec(),
                    ))
                    .unwrap(),
            );
            let response = upstream_response(response, Arc::clone(&turn), key.clone())
                .await
                .unwrap();
            drop(response);
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if turn.manifest.lock().await.parts[&(key.clone() + "_in")].complete {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                stored(temp.path(), "api_retries", &(key + "_in")).await,
                b"data: {\"secret\":\"keep\"}\r\n\r\ndata: [DONE]\r\n\r\n"
            );
        }
        assert_eq!(turn.manifest.lock().await.parts.len(), 5);
        let raw = b"\xef\xbb\xbfdata: {\"choices\":[]}\r\n\r\n: keepalive\n\ndata: [DONE]\n\n";
        let response =
            served_response(Response::new(Body::from(raw.as_slice())), Arc::clone(&turn))
                .await
                .unwrap();
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
            raw.as_slice()
        );
        assert_eq!(stored(temp.path(), "api_retries", "client_out").await, raw);
    }
    #[tokio::test]
    async fn unpolled_client_response_is_closed_as_incomplete() {
        let temp = tempfile::tempdir().unwrap();
        let turn = WireTurn::start(
            temp.path(),
            "api_cancel",
            "POST",
            "/v1/responses",
            &HeaderMap::new(),
            b"{}",
        )
        .await
        .unwrap();
        let response = served_response(
            Response::new(Body::from("not delivered")),
            Arc::clone(&turn),
        )
        .await
        .unwrap();
        drop(response);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if turn.manifest.lock().await.parts["client_out"]
                    .error
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let part = turn.manifest.lock().await.parts["client_out"].clone();
        assert!(!part.complete);
        assert_eq!(part.bytes, 0);
    }
    #[tokio::test]
    async fn fixed_length_entity_is_complete_without_an_eof_poll() {
        let temp = tempfile::tempdir().unwrap();
        let turn = WireTurn::start(
            temp.path(),
            "api_length",
            "POST",
            "/v1/responses",
            &HeaderMap::new(),
            b"{}",
        )
        .await
        .unwrap();
        let response = served_response(
            Response::new(Body::from("complete body")),
            Arc::clone(&turn),
        )
        .await
        .unwrap();
        let mut body = response.into_body();
        let frame = body.frame().await.unwrap().unwrap();
        assert_eq!(frame.data_ref().unwrap().as_ref(), b"complete body");
        drop(body);
        let part = turn.manifest.lock().await.parts["client_out"].clone();
        assert!(part.complete);
        assert!(part.error.is_none());
        assert_eq!(
            stored(temp.path(), "api_length", "client_out").await,
            b"complete body"
        );
    }
    #[test]
    fn older_fixed_length_status_is_corrected_only_with_a_complete_entity() {
        let mut values = HeaderMap::new();
        values.insert("content-length", "10".parse().unwrap());
        let mut part = WirePart {
            file: "client_out.body".into(),
            chunks: vec![],
            headers: headers(&values),
            method: None,
            target: None,
            status: Some(200),
            bytes: 9,
            sha256: Some("committed-hash".into()),
            complete: false,
            error: Some("HTTP body did not reach clean EOF".into()),
        };
        reconcile_fixed_length(&mut part);
        assert!(!part.complete);
        part.bytes = 10;
        reconcile_fixed_length(&mut part);
        assert!(part.complete);
        assert!(part.error.is_none());
        part.complete = false;
        part.error = Some("storage failure".into());
        reconcile_fixed_length(&mut part);
        assert!(!part.complete);
    }
    #[test]
    fn wire_capture_configuration_round_trips() {
        let value = serde_json::json!({"wire_capture_dir":"/data/exact-http"});
        let config: crate::config::PersistedConfig = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&config).unwrap()["wire_capture_dir"],
            value["wire_capture_dir"]
        );
        assert_eq!(
            crate::config::Config::from_persisted(&config)
                .unwrap()
                .wire_capture_dir,
            Some(PathBuf::from("/data/exact-http"))
        );
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    #[tokio::test]
    async fn all_four_captures_equal_actual_http_bodies_before_parsing_or_serialization() {
        let directory = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data":[{"id":"served-model"}]})),
            )
            .mount(&server)
            .await;
        let upstream = "data: { \"id\":\"chat-wire\", \"choices\":[{\"index\":0,\"delta\":{\"content\":\"wire content\"},\"finish_reason\":null}] }\r\n\r\ndata: {\"id\":\"chat-wire\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(upstream),
            )
            .mount(&server)
            .await;
        let config = crate::config::Config::from_persisted(&crate::config::PersistedConfig {
            upstream_base_url: format!("{}/v1", server.uri()),
            wire_capture_dir: Some(directory.path().to_string_lossy().into_owned()),
            ..Default::default()
        })
        .unwrap();
        let (app, gateway) = crate::build_app_with_gateway(config);
        let inbound=b" { \"model\" : \"served-model\", \"messages\" : [{\"role\":\"user\",\"content\":\"hello \\u0061\"}], \"stream\":true, \"api_key\":\"preserve-body-secret\" }\r\n";
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer preserve-client-header")
                    .body(Body::from(inbound.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let client_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
        let mut id = None;
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().to_string();
            if valid_id(&name) {
                id = Some(name);
            }
        }
        let id = id.unwrap();
        let manifest: WireManifest = serde_json::from_slice(
            &tokio::fs::read(directory.path().join(&id).join("manifest.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.parts.len(), 4);
        for (key, expected) in [
            ("client_in", inbound.as_slice()),
            ("upstream_000001_in", upstream.as_bytes()),
            ("client_out", client_body.as_ref()),
        ] {
            use futures::TryStreamExt;
            let actual = body_stream(
                directory.path().to_path_buf(),
                id.clone(),
                manifest.parts[key].clone(),
            )
            .try_fold(Vec::new(), |mut bytes, b| async move {
                bytes.extend_from_slice(&b);
                Ok(bytes)
            })
            .await
            .unwrap();
            assert_eq!(actual, expected, "exact body {key}");
            assert!(manifest.parts[key].complete);
        }
        let outgoing = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method.as_str() == "POST")
            .unwrap()
            .body;
        use futures::TryStreamExt;
        let actual = body_stream(
            directory.path().to_path_buf(),
            id.clone(),
            manifest.parts["upstream_000001_out"].clone(),
        )
        .try_fold(Vec::new(), |mut bytes, b| async move {
            bytes.extend_from_slice(&b);
            Ok(bytes)
        })
        .await
        .unwrap();
        assert_eq!(actual, outgoing);
        assert!(
            String::from_utf8(actual)
                .unwrap()
                .contains("preserve-body-secret")
        );
        let denied = manifest_api(
            axum::extract::State(Arc::clone(&gateway)),
            axum::extract::Path(id.clone()),
            crate::dashboard_auth::AuthSession::delegated(u64::MAX),
        )
        .await;
        assert_eq!(denied.status(), 403);
        let session = crate::dashboard_auth::AuthSession {
            exp: u64::MAX,
            user: None,
            kind: crate::dashboard_auth::AuthSessionKind::DashboardToken,
        };
        let response = body_api(
            axum::extract::State(Arc::clone(&gateway)),
            axum::extract::Path((id.clone(), "client_in".into())),
            session,
        )
        .await;
        assert_eq!(response.status(), 200);
        assert!(
            response.headers()["cache-control"]
                .to_str()
                .unwrap()
                .contains("no-store")
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
            inbound.as_slice()
        );
        // The ingress tee is before decompression: preserve the compressed
        // entity, not a freshly serialized JSON representation of its contents.
        let encoded = zstd::stream::encode_all(inbound.as_slice(), 1).unwrap();
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .header("content-encoding", "zstd")
                    .body(Body::from(encoded.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
        let mut found = false;
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !valid_id(&name) || name == id {
                continue;
            }
            let compressed: WireManifest = serde_json::from_slice(
                &tokio::fs::read(entry.path().join("manifest.json"))
                    .await
                    .unwrap(),
            )
            .unwrap();
            let actual = body_stream(
                directory.path().to_path_buf(),
                name,
                compressed.parts["client_in"].clone(),
            )
            .try_fold(Vec::new(), |mut all, bytes| async move {
                all.extend_from_slice(&bytes);
                Ok(all)
            })
            .await
            .unwrap();
            assert_eq!(actual, encoded);
            found = true;
        }
        assert!(found, "compressed request retained");
    }
}

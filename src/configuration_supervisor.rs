//! One service process at a time: drain, restart, probe this exact revision, commit
//! or restore the previous revision. The original SQL store remains the settings owner.
use crate::{
    control_plane_store::PersistenceStore,
    dashboard_settings::{self as settings, Settings, Specification},
};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};
async fn private_write(path: PathBuf, bytes: Vec<u8>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let temp = path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|_| "Settings directory is not writable")?;
        f.write_all(&bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| "Could not persist runtime settings")?;
        std::fs::rename(temp, path).map_err(|_| "Could not install runtime settings")
    })
    .await
    .map_err(|_| "Settings writer failed")?
    .map_err(str::to_string)
}
fn command(spec: &Specification, backend: &str, url: &str) -> Result<Command, String> {
    let mut cmd =
        Command::new(std::env::current_exe().map_err(|_| "Cannot locate gateway binary")?);
    for (name, _) in std::env::vars() {
        if settings::managed_env(&name)
            || spec.environment.contains_key(&name)
            || settings::referenced_environment(&spec.document).contains(&name)
            || name.starts_with(settings::INTERNAL_PREFIX)
        {
            cmd.env_remove(name);
        }
    }
    cmd.envs(&spec.environment)
        .env("LLMCONDUIT_MANAGED_STORE_BACKEND", backend)
        .env("LLMCONDUIT_MANAGED_STORE_URL", url)
        .kill_on_drop(true);
    Ok(cmd)
}
fn bind(spec: &Specification) -> Result<std::net::SocketAddr, String> {
    spec.environment
        .get("LLMCONDUIT_BIND_ADDR")
        .map(String::as_str)
        .or_else(|| {
            spec.document
                .get("bind_addr")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("127.0.0.1:8080")
        .parse()
        .map_err(|_| "Invalid gateway bind address".to_string())
}
async fn runtime_path(spec: &Specification, dir: &Path) -> Result<PathBuf, String> {
    let path = dir.join("runtime.yaml");
    private_write(
        path.clone(),
        serde_yaml::to_string(&spec.document)
            .map_err(|_| "Invalid configuration")?
            .into_bytes(),
    )
    .await?;
    Ok(path)
}
pub async fn preflight(
    spec: &Specification,
    dir: &Path,
    backend: &str,
    url: &str,
) -> Result<(), String> {
    settings::validate(spec)?;
    let path = dir.join(format!("validate-{}.yaml", uuid::Uuid::new_v4().simple()));
    private_write(
        path.clone(),
        serde_yaml::to_string(&spec.document)
            .map_err(|_| "Invalid configuration")?
            .into_bytes(),
    )
    .await?;
    let status = command(spec, backend, url)?
        .args(["check-config", "--config"])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|_| "Could not run configuration validation")?;
    let _ = tokio::fs::remove_file(path).await;
    if !status.success() {
        return Err(
            "Runtime configuration failed validation; review values and environment overrides"
                .into(),
        );
    }
    Ok(())
}
async fn launch(
    spec: &Specification,
    revision: u64,
    dir: &Path,
    backend: &str,
    url: &str,
    debug: bool,
) -> Result<(Child, String), String> {
    let path = runtime_path(spec, dir).await?;
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut cmd = command(spec, backend, url)?;
    cmd.args(["start", "--config"])
        .arg(path)
        .env("LLMCONDUIT_MANAGED_REVISION", revision.to_string())
        .env("LLMCONDUIT_MANAGED_READY_TOKEN", &token)
        .env("LLMCONDUIT_MANAGED_STATE_DIR", dir)
        .env(
            "LLMCONDUIT_MANAGED_REQUIRE_DASHBOARD",
            if debug { "true" } else { "false" },
        );
    if debug {
        cmd.arg("--with-debug-ui");
    }
    Ok((cmd.spawn().map_err(|_| "Could not start gateway")?, token))
}
async fn ready(
    child: &mut Child,
    spec: &Specification,
    token: &str,
    revision: u64,
    required_hosts: &[String],
) -> bool {
    let Ok(mut address) = bind(spec) else {
        return false;
    };
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        })
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .no_proxy()
        .build()
        .expect("Readiness client");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    while tokio::time::Instant::now() < deadline {
        if !matches!(child.try_wait(), Ok(None)) {
            return false;
        }
        if let Ok(response) = client
            .get(format!("http://{address}/internal/configuration-ready"))
            .bearer_auth(token)
            .send()
            .await
            && response.status().is_success()
            && let Ok(value) = response.json::<serde_json::Value>().await
        {
            let hosts = value["connected_hosts"].as_array();
            if value["revision"].as_str() == Some(revision.to_string().as_str())
                && required_hosts.iter().all(|id| {
                    hosts.is_some_and(|items| items.iter().any(|v| v.as_str() == Some(id)))
                })
            {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    false
}
async fn connected_hosts(spec: &Specification, token: &str) -> Vec<String> {
    let Ok(mut address) = bind(spec) else {
        return vec![];
    };
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        })
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("Readiness client");
    if let Ok(response) = client
        .get(format!("http://{address}/internal/configuration-ready"))
        .bearer_auth(token)
        .send()
        .await
        && let Ok(value) = response.json::<serde_json::Value>().await
    {
        return value["connected_hosts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
    }
    vec![]
}

async fn stop(child: &mut Child) {
    if let Some(pid) = child.id() {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        } // SIGTERM invokes the gateway's existing graceful stream drain.
        unsafe {
            kill(pid as i32, 15);
        }
        if tokio::time::timeout(Duration::from_secs(35), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }
}
/// Optional deployment-operator mount lets Traefik follow managed listener changes.
/// A complete file is atomically replaced only after candidate readiness succeeds.
async fn publish_ingress(spec: &Specification) -> Result<(), String> {
    if let Ok(path) = std::env::var("LLMCONDUIT_MANAGED_INGRESS_FILE") {
        let address = bind(spec)?;
        let upstream = format!("http://llmconduit:{}", address.port());
        let document = serde_json::json!({"http":{"services":{"managed-gateway":{"loadBalancer":{"servers":[{"url":upstream}]}}}}});
        let ingress_path = PathBuf::from(path);
        private_write(
            ingress_path.clone(),
            serde_yaml::to_string(&document)
                .map_err(|_| "Invalid ingress")?
                .into_bytes(),
        )
        .await?;
        // Contains only an internal upstream URL; Traefik runs as another UID.
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(ingress_path, std::fs::Permissions::from_mode(0o644))
            .await
            .map_err(|_| "Could not publish readable ingress configuration")?;
    }
    Ok(())
}
fn storage(spec: &Specification) -> (String, Option<String>) {
    let backend = spec
        .environment
        .get("LLMCONDUIT_STORAGE_BACKEND")
        .map(String::as_str)
        .or_else(|| {
            spec.document
                .pointer("/control_plane/storage/backend")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("none")
        .to_owned();
    let url = spec
        .environment
        .get("LLMCONDUIT_DATABASE_URL")
        .cloned()
        .or_else(|| {
            spec.document
                .pointer("/control_plane/storage/url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    (backend, url)
}
async fn migrate_storage(old: &Specification, new: &Specification) -> Result<(), String> {
    let from = storage(old);
    let to = storage(new);
    if from == to
        || (from.1.is_some() && from.1 == to.1 && matches!(to.0.as_str(), "sqlite" | "postgres"))
        || !matches!(to.0.as_str(), "sqlite" | "postgres")
    {
        return Ok(());
    }
    if !matches!(from.0.as_str(), "sqlite" | "postgres") {
        return Err("Switching back to SQL requires an existing source database; preserve the source connection when disabling history".into());
    }
    let source = settings::connect(
        &from.0,
        from.1.as_deref().ok_or("Source database URL is missing")?,
    )
    .await?;
    let destination = settings::connect(
        &to.0,
        to.1.as_deref()
            .ok_or("Destination database URL is missing")?,
    )
    .await?;
    source.copy_into_empty(&destination).await.map_err(|_|"Storage migration failed. Destination must be empty and reachable; source data is retained".to_string())
}

async fn migrate_mesh_state(old: &Specification, new: &Specification) -> Result<(), String> {
    for key in ["identity_path", "state_path"] {
        let path = format!("/mesh/controller/{key}");
        if let (Some(source), Some(destination)) = (
            old.document
                .pointer(&path)
                .and_then(serde_json::Value::as_str),
            new.document
                .pointer(&path)
                .and_then(serde_json::Value::as_str),
        ) {
            if source == destination {
                continue;
            }
            let source = PathBuf::from(source);
            let destination = PathBuf::from(destination);
            if key == "state_path" {
                tokio::task::spawn_blocking(move || {
                    crate::authz::store::AuthStore::backup_into_new(&source, &destination)
                })
                .await
                .map_err(|_| "Mesh state migration failed")??;
            } else {
                let bytes = tokio::fs::read(&source)
                    .await
                    .map_err(|_| "Could not read controller identity")?;
                if let Ok(existing) = tokio::fs::read(&destination).await {
                    if existing != bytes {
                        return Err("Destination contains a different controller identity".into());
                    }
                } else {
                    private_write(destination, bytes).await?;
                }
            }
        }
    }
    Ok(())
}

async fn migrate_auth_store(old: &Specification, new: &Specification) -> Result<(), String> {
    let old = old
        .document
        .pointer("/auth/store_path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    let new = new
        .document
        .pointer("/auth/store_path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    if let (Some(old), Some(new)) = (old, new)
        && old != new
        && old.is_file()
    {
        tokio::task::spawn_blocking(move || {
            crate::authz::store::AuthStore::backup_into_new(&old, &new)
        })
        .await
        .map_err(|_| "Policy store copy failed")??;
    }
    Ok(())
}

pub async fn run(
    backend: &str,
    url: &str,
    bootstrap: Specification,
    dir: PathBuf,
    debug: bool,
) -> Result<(), String> {
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|_| "Could not create settings directory")?;
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| "Could not secure settings directory")?;
    }
    // Install handlers before the first child becomes reachable: a shutdown
    // during startup must be queued for graceful child cleanup, not orphan it.
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| "Could not watch shutdown")?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "Could not watch shutdown")?;
    let store = settings::connect(backend, url).await?;
    if store.get_setting(settings::KEY).await?.is_none() {
        settings::validate(&bootstrap)?;
        let state = Settings {
            revision: 0,
            applied_revision: 0,
            status: "applied".into(),
            pending: false,
            draft: bootstrap.clone(),
            active: bootstrap,
            error: None,
        };
        settings::write(store.as_ref(), None, &state).await?;
    }
    let (_, initial) = settings::read(store.as_ref()).await?;
    preflight(&initial.active, &dir, backend, url).await?;
    let (mut child, mut current_token) = launch(
        &initial.active,
        initial.applied_revision,
        &dir,
        backend,
        url,
        debug,
    )
    .await?;
    if !ready(
        &mut child,
        &initial.active,
        &current_token,
        initial.applied_revision,
        &[],
    )
    .await
    {
        stop(&mut child).await;
        return Err("Gateway failed readiness with last applied configuration".into());
    }
    publish_ingress(&initial.active).await?;
    let mut timer = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {_=term.recv()=>{stop(&mut child).await;return Ok(())},_=interrupt.recv()=>{stop(&mut child).await;return Ok(())},_=timer.tick()=>{}}
        let (raw, mut state) = settings::read(store.as_ref()).await?;
        if state.pending {
            let result = preflight(&state.draft, &dir, backend, url).await;
            let mut failure = result.err();
            if failure.is_none() {
                let required_hosts = if state
                    .draft
                    .document
                    .pointer("/mesh/controller/enabled")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                {
                    connected_hosts(&state.active, &current_token).await
                } else {
                    vec![]
                };
                stop(&mut child).await;
                failure = migrate_storage(&state.active, &state.draft).await.err();
                if failure.is_none() {
                    failure = migrate_auth_store(&state.active, &state.draft).await.err();
                }
                if failure.is_none() {
                    failure = migrate_mesh_state(&state.active, &state.draft).await.err();
                }
                if failure.is_none() {
                    match launch(&state.draft, state.revision, &dir, backend, url, debug).await {
                        Ok((mut candidate, token)) => {
                            if ready(
                                &mut candidate,
                                &state.draft,
                                &token,
                                state.revision,
                                &required_hosts,
                            )
                            .await
                            {
                                if let Err(e) = publish_ingress(&state.draft).await {
                                    stop(&mut candidate).await;
                                    failure = Some(e)
                                } else {
                                    current_token = token;
                                    child = candidate;
                                    state.active = state.draft.clone();
                                    state.applied_revision = state.revision;
                                }
                            } else {
                                stop(&mut candidate).await;
                                failure = Some(
                                    "Gateway failed readiness; previous configuration restored"
                                        .into(),
                                )
                            }
                        }
                        Err(e) => failure = Some(e),
                    }
                }
                if failure.is_some() {
                    let (restored, token) = launch(
                        &state.active,
                        state.applied_revision,
                        &dir,
                        backend,
                        url,
                        debug,
                    )
                    .await?;
                    child = restored;
                    if !ready(
                        &mut child,
                        &state.active,
                        &token,
                        state.applied_revision,
                        &[],
                    )
                    .await
                    {
                        return Err("Rollback failed readiness".into());
                    }
                    current_token = token;
                    publish_ingress(&state.active).await?;
                }
            }
            state.pending = false;
            state.status = if failure.is_some() {
                "rolled_back"
            } else {
                "applied"
            }
            .into();
            state.error = failure;
            if !settings::write(store.as_ref(), Some(&raw), &state).await? {
                return Err("Configuration changed during apply".into());
            }
        } else if child
            .try_wait()
            .map_err(|_| "Could not inspect gateway")?
            .is_some()
        {
            tracing::warn!("Restarting exited gateway using last applied settings");
            let (replacement, token) = launch(
                &state.active,
                state.applied_revision,
                &dir,
                backend,
                url,
                debug,
            )
            .await?;
            child = replacement;
            if !ready(
                &mut child,
                &state.active,
                &token,
                state.applied_revision,
                &[],
            )
            .await
            {
                return Err("Gateway restart failed readiness".into());
            }
            current_token = token;
        }
    }
}

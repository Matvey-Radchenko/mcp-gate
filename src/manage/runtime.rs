use super::{launch, store::Record};
use crate::{
    clients::{Binding, Candidate, Client},
    config::Config,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?)
}
pub async fn health(record: &Record) -> Result<Value> {
    let c = Config::load(&record.config())?;
    Ok(http()?
        .get(format!("http://{}/health", c.listen))
        .bearer_auth(c.token()?)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
pub async fn maintenance(record: &Record, enable: bool) -> Result<bool> {
    let c = Config::load(&record.config())?;
    let method = if enable {
        reqwest::Method::POST
    } else {
        reqwest::Method::DELETE
    };
    let response = http()?
        .request(method, format!("http://{}/admin/maintenance", c.listen))
        .bearer_auth(c.token()?)
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::CONFLICT {
        return Ok(false);
    }
    response.error_for_status()?;
    Ok(true)
}

pub async fn prepare(root: &Path, candidate: &Candidate) -> Result<Record> {
    let id = uuid::Uuid::new_v4().to_string();
    let release = root.join("releases").join(&id);
    crate::platform::private_dir(&release)?;
    for name in ["bin", "state", "credentials", "logs"] {
        crate::platform::private_dir(&release.join(name))?;
    }
    let binary = super::upgrade::binary(root)?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let environment = launch::environment(candidate)?;
    let command = launch::executable(candidate, &environment)?;
    let mode = candidate
        .recipe
        .as_ref()
        .map_or("session", |r| r.mode.as_str());
    let config = json!({"format_version":2,"ownership":mode,"max_workers":if mode == "shared" { 1 } else { 4 },"shared_client_roots":if mode == "shared" { "ignore" } else { "reject" },"listen":format!("127.0.0.1:{port}"),
        "token_file":release.join("client-token"),"catalog_file":release.join("catalog.json"),"state_dir":release.join("state"),
        "backend":{"profile":"stdio","command":command,"docker": command.file_stem().is_some_and(|s|s == "docker"),"args":&candidate.command[1..],"version":"discovery-pending",
        "working_directory":candidate.cwd,"env":environment,"env_files":candidate.env_files,"inherit_env":candidate.inherit_env}});
    let mut config: Config = serde_json::from_value(config)?;
    if let Some(entry) = crate::clients::recipes::entrypoint(&candidate.command, &candidate.cwd)
        && entry != config.backend.command
    {
        let index = candidate
            .command
            .iter()
            .position(|arg| candidate.cwd.join(arg) == entry)
            .context("Cannot retain entrypoint argument order")?;
        config.backend.command_args = candidate.command[1..index].to_vec();
        config.backend.entrypoint = Some(entry);
        config.backend.args = candidate.command[index + 1..].to_vec();
    }
    if let Some(recipe) = &candidate.recipe {
        config.tool_policy.disabled = recipe.disabled_tools.clone();
        config.backend.directory_env = recipe.directory_env.clone();
        config.backend.working_directory_env = recipe.working_directory_env.clone();
    }
    config.validate()?;
    crate::install::init_token(&config.token_file)?;
    let catalog = crate::install::discover_and_pin(&mut config).await.map_err(|_| anyhow::anyhow!("Backend discovery failed; check the command, dependencies, context and exclusive resource ownership. Values hidden."))?;
    if let Some(recipe) = &candidate.recipe {
        ensure!(
            config.backend.version == recipe.server_version,
            "Backend serverInfo.version does not match the proposed recipe; no client settings changed"
        );
    }
    super::store::atomic(&config.catalog_file, &serde_json::to_vec_pretty(&catalog)?)?;
    super::store::atomic(
        &release.join("config.toml"),
        toml::to_string_pretty(&config)?.as_bytes(),
    )?;
    let mut binding = candidate.binding.clone();
    binding.after = remote(&binding, &binary, &release.join("config.toml"), &config)?;
    Ok(Record {
        removing: false,
        id: id.clone(),
        release,
        label: format!("local.mcp-gate.{id}"),
        binary_hash: crate::catalog::digest(&binary)?,
        binary,
        bindings: vec![binding],
    })
}

fn quote(path: &Path) -> String {
    if cfg!(windows) {
        format!("\"{}\"", path.display())
    } else {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }
}
pub fn headers_helper(binary: &Path, config: &Path) -> String {
    format!("{} headers --config {}", quote(binary), quote(config))
}
fn remote(binding: &Binding, binary: &Path, path: &Path, config: &Config) -> Result<Value> {
    let mut value = binding.direct.clone();
    let object = value
        .as_object_mut()
        .context("MCP configuration must be an object")?;
    for key in ["command", "args", "env", "environment", "env_vars", "cwd"] {
        object.remove(key);
    }
    object.insert("url".into(), json!(format!("http://{}/mcp", config.listen)));
    let helper = headers_helper(binary, path);
    match binding.client {
        Client::Codex => {
            object.insert("http_headers_helper".into(), json!(helper));
        }
        Client::ClaudeCode => {
            object.insert("type".into(), json!("http"));
            object.insert("headersHelper".into(), json!(helper));
        }
        Client::Opencode => {
            object.insert("type".into(), json!("remote"));
            object.insert("oauth".into(), json!(false));
            let auth = config.token_file.with_file_name("authorization");
            super::store::atomic(&auth, format!("Bearer {}", config.token()?).as_bytes())?;
            object.insert(
                "headers".into(),
                json!({"Authorization":format!("{{file:{}}}",auth.display())}),
            );
        }
    }
    Ok(value)
}
/// A replaced service may hold its state lock before it listens. Recovery runs
/// at `upgrade-registered`, before `ready`, so both use this startup window.
const SERVICE_HANDOFF: Duration = Duration::from_secs(30);

pub async fn ready(record: &Record) -> Result<()> {
    // Bound elapsed time, not retries: refused connections on Windows can take
    // seconds each, and multiplying those delays would stall safe recovery.
    let wait = async {
        loop {
            if let Ok(value) = health(record).await {
                ensure!(
                    value["workers"] == 0,
                    "New gateway unexpectedly started a backend"
                );
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    if let Ok(result) = tokio::time::timeout(SERVICE_HANDOFF, wait).await {
        return result;
    }
    anyhow::bail!(
        "Gateway did not become healthy: {}. Inspect its private service logs",
        super::service::diagnostics(record)
    )
}

pub fn reuse(registry: &super::store::Registry, candidate: &Candidate) -> Result<Option<Record>> {
    if candidate.recipe.as_ref().is_none_or(|r| r.mode != "shared") {
        return Ok(None);
    }
    for record in &registry.gateways {
        if record.removing
            || !record
                .bindings
                .iter()
                .any(|b| b.direct == candidate.binding.direct)
        {
            continue;
        }
        let config = Config::load(&record.config())?;
        if config.ownership != crate::config::Ownership::Shared
            || config.backend.env != launch::environment(candidate)?
            || config.backend.working_directory.as_ref() != Some(&candidate.cwd)
            || config.backend.env_files != candidate.env_files
            || config.backend.inherit_env != candidate.inherit_env
        {
            continue;
        }
        crate::catalog::Catalog::load(&config.catalog_file, &config.backend)?;
        let mut binding = candidate.binding.clone();
        binding.after = remote(&binding, &record.binary, &record.config(), &config)?;
        let mut reused = record.clone();
        reused.bindings = vec![binding];
        return Ok(Some(reused));
    }
    Ok(None)
}

/// A live maintenance barrier or the daemon's exclusive state lock prevents a
/// new owner from accepting work while its service registration is removed.
pub async fn stop_idle(record: &Record) -> Result<bool> {
    let config = Config::load(&record.config())?;
    let offline = match hold_for_stop(record, &config).await? {
        StopHold::Busy => return Ok(false),
        StopHold::Paused => None,
        StopHold::Offline(lock) => Some(lock),
    };
    let result = super::service::unregister(record);
    if result.is_err() {
        let _ = maintenance(record, false).await;
    }
    drop(offline);
    result?;
    await_state_release(&config).await
}

enum StopHold {
    /// HTTP 409, or still unreachable after the handoff bound.
    Busy,
    /// Maintenance is held in the running process.
    Paused,
    /// This task holds the state lock; no gateway process does.
    Offline(std::fs::File),
}

/// Pause an idle gateway, or take its state lock when no process owns it.
/// A starting process takes the lock before it listens. One refused health
/// request is not evidence that the gateway is busy.
async fn hold_for_stop(record: &Record, config: &Config) -> Result<StopHold> {
    use fs2::FileExt;
    let deadline = tokio::time::Instant::now() + SERVICE_HANDOFF;
    loop {
        match maintenance(record, true).await {
            Ok(true) => return Ok(StopHold::Paused),
            Ok(false) => return Ok(StopHold::Busy),
            Err(_) => {
                let lock = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(config.state_dir.join("gateway.lock"))?;
                if lock.try_lock_exclusive().is_ok() {
                    return Ok(StopHold::Offline(lock));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Ok(StopHold::Busy);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn await_state_release(config: &Config) -> Result<bool> {
    use fs2::FileExt;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(config.state_dir.join("gateway.lock"))?;
    for _ in 0..400 {
        if lock.try_lock_exclusive().is_ok() {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!(
        "Service stop requested but its process still owns the state directory; replacement deferred"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;

    #[tokio::test]
    async fn a_starting_process_lock_is_not_a_busy_gateway() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let token = root.path().join("token");
        std::fs::write(&token, "ab".repeat(32)).unwrap();
        crate::platform::private_permissions(&token).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let release = root.path().join("release");
        std::fs::create_dir_all(&release).unwrap();
        let config_path = release.join("config.toml");
        let config: Config = serde_json::from_value(json!({
            "format_version": 2,
            "listen": format!("127.0.0.1:{port}"),
            "token_file": &token,
            "catalog_file": root.path().join("catalog.json"),
            "state_dir": &state,
            "backend": {"profile": "stdio", "command": &token, "version": "fixture"}
        }))
        .unwrap();
        std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
        let config = Config::load(&config_path).unwrap();
        let record = Record {
            removing: false,
            id: "fixture".into(),
            release,
            label: "local.mcp-gate.fixture".into(),
            binary_hash: "0".repeat(64),
            binary: token,
            bindings: Vec::new(),
        };
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state.join("gateway.lock"))
            .unwrap();
        held.try_lock_exclusive().unwrap();
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let hold = hold_for_stop(&record, &config).await.unwrap();
        assert!(
            matches!(hold, StopHold::Offline(_)),
            "a lock released during startup must not be reported as busy"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

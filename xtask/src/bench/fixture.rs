//! Prepare an isolated gateway with the integration tests' catalog/serve protocol.
use super::{
    Mode, TIMEOUT,
    binaries::Binaries,
    process::{Signals, command, finish, terminate},
};
use anyhow::{Context, Result, bail, ensure};
use reqwest::Client;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::TcpListener,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    process::Child,
    time::{sleep, timeout},
};
use uuid::Uuid;

pub(super) struct Fixture<'a> {
    pub(super) binaries: &'a Binaries,
    pub(super) directory: TempDir,
    pub(super) gateway: Option<Gateway>,
    pub(super) concurrency: usize,
}

pub(super) struct Gateway {
    pub(super) base: String,
    pub(super) config: PathBuf,
    pub(super) token: String,
}

impl<'a> Fixture<'a> {
    pub(super) async fn prepare(
        binaries: &'a Binaries,
        mode: Mode,
        concurrency: usize,
        signals: &mut Signals,
    ) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("mcp-gate-bench-")
            .tempdir()?;
        let gateway = async {
            if matches!(mode, Mode::Direct) {
                return Ok(None);
            }
            let gateway = Gateway::configure(directory.path(), binaries, mode, concurrency)?;
            gateway.catalog(binaries, directory.path(), signals).await?;
            Ok(Some(gateway))
        }
        .await;
        match gateway {
            Ok(gateway) => Ok(Self {
                binaries,
                directory,
                gateway,
                concurrency,
            }),
            Err(error) => finish(Err(error), remove(directory)),
        }
    }

    pub(super) fn close(self) -> Result<()> {
        remove(self.directory)
    }
}

fn remove(directory: TempDir) -> Result<()> {
    directory
        .close()
        .context("Cannot remove benchmark directory")
}

impl Gateway {
    fn configure(root: &Path, binaries: &Binaries, mode: Mode, concurrency: usize) -> Result<Self> {
        let listen = TcpListener::bind("127.0.0.1:0")
            .context("Cannot reserve a temporary IPv4 loopback port")?
            .local_addr()?;
        let token_path = root.join("token");
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&token_path)?
            .write_all(token.as_bytes())?;
        let config = root.join("config.toml");
        let settings = json!({
            "format_version": 2, "listen": listen,
            "token_file": token_path, "catalog_file": root.join("catalog.json"),
            "state_dir": root.join("state"), "ownership": mode.name(), "idle_seconds": 0,
            "max_workers": if matches!(mode, Mode::Shared) { 1 } else { concurrency },
            "max_sessions": concurrency, "max_pending_calls": concurrency,
            "backend": {
                "profile": "stdio", "command": binaries.backend, "version": "mock-1",
                "working_directory": root
            }
        });
        fs::write(&config, toml::to_string(&settings)?)?;
        Ok(Self {
            base: format!("http://{listen}"),
            config,
            token,
        })
    }

    async fn catalog(&self, binaries: &Binaries, root: &Path, signals: &mut Signals) -> Result<()> {
        let mut child = command(&binaries.gateway, root)
            .arg("catalog")
            .arg("--config")
            .arg(&self.config)
            .spawn()
            .context("Cannot start catalog probe")?;
        tokio::select! {
            biased;
            () = signals.interrupted() => {
                finish(Err(anyhow::anyhow!("Benchmark interrupted")), terminate(&mut child).await)
            }
            result = timeout(TIMEOUT, child.wait()) => match result {
                Ok(status) => {
                    ensure!(status?.success(), "Catalog probe failed");
                    Ok(())
                }
                Err(error) => finish(Err(error).context("Catalog probe timed out"), terminate(&mut child).await),
            }
        }
    }

    pub(super) async fn ready(&self, http: &Client, child: &mut Child) -> Result<()> {
        let url = format!("{}/health", self.base);
        timeout(TIMEOUT, async {
            loop {
                if let Some(status) = child.try_wait()? {
                    bail!("Gateway exited during startup: {status}");
                }
                match http.get(&url).bearer_auth(&self.token).send().await {
                    Ok(response) => {
                        ensure!(
                            response.status().is_success(),
                            "Gateway readiness returned {}",
                            response.status()
                        );
                        response.bytes().await?;
                        return Ok(());
                    }
                    Err(error) if error.is_connect() => sleep(Duration::from_millis(1)).await,
                    Err(error) => return Err(error.into()),
                }
            }
        })
        .await
        .context("Gateway readiness timed out")?
    }
}

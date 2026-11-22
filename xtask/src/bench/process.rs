//! Own benchmark subprocesses and bound cleanup after a failure or interruption.
use super::{TIMEOUT, collect, fixture::Fixture, rpc};
use anyhow::{Context, Result, bail, ensure};
use reqwest::Client;
use std::{path::Path, process::Stdio};
use tokio::{
    process::{Child, Command},
    signal::unix::{Signal, SignalKind, signal},
    time::timeout,
};

enum Process {
    Gateway(Child),
    Direct(Child),
}

#[derive(Default)]
pub(super) struct Processes(Vec<Process>);

impl Processes {
    pub(super) fn start(&mut self, fixture: &Fixture<'_>) -> Result<()> {
        let root = fixture.directory.path();
        if let Some(gateway) = &fixture.gateway {
            let child = command(&fixture.binaries.gateway, root)
                .arg("serve")
                .arg("--config")
                .arg(&gateway.config)
                .spawn()
                .context("Cannot start gateway")?;
            self.0.push(Process::Gateway(child));
        } else {
            for _ in 0..fixture.concurrency {
                let child = command(&fixture.binaries.backend, root)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .context("Cannot start direct mock backend")?;
                self.0.push(Process::Direct(child));
            }
        }
        Ok(())
    }

    pub(super) fn pids(&self) -> Result<Vec<u32>> {
        self.0
            .iter()
            .map(|process| match process {
                Process::Gateway(child) | Process::Direct(child) => {
                    child.id().context("Missing child PID")
                }
            })
            .collect()
    }

    pub(super) async fn connect(
        &mut self,
        fixture: &Fixture<'_>,
        http: &Client,
    ) -> Result<Vec<rpc::Session<'_>>> {
        if let (Some(gateway), [Process::Gateway(child)]) =
            (&fixture.gateway, self.0.as_mut_slice())
        {
            gateway.ready(http, child).await?;
            collect(
                (0..fixture.concurrency)
                    .map(|_| rpc::Session::http(http.clone(), &gateway.base, &gateway.token)),
            )
            .await
        } else {
            collect(self.0.iter_mut().map(|process| async {
                let Process::Direct(child) = process else {
                    bail!("Expected a direct mock backend");
                };
                rpc::Session::stdio(child).await
            }))
            .await
        }
    }

    pub(super) async fn stop(self) -> Result<()> {
        let results = futures::future::join_all(self.0.into_iter().map(Process::stop)).await;
        results.into_iter().fold(Ok(()), finish)
    }
}

impl Process {
    async fn stop(self) -> Result<()> {
        match self {
            Self::Gateway(mut child) => terminate(&mut child).await,
            Self::Direct(mut child) => {
                drop(child.stdin.take());
                wait(&mut child).await
            }
        }
    }
}

pub(super) fn command(binary: &Path, root: &Path) -> Command {
    let mut command = Command::new(binary);
    // Isolate fixture-specific environment as well as config/state and working directory.
    // Each child has its own group so terminal interrupts are handled by the owner.
    command
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .process_group(0)
        .kill_on_drop(true);
    command
}

pub(super) async fn terminate(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        let pid = child.id().context("Missing child PID")?;
        // SAFETY: this child is still ours and unreaped. SIGTERM lets it clean up its workers.
        if unsafe { libc::kill(i32::try_from(pid)?, libc::SIGTERM) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return finish(
                    Err(error).context("Cannot signal benchmark child"),
                    wait(child).await,
                );
            }
        }
    }
    wait(child).await
}

async fn wait(child: &mut Child) -> Result<()> {
    match timeout(TIMEOUT, child.wait()).await {
        Ok(status) => {
            let status = status?;
            ensure!(status.success(), "Benchmark child exited with {status}");
            Ok(())
        }
        Err(_) => {
            timeout(TIMEOUT, child.kill())
                .await
                .context("Cannot reap benchmark child")??;
            bail!("Benchmark child did not stop gracefully and was killed");
        }
    }
}

pub(super) fn finish<T>(result: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("Cleanup also failed: {cleanup:#}")))
        }
    }
}

pub(super) struct Signals {
    interrupt: Signal,
    terminate: Signal,
}

impl Signals {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    pub(super) async fn interrupted(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {},
            _ = self.terminate.recv() => {},
        }
    }
}

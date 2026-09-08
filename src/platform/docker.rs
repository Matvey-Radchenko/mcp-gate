//! The Docker daemon outlives its CLI. Stop only the container ID created by this worker.
use anyhow::{Result, ensure};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

pub fn validate(args: &[String]) -> Result<()> {
    ensure!(
        args.first().is_some_and(|a| a == "run"),
        "Only stdio docker run is supported; exec/attach/shared containers are left unchanged"
    );
    ensure!(
        args.iter().any(|a| a == "-i" || a == "--interactive"),
        "docker run requires explicit stdio -i/--interactive"
    );
    for arg in args {
        let key = arg.split('=').next().unwrap_or(arg);
        ensure!(
            !matches!(
                key,
                "-d" | "--detach" | "-t" | "--tty" | "--name" | "--cidfile" | "--restart"
            ) && !(arg.starts_with('-')
                && !arg.starts_with("--")
                && (arg.contains('d') || arg.contains('t'))),
            "Docker detach, tty, named/shared containers and restart policies require a separate manual setup"
        );
    }
    Ok(())
}
pub struct Container {
    executable: PathBuf,
    cidfile: PathBuf,
    environment: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}
impl Container {
    pub fn prepare(command: &mut Command, directory: &Path) -> Result<Self> {
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        validate(&args)?;
        let cidfile = directory.join(format!("container-{}.cid", uuid::Uuid::new_v4()));
        let executable = PathBuf::from(command.as_std().get_program());
        let environment = command
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
            .collect();
        let mut replacement = Command::new(&executable);
        replacement
            .arg("run")
            .arg("--cidfile")
            .arg(&cidfile)
            .args(&args[1..]);
        replacement.env_clear();
        for (key, value) in command.as_std().get_envs() {
            if let Some(value) = value {
                replacement.env(key, value);
            }
        }
        if let Some(cwd) = command.as_std().get_current_dir() {
            replacement.current_dir(cwd);
        }
        *command = replacement;
        Ok(Self {
            executable,
            cidfile,
            environment,
        })
    }
    pub async fn stop(self) {
        if let Ok(value) = std::fs::read_to_string(&self.cidfile) {
            let id = value.trim();
            if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                let mut command = Command::new(&self.executable);
                command
                    .args(["stop", "--time", "5", id])
                    .env_clear()
                    .envs(self.environment)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true);
                let stopped = tokio::time::timeout(Duration::from_secs(12), command.status()).await;
                if !matches!(stopped,Ok(Ok(status)) if status.success()) {
                    tracing::error!(
                        "Owned Docker container cleanup failed; private cidfile retained for recovery"
                    );
                    return;
                }
            }
        }
        let _ = std::fs::remove_file(self.cidfile);
    }
}

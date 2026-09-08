pub use crate::backend::Backend;
use crate::backend::Profile;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub tool_policy: crate::policy::ToolPolicy,
    #[serde(default = "default_version")]
    pub format_version: u32,
    #[serde(default)]
    pub ownership: Ownership,
    #[serde(default)]
    pub shared_client_roots: SharedClientRoots,
    #[serde(default = "default_queue")]
    pub max_pending_calls: usize,
    #[serde(default = "default_startup")]
    pub queue_timeout_seconds: u64,
    pub listen: SocketAddr,
    pub token_file: PathBuf,
    pub catalog_file: PathBuf,
    pub state_dir: PathBuf,
    #[serde(default = "default_workers")]
    pub max_workers: usize,
    #[serde(default = "default_sessions")]
    pub max_sessions: usize,
    #[serde(default = "default_grace")]
    pub disconnect_grace_seconds: u64,
    #[serde(default = "default_startup")]
    pub startup_timeout_seconds: u64,
    #[serde(default = "default_call")]
    pub call_timeout_seconds: u64,
    pub backend: Backend,
}
fn default_version() -> u32 {
    1
}
fn default_queue() -> usize {
    16
}
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    #[default]
    Session,
    Shared,
}
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SharedClientRoots {
    #[default]
    Reject,
    /// Explicit operator decision for root-independent APIs. Never advertise or
    /// forward client roots upstream; backend context is fixed by its config.
    Ignore,
}
fn default_workers() -> usize {
    4
}
fn default_sessions() -> usize {
    128
}
fn default_grace() -> u64 {
    60
}
fn default_startup() -> u64 {
    30
}
fn default_call() -> u64 {
    300
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let value: Self = toml::from_str(&std::fs::read_to_string(path)?)
            .map_err(|_| anyhow::anyhow!("Invalid gateway configuration; values hidden"))?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        self.tool_policy.validate()?;
        ensure!(
            matches!(self.format_version, 1 | 2),
            "Unsupported configuration version"
        );
        ensure!(
            self.format_version == 2
                || (self.ownership == Ownership::Session
                    && self.backend.profile == Profile::ChromeDevtools),
            "Generic/shared backends require format_version = 2"
        );
        ensure!(
            self.ownership != Ownership::Shared
                || (self.backend.profile == Profile::Stdio && self.max_workers == 1),
            "Shared mode requires the stdio profile and max_workers = 1"
        );
        ensure!(
            self.max_pending_calls <= 1024 && self.queue_timeout_seconds > 0,
            "Invalid queue limits"
        );
        self.backend.validate()?;
        ensure!(
            self.shared_client_roots == SharedClientRoots::Reject
                || self.ownership == Ownership::Shared,
            "shared_client_roots only applies to shared mode"
        );
        ensure!(
            self.listen.ip().is_loopback(),
            "Only loopback listeners are supported"
        );
        ensure!(
            self.max_workers > 0 && self.max_sessions >= self.max_workers,
            "Invalid capacity limits"
        );
        ensure!(
            self.startup_timeout_seconds > 0
                && self.call_timeout_seconds > 0
                && self.disconnect_grace_seconds > 0,
            "Timeouts must be positive"
        );
        for p in [&self.token_file, &self.catalog_file, &self.state_dir] {
            ensure!(
                p.is_absolute(),
                "Configuration paths must be absolute: {}",
                p.display()
            );
        }
        Ok(())
    }
    pub fn token(&self) -> Result<String> {
        crate::platform::validate_private(&self.token_file)?;
        let token = std::fs::read_to_string(&self.token_file)?.trim().to_owned();
        ensure!(
            token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "Token must be 32 bytes encoded as hex"
        );
        Ok(token)
    }
}

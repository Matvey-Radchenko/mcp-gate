pub use crate::backend::Backend;
use crate::backend::Profile;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
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
    /// An explicit operator decision survives catalog refresh independently of recipes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership_source: Option<OwnershipSource>,
    #[serde(default)]
    pub shared_client_roots: SharedClientRoots,
    #[serde(default, skip_serializing_if = "SharedRecovery::is_manual")]
    pub shared_recovery: SharedRecovery,
    /// Backend lifetime without executable requests (tools/call, resources/read,
    /// prompts/get), measured from completion. Discovery and ping do not extend
    /// it. Active/admitted queued work prevents shutdown. Session ownership also
    /// keeps an open GET stream alive for recordings; expiry closes its HTTP
    /// session. Shared expiry keeps HTTP sessions and never clears failure state.
    /// Omitted selects the ownership default (session: 900, shared: off); 0 is off.
    #[serde(
        default,
        alias = "session_idle_seconds",
        alias = "shared_idle_seconds",
        deserialize_with = "deserialize_idle",
        skip_serializing_if = "Option::is_none"
    )]
    pub idle_seconds: Option<u64>,
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
    /// When every worker slot is busy, shut down the longest-idle session-owned
    /// backend that has been idle at least this long. Zero evicts any idle one.
    #[serde(default = "default_session_evict")]
    pub session_idle_evict_seconds: u64,
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipSource {
    Operator,
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
/// Restart only backends whose volatile state may be discarded. This never
/// retries an action or establishes whether a remote write completed.
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SharedRecovery {
    #[default]
    Manual,
    Restart,
}
impl SharedRecovery {
    pub fn is_manual(&self) -> bool {
        *self == Self::Manual
    }
}
pub(crate) const DEFAULT_SESSION_WORKERS: usize = 4;
fn default_workers() -> usize {
    DEFAULT_SESSION_WORKERS
}
fn default_sessions() -> usize {
    128
}
fn default_grace() -> u64 {
    60
}
fn default_session_evict() -> u64 {
    2 * 60
}
fn default_startup() -> u64 {
    30
}
fn default_call() -> u64 {
    300
}

fn deserialize_idle<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Seconds(u64),
        Off(Off),
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Off {
        Off,
    }
    Ok(Some(match Input::deserialize(deserializer)? {
        Input::Seconds(seconds) => seconds,
        Input::Off(_) => 0,
    }))
}

impl Config {
    /// Resolve an omitted value only after ownership is known. Disabled timers
    /// have no deadline; session capacity eviction remains independent.
    pub fn idle_timeout(&self) -> Option<Duration> {
        let seconds = self.idle_seconds.unwrap_or(match self.ownership {
            Ownership::Session => 900,
            Ownership::Shared => 0,
        });
        (seconds > 0).then(|| Duration::from_secs(seconds))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let value: Self = toml::from_str(&std::fs::read_to_string(path)?)
            .map_err(|_| anyhow::anyhow!("Invalid gateway configuration; values hidden"))?;
        value.validate()?;
        Ok(value)
    }

    /// Resolve the local listener and its private bearer token for an MCP client.
    pub fn client_connection(path: &Path) -> Result<(SocketAddr, String)> {
        let config = Self::load(path)?;
        Ok((config.listen, config.token()?))
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
            self.ownership != Ownership::Shared
                || !self.backend.docker
                || self.idle_timeout().is_none(),
            "Cannot enable idle_seconds for a shared Docker backend; use idle_seconds = 0 (off)"
        );
        ensure!(
            self.shared_recovery == SharedRecovery::Manual
                || (self.ownership == Ownership::Shared && !self.backend.docker),
            "Automatic recovery requires an explicitly reviewed shared, non-Docker backend"
        );
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
        ensure!(
            self.ownership != Ownership::Session
                || self.idle_timeout().is_none_or(|idle| {
                    Duration::from_secs(self.session_idle_evict_seconds) <= idle
                }),
            "session_idle_evict_seconds cannot exceed idle_seconds when session idle is enabled"
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

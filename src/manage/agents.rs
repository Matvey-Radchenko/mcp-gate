//! Read-only discovery of local.mcp-gate user services. Helpers are never executed.
use super::store::Registry;
use crate::config::Config;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct Service {
    pub name: String,
    pub label: String,
    pub binary: PathBuf,
    pub config: PathBuf,
    pub listen: Option<String>,
    pub ownership: &'static str,
    pub state: &'static str,
    #[cfg(target_os = "linux")]
    #[serde(flatten)]
    registration: super::service::linux::ServiceState,
}

pub async fn discover(home: &Path, registry: &Registry) -> anyhow::Result<Vec<Service>> {
    let mut services = Vec::new();
    for launch in launches(home)? {
        let owned = registry
            .gateways
            .iter()
            .any(|record| launch.is_recorded_as(record));
        let (listen, state) = describe(&launch.config).await;
        services.push(Service {
            name: launch.name,
            label: launch.label,
            binary: launch.binary,
            config: launch.config,
            listen,
            ownership: if owned { "managed" } else { "unmanaged" },
            state,
            #[cfg(target_os = "linux")]
            registration: launch.registration,
        });
    }
    services.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(services)
}

pub fn explain(services: &[Service]) {
    if services.is_empty() {
        println!("No local.mcp-gate user services found.");
        return;
    }
    println!("Gateways");
    println!("  {:<24} {:<12} state", "name", "ownership");
    for service in services {
        println!(
            "  {:<24} {:<12} {}",
            super::ui::safe(&service.name),
            service.ownership,
            service.state
        );
    }
    if services
        .iter()
        .any(|service| service.ownership == "unmanaged")
    {
        println!("Unmanaged gateways are outside setup and remove.");
        println!("Adopt one with: mcp-gate adopt <name>");
        println!("Preview adoption: mcp-gate adopt <name> --dry-run");
    }
}

struct Launch {
    name: String,
    label: String,
    binary: PathBuf,
    config: PathBuf,
    #[cfg(target_os = "linux")]
    registration: super::service::linux::ServiceState,
}

impl Launch {
    fn is_recorded_as(&self, record: &super::store::Record) -> bool {
        record.label == self.label && record.binary == self.binary && record.config() == self.config
    }
}

#[cfg(target_os = "linux")]
fn launches(_home: &Path) -> anyhow::Result<Vec<Launch>> {
    Ok(super::service::linux::definitions()?
        .into_iter()
        .filter_map(|(label, identity, registration)| {
            if identity.binary.file_name()?.to_str()? != "mcp-gate" {
                return None;
            }
            let name = label.strip_prefix("local.mcp-gate.")?.to_owned();
            Some(Launch {
                name,
                label,
                binary: identity.binary,
                config: identity.config,
                registration,
            })
        })
        .collect())
}

#[cfg(target_os = "macos")]
fn launches(home: &Path) -> anyhow::Result<Vec<Launch>> {
    Ok(plists(home)
        .into_iter()
        .filter_map(|path| {
            let (label, binary, config) = super::service::launchd::identity(&path).ok()?;
            launch(label, binary, config)
        })
        .collect())
}

#[cfg(windows)]
fn launches(_home: &Path) -> anyhow::Result<Vec<Launch>> {
    Ok(super::service::scheduler::definitions()?
        .into_iter()
        .filter_map(|(label, binary, config)| launch(label, binary, config))
        .collect())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn launches(_home: &Path) -> anyhow::Result<Vec<Launch>> {
    Ok(Vec::new())
}

#[cfg(any(target_os = "macos", windows))]
fn launch(label: String, binary: PathBuf, config: PathBuf) -> Option<Launch> {
    let name = label.strip_prefix("local.mcp-gate.")?.to_owned();
    (valid_name(&name) && gateway_binary(&binary) && config.is_absolute()).then(|| Launch {
        name,
        label,
        binary,
        config,
    })
}

pub(super) fn gateway_binary(path: &Path) -> bool {
    path.is_absolute()
        && matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("mcp-gate" | "mcp-gate.exe" | "mcp-session-gateway" | "mcp-session-gateway.exe")
        )
}

#[cfg(target_os = "linux")]
pub(super) fn recorded_labels(_home: &Path) -> std::collections::BTreeSet<String> {
    // A preview can still propose a label without a user bus. Registration
    // checks the live manager and the target path again before claiming it.
    super::service::linux::labels()
        .map(Iterator::collect)
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
pub(super) fn recorded_labels(home: &Path) -> std::collections::BTreeSet<String> {
    let mut labels = std::collections::BTreeSet::new();
    for path in plists(home) {
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            && stem.starts_with("local.mcp-gate.")
        {
            labels.insert(stem.to_string());
        }
    }
    labels
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn recorded_labels(_home: &Path) -> std::collections::BTreeSet<String> {
    #[cfg(windows)]
    return super::service::scheduler::definitions()
        .map(|tasks| tasks.into_iter().map(|(label, _, _)| label).collect())
        .unwrap_or_default();
    #[cfg(not(windows))]
    std::collections::BTreeSet::new()
}

pub(super) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

async fn describe(path: &Path) -> (Option<String>, &'static str) {
    let Ok(config) = Config::load(path) else {
        return (None, "config unreadable");
    };
    let listen = Some(config.listen.to_string());
    let state = match super::legacy::health(&config).await {
        Ok(_) => "running",
        Err(_) => "not reachable",
    };
    (listen, state)
}

#[cfg(target_os = "macos")]
pub(super) fn plists(home: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(home.join("Library/LaunchAgents")) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "plist") {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

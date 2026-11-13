//! Read-only discovery of local.mcp-gate LaunchAgents. Helpers are never executed.
use super::store::Registry;
use crate::config::Config;
use serde::Serialize;
use serde_json::Value;
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
}

pub async fn discover(home: &Path, registry: &Registry) -> Vec<Service> {
    let mut services = Vec::new();
    for path in plists(home) {
        let Some(plist) = read_plist(&path) else {
            continue;
        };
        let Some(launch) = launch(&plist) else {
            continue;
        };
        let owned = registry
            .gateways
            .iter()
            .any(|record| record.id == launch.name || record.label == launch.label);
        let (listen, state) = describe(&launch.config).await;
        services.push(Service {
            name: launch.name,
            label: launch.label,
            binary: launch.binary,
            config: launch.config,
            listen,
            ownership: if owned { "managed" } else { "unmanaged" },
            state,
        });
    }
    services.sort_by(|left, right| left.name.cmp(&right.name));
    services
}

pub fn explain(services: &[Service]) {
    if services.is_empty() {
        println!("No local.mcp-gate LaunchAgents found.");
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
}

fn launch(plist: &Value) -> Option<Launch> {
    let label = plist.get("Label")?.as_str()?;
    let name = label.strip_prefix("local.mcp-gate.")?;
    if !valid_name(name) {
        return None;
    }
    let args = plist.get("ProgramArguments")?.as_array()?;
    let binary = PathBuf::from(args.first()?.as_str()?);
    let file = binary.file_name()?.to_str()?;
    if !matches!(file, "mcp-gate" | "mcp-gate.exe") {
        return None;
    }
    let index = args.iter().position(|value| value == "--config")?;
    let config = PathBuf::from(args.get(index + 1)?.as_str()?);
    if !binary.is_absolute() || !config.is_absolute() {
        return None;
    }
    Some(Launch {
        name: name.to_string(),
        label: label.to_string(),
        binary,
        config,
    })
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

fn plists(home: &Path) -> Vec<PathBuf> {
    if !cfg!(any(target_os = "macos", feature = "test-backend")) {
        return Vec::new();
    }
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

fn read_plist(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && (value.get("Label").is_some() || value.get("ProgramArguments").is_some())
    {
        return Some(value);
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(path)
            .output()
            .ok()?;
        if output.status.success() {
            return serde_json::from_slice(&output.stdout).ok();
        }
    }
    None
}

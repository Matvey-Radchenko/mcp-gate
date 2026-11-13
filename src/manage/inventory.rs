//! A shared, read-only view for setup, status and remove. Ownership comes from the registry.
use super::{Selection, legacy, store::Registry};
use crate::clients::{self, Binding, Candidate, SourceIssue};
use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;

pub struct Inventory {
    pub candidates: Vec<Candidate>,
    pub connections: Vec<Connection>,
    pub issues: Vec<SourceIssue>,
}
#[derive(Serialize)]
pub struct Connection {
    pub client: clients::Client,
    pub name: String,
    pub source: PathBuf,
    pub project: Option<PathBuf>,
    pub kind: &'static str,
    pub state: &'static str,
    pub reason: Option<String>,
    pub gateway: Option<String>,
}

pub fn same_binding(a: &Binding, b: &Binding) -> bool {
    clients::identity(&a.target) == clients::identity(&b.target) && a.path == b.path
}

pub async fn read(
    options: &Selection,
    registry: &Registry,
    check_legacy: bool,
) -> Result<Inventory> {
    let home = super::store::home()?;
    let project = crate::platform::project_path(
        &options.project.clone().unwrap_or(std::env::current_dir()?),
    )?;
    let scan =
        clients::inventory_current(&home, &project, &options.client, options.project.is_some())?;
    let legacy = legacy::discover(&home, check_legacy).await;
    let mut candidates = Vec::new();
    let mut connections = Vec::new();
    for c in scan
        .candidates
        .into_iter()
        .filter(|c| super::matches(options, c.binding.client, &c.binding.name))
    {
        let managed = registry
            .gateways
            .iter()
            .find(|r| r.bindings.iter().any(|b| same_binding(b, &c.binding)));
        let endpoint = c.binding.direct["url"].as_str().unwrap_or_default();
        let old = legacy
            .iter()
            .find(|l| legacy::same_endpoint(endpoint, &l.endpoint));
        let disabled = c.binding.direct["enabled"] == false || c.binding.direct["disabled"] == true;
        let changed = managed.is_some_and(|record| {
            record
                .bindings
                .iter()
                .any(|b| same_binding(b, &c.binding) && b.after != c.binding.direct)
        });
        let kind = if managed.is_some() {
            "managed"
        } else if old.is_some() || c.issue.as_deref().is_some_and(marks_external) {
            "external"
        } else if !endpoint.is_empty() {
            "http"
        } else {
            "direct"
        };
        let state = if disabled {
            "disabled"
        } else if managed.is_some_and(|r| r.removing) {
            "cleanup pending"
        } else if changed {
            "settings changed"
        } else if let Some(old) = old {
            if !check_legacy
                || old
                    .diagnostic
                    .is_some_and(|d| d.contains("not checked") || d.contains("not verified"))
            {
                "not checked"
            } else {
                health_state(old.health.as_ref())
            }
        } else if kind == "direct" && c.issue.is_some() {
            if super::context::needs_directory(&c) {
                "setup needs a directory"
            } else {
                "setup needs attention"
            }
        } else {
            "not checked"
        };
        let reason = match kind {
            "managed" => None,
            "external" => Some(format!(
                "{}This connection already uses a gateway that this installation did not set up. Original direct settings were not saved, so setup and remove will not change it.",
                old.and_then(|l| l
                    .diagnostic
                    .map(str::to_owned)
                    .or_else(|| l.health.as_ref().and_then(super::health::diagnostic)))
                    .map_or_else(String::new, |d| format!("{d}. "))
            )),
            "http" => Some(
                "Existing HTTP connection; gateway ownership and backend health are not verified."
                    .into(),
            ),
            _ => c.issue.clone(),
        };
        connections.push(Connection {
            client: c.binding.client,
            name: c.binding.name.clone(),
            source: c.binding.source.clone(),
            project: c.binding.project.clone(),
            kind,
            state,
            reason,
            gateway: managed
                .map(|r| r.id.clone())
                .or_else(|| old.map(|l| l.config.display().to_string())),
        });
        if managed.is_none() {
            candidates.push(c);
        }
    }
    // Keep managed records visible when a user deleted or redirected their client entry.
    for record in &registry.gateways {
        for b in record
            .bindings
            .iter()
            .filter(|b| super::matches_binding(options, b))
        {
            if connections.iter().any(|c| {
                c.gateway.as_deref() == Some(&record.id) && c.client == b.client && c.name == b.name
            }) {
                continue;
            }
            connections.push(Connection { client: b.client, name: b.name.clone(), source: b.target.clone(), project: b.project.clone(),
                kind: "managed", state: "settings changed", reason: Some("Saved connection no longer matches client settings; inspect details before setup/remove.".into()), gateway: Some(record.id.clone()) });
        }
    }
    connections
        .sort_by(|a, b| (a.client, &a.project, &a.name).cmp(&(b.client, &b.project, &b.name)));
    Ok(Inventory {
        candidates,
        connections,
        issues: scan.issues,
    })
}

/// `legacy` was the pre-release name for a gateway this installation does not own.
pub(super) fn is_external(kind: &str) -> bool {
    kind == "external" || kind == "legacy"
}

fn marks_external(issue: &str) -> bool {
    issue.starts_with("External gateway") || issue.starts_with("Legacy gateway")
}

pub fn health_state(health: Option<&serde_json::Value>) -> &'static str {
    super::health::state(health)
}

pub fn overview(inventory: &Inventory, details: bool) {
    if inventory.connections.is_empty() {
        println!("No MCP connections found in the selected configuration files.");
    }
    let mut client = None;
    for c in &inventory.connections {
        if client != Some(c.client) {
            println!("\n{}", c.client);
            client = Some(c.client);
        }
        println!(
            "  {:<22} {:<10} {}",
            super::ui::safe(&c.name),
            c.kind,
            c.state
        );
        if details {
            println!(
                "    Source: {}",
                super::ui::safe(&c.source.display().to_string())
            );
            if let Some(project) = &c.project {
                println!(
                    "    Project: {}",
                    super::ui::safe(&project.display().to_string())
                );
            }
            if let Some(reason) = &c.reason {
                println!("    {}", super::ui::safe(reason));
            }
        }
    }
    for issue in &inventory.issues {
        println!(
            "\n{}: configuration needs attention{}",
            issue.client,
            if details {
                format!(
                    " — {}: {}",
                    super::ui::safe(&issue.source.display().to_string()),
                    issue.reason
                )
            } else {
                "; use --details".into()
            }
        );
    }
}

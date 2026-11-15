mod adopt;
mod agents;
mod choose;
mod context;
mod health;
mod inventory;
mod labels;
pub mod launch;
mod legacy;
mod preview;
mod recovery;
mod removal_preview;
mod remove;
pub mod runtime;
pub mod service;
mod setup;
mod status;
pub mod store;
mod ui;
mod upgrade;
mod wizard;
use crate::clients::Client;
use anyhow::{Result, ensure};
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Default, Clone)]
pub struct Selection {
    /// Client to include. Repeat to select more than one.
    #[arg(long, value_enum)]
    pub client: Vec<Client>,
    /// Limit Claude Code connections to this project directory.
    #[arg(long)]
    pub project: Option<PathBuf>,
    /// MCP server name to include. Repeat to select more than one.
    #[arg(long)]
    pub server: Vec<String>,
    /// Show the plan and change nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Show command, arguments, working directory and the gateway URL. Hide environment values, headers and secrets.
    #[arg(long)]
    pub diff: bool,
    /// Working directory for selected connections that do not already declare one.
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<PathBuf>,
    /// Set from the global --json flag.
    #[arg(skip)]
    pub json: bool,
    /// Apply without prompting. Requires both --client and --server.
    #[arg(long)]
    pub yes: bool,
    /// Set by `remove --all`. Hidden here so `setup` does not advertise it.
    #[arg(skip)]
    pub all: bool,
    /// Show source files and connections that cannot be changed.
    #[arg(long)]
    pub details: bool,
}
#[derive(Args)]
pub struct Status {
    /// Client to include. Repeat to select more than one.
    #[arg(long, value_enum)]
    pub client: Vec<Client>,
    /// Limit connections to this project directory.
    #[arg(long)]
    pub project: Option<PathBuf>,
    /// MCP server name to include. Repeat to select more than one.
    #[arg(long)]
    pub server: Vec<String>,
    /// Set from the global --json flag.
    #[arg(skip)]
    pub json: bool,
    /// Show source files and full diagnostic explanations.
    #[arg(long)]
    pub details: bool,
    /// Initialize configured backends for diagnosis. Does not call their tools.
    #[arg(long)]
    pub probe: bool,
    /// Legacy single-config diagnostics.
    #[arg(long)]
    pub config: Option<PathBuf>,
}
pub async fn setup(options: Selection) -> Result<()> {
    setup::run(options).await
}
pub async fn status(options: Status) -> Result<()> {
    status::run(options).await
}
pub async fn remove(mut options: Selection, all: bool, name: Option<String>) -> Result<()> {
    options.all = all;
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        options.server.push(name);
    }
    remove::run(options).await
}
pub async fn adopt(name: String, dry_run: bool, json: bool) -> Result<()> {
    adopt::run(name, dry_run, json).await
}

pub fn canceled(error: &anyhow::Error) -> bool {
    error.is::<ui::Canceled>()
}

pub(crate) fn confirm(message: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    ui::confirm(message)
}
pub(crate) fn explicit(options: &Selection, remove: bool) -> Result<()> {
    if !options.yes || (!options.server.is_empty() && !options.client.is_empty()) {
        return Ok(());
    }
    if remove {
        anyhow::bail!(
            "--yes requires both --client and --server. To restore every managed connection, use remove --all --yes."
        );
    }
    anyhow::bail!("--yes requires both --client and --server");
}
pub(crate) fn matches(options: &Selection, client: Client, name: &str) -> bool {
    (options.client.is_empty() || options.client.contains(&client))
        && (options.server.is_empty() || options.server.iter().any(|n| n == name))
}
pub(crate) fn restart(clients: &std::collections::BTreeSet<Client>) {
    if !clients.is_empty() {
        println!(
            "Restart each affected application once: {}. Existing actions are not restored or replayed.",
            clients
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

pub(crate) fn matches_binding(options: &Selection, binding: &crate::clients::Binding) -> bool {
    matches(options, binding.client, &binding.name)
        && options.project.as_ref().is_none_or(|project| {
            crate::platform::project_path(project).ok().as_ref() == binding.project.as_ref()
        })
}

/// Fault injection exists only in the test-feature binary, never npm artifacts.
pub(crate) fn checkpoint(stage: &str) -> Result<()> {
    #[cfg(feature = "test-backend")]
    ensure!(
        std::env::var("MCP_GATE_TEST_FAIL").as_deref() != Ok(stage),
        "Injected fixture failure at {stage}"
    );
    #[cfg(not(feature = "test-backend"))]
    let _ = stage;
    Ok(())
}

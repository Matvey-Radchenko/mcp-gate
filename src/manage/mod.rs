mod context;
mod health;
mod inventory;
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
    #[arg(long, value_enum)]
    pub client: Vec<Client>,
    #[arg(long)]
    pub project: Option<PathBuf>,
    #[arg(long)]
    pub server: Vec<String>,
    #[arg(long)]
    pub dry_run: bool,
    /// Show a detailed structural diff with all configuration values redacted.
    #[arg(long)]
    pub diff: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub yes: bool,
    /// Remove all managed connections within the selected scope (remove only).
    #[arg(long)]
    pub all: bool,
    /// Show source files and explanations, including connections that cannot be migrated.
    #[arg(long)]
    pub details: bool,
}
#[derive(Args)]
pub struct Status {
    #[arg(long, value_enum)]
    pub client: Vec<Client>,
    #[arg(long)]
    pub project: Option<PathBuf>,
    #[arg(long)]
    pub server: Vec<String>,
    #[arg(long)]
    pub json: bool,
    /// Show source files and full diagnostic explanations.
    #[arg(long)]
    pub details: bool,
    /// Explicitly initialize configured backends for diagnosis; never calls their tools.
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
pub async fn remove(options: Selection) -> Result<()> {
    remove::run(options).await
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
pub(crate) fn explicit(options: &Selection) -> Result<()> {
    ensure!(
        !options.yes || (!options.server.is_empty() && !options.client.is_empty()),
        "--yes requires explicit --client and --server selections"
    );
    Ok(())
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

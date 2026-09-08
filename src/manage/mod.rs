mod preview;
mod recovery;
mod remove;
pub mod runtime;
pub mod service;
mod setup;
mod status;
pub mod store;
mod upgrade;
use crate::clients::Client;
use anyhow::{Result, ensure};
use clap::Args;
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

#[derive(Args, Default)]
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
}
#[derive(Args)]
pub struct Status {
    #[arg(long)]
    pub json: bool,
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

pub(crate) fn confirm(message: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    ensure!(
        io::stdin().is_terminal(),
        "Interactive confirmation required; select --client and --server explicitly with --yes"
    );
    eprint!("{message} [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
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
        && options
            .project
            .as_ref()
            .is_none_or(|project| project.canonicalize().ok().as_ref() == binding.project.as_ref())
}

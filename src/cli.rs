//! Public argument parsing and terminal diagnostics; execution stays in main.
use clap::{Parser, Subcommand};
use mcp_gate::manage;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about)]
pub(crate) struct Cli {
    /// Write one JSON document to stdout. Accepted before or after the subcommand.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<Action>,
}
#[derive(Subcommand)]
pub(crate) enum Action {
    /// Connect installed MCP servers through local gateways.
    Setup(manage::Selection),
    /// Show or set a gateway's backend idle timeout; takes effect after restart.
    Idle(manage::Idle),
    /// Restore selected direct MCP connections; keep packages and data.
    ///
    /// `--yes` requires both `--client` and `--server`. `remove --all --yes` restores every managed connection.
    Remove {
        #[command(flatten)]
        options: manage::Selection,
        /// Restore every managed connection in the selected scope.
        #[arg(long)]
        all: bool,
        /// Managed connection name. Equivalent to --server when that list is empty.
        name: Option<String>,
    },
    /// Record an existing local.mcp-gate LaunchAgent in this installation.
    ///
    /// The name is the label suffix in `local.mcp-gate.<name>`, including older uuid labels.
    Adopt {
        /// Gateway name from the LaunchAgent label local.mcp-gate.<name>.
        name: String,
        /// Show the ownership change without writing the registry.
        #[arg(long)]
        dry_run: bool,
        /// Set from the global --json flag.
        #[arg(skip)]
        json: bool,
    },
    /// Stage a separate configured release. Does not load launchd or change any client.
    #[command(hide = true)]
    Stage {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        prefix: PathBuf,
        #[arg(long)]
        label: String,
        /// Optional dependency tree to snapshot; the backend entrypoint must be inside it.
        #[arg(long)]
        artifact_root: Option<PathBuf>,
    },
    /// Credential helper for a local MCP client. Its stdout contains a SECRET; do not log it.
    #[command(hide = true)]
    Headers {
        #[arg(long)]
        config: PathBuf,
    },
    /// Bridge an MCP stdio client to a local gateway.
    Connect {
        /// Managed gateway name, ID, or service label.
        name: String,
        /// Path to an explicit gateway TOML configuration.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Gateway state, configuration diagnostics and reasons for problems.
    Status(manage::Status),
    /// Close one MCP session. A shared backend remains owned by the gateway.
    #[command(hide = true)]
    CloseSession {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        id: String,
    },
    /// Install a new local release and generate a launchd plist; never changes client settings.
    #[command(hide = true)]
    Install {
        #[arg(long)]
        prefix: PathBuf,
        #[arg(long)]
        runtime: PathBuf,
        #[arg(long)]
        node: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8769")]
        listen: std::net::SocketAddr,
    },
    /// Serve authenticated Streamable HTTP. Starts no backend workers at boot.
    #[command(hide = true)]
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    /// Query one temporary backend and save its catalog for subsequent review.
    #[command(hide = true)]
    Catalog {
        #[arg(long)]
        config: PathBuf,
    },
    /// Validate configuration, catalog and token without starting a worker.
    #[command(hide = true)]
    Check {
        #[arg(long)]
        config: PathBuf,
        /// Set from the global --json flag.
        #[arg(skip)]
        json: bool,
    },
    /// Create a private random local client token. Never overwrites an existing file.
    #[command(hide = true)]
    InitToken {
        #[arg(long)]
        output: PathBuf,
    },
}
pub(crate) fn apply_json(action: &mut Action, json: bool) {
    match action {
        Action::Setup(options) | Action::Remove { options, .. } => options.json = json,
        Action::Status(options) => options.json = json,
        Action::Idle(options) => options.json = json,
        Action::Adopt { json: slot, .. } | Action::Check { json: slot, .. } => *slot = json,
        _ => {}
    }
}

pub(crate) fn print_start(json: bool) {
    const MESSAGE: &str = "\
mcp-gate connects installed MCP servers through local gateways.

Start here:
  mcp-gate setup              Choose connections and register gateways
  mcp-gate setup --dry-run    Preview the plan without changing anything
  mcp-gate status             Show connections and gateways

Run mcp-gate <command> --help for every flag.";
    if json {
        println!(
            "{}",
            serde_json::json!({
                "format_version": 1,
                "phase": "help",
                "message": MESSAGE,
                "commands": ["setup", "status", "remove", "adopt", "idle", "connect"]
            })
        );
    } else {
        println!("{MESSAGE}");
    }
}

pub(crate) fn json_parse_error(error: &clap::error::Error) -> String {
    let rendered = plain(&error.to_string());
    let mut lines = rendered
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let summary = lines.next().unwrap_or("invalid arguments");
    let summary = summary
        .strip_prefix("error:")
        .unwrap_or(summary)
        .trim()
        .to_string();
    let mut usage = None;
    let mut hint = None;
    for line in lines {
        if let Some(text) = line.strip_prefix("Usage:") {
            usage = Some(text.trim().to_string());
        } else if hint.is_none() {
            hint = Some(line.to_string());
        }
    }
    let mut value = serde_json::json!({"format_version":1,"phase":"failed","error":summary});
    if let Some(usage) = usage {
        value["usage"] = serde_json::json!(usage);
    }
    if let Some(hint) = hint {
        value["hint"] = serde_json::json!(hint);
    }
    value.to_string()
}

pub(crate) fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if !c.is_control() || c == '\n' || c == '\t' {
            out.push(c);
        }
    }
    out
}

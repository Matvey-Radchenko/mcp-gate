use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use mcp_gate::{
    catalog::Catalog, config::Config, deployment, gateway::Gateway, http::Server, install, manage,
};
use std::{fs::OpenOptions, path::PathBuf};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Connect installed MCP servers through local gateways.
    Setup(manage::Selection),
    /// Restore selected direct MCP connections; keep packages and data.
    Remove(manage::Selection),
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
    },
    /// Create a private random local client token. Never overwrites an existing file.
    #[command(hide = true)]
    InitToken {
        #[arg(long)]
        output: PathBuf,
    },
}
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("mcp_gate=info".parse()?),
        )
        .with_target(false)
        .init();
    match Cli::parse().command {
        Action::Stage {
            config,
            prefix,
            label,
            artifact_root,
        } => {
            deployment::stage(&config, &prefix, &label, artifact_root.as_deref()).await?;
        }
        Action::Headers { config } => {
            let config = Config::load(&config)?;
            println!(
                "{}",
                serde_json::json!({"Authorization":format!("Bearer {}",config.token()?)})
            );
        }
        Action::Setup(options) => manage::setup(options).await?,
        Action::Remove(options) => manage::remove(options).await?,
        Action::Status(options) => manage::status(options).await?,
        Action::CloseSession { config, id } => {
            let config = Config::load(&config)?;
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()?;
            client
                .delete(format!("http://{}/mcp", config.listen))
                .bearer_auth(config.token()?)
                .header("mcp-session-id", id)
                .header("mcp-protocol-version", "2025-11-25")
                .send()
                .await?
                .error_for_status()?;
            println!(
                "Session closed. Session-owned backends are cleaned up; a shared backend remains available."
            );
        }
        Action::Install {
            prefix,
            runtime,
            node,
            listen,
        } => {
            install::install(&prefix, &runtime, &node, listen).await?;
        }
        Action::InitToken { output } => {
            install::init_token(&output)?;
        }
        Action::Check { config } => {
            let config = Config::load(&config)?;
            config.token()?;
            let catalog = Catalog::load(&config.catalog_file, &config.backend)?;
            config.tool_policy.validate_catalog(&catalog.tools)?;
            println!(
                "OK: backend {}, {} tools, max {} workers",
                config.backend.version,
                catalog.tools.len(),
                config.max_workers
            );
        }
        Action::Catalog { config } => {
            let config = Config::load(&config)?;
            std::fs::create_dir_all(&config.state_dir)?;
            let catalog = install::generate_catalog(&config).await?;
            // Generated artifact, not handwritten schemas. Install only after reviewing its diff.
            std::fs::write(&config.catalog_file, serde_json::to_vec_pretty(&catalog)?)?;
            println!("Catalog generated: {} tools", catalog.tools.len());
        }
        Action::Serve { config } => {
            let config = Config::load(&config)?;
            let token = config.token()?;
            let catalog = Catalog::load(&config.catalog_file, &config.backend)?;
            config.tool_policy.validate_catalog(&catalog.tools)?;
            std::fs::create_dir_all(&config.state_dir)?;
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(config.state_dir.join("gateway.lock"))?;
            lock.try_lock_exclusive()
                .context("Another gateway owns this state directory")?;
            let listener = tokio::net::TcpListener::bind(config.listen)
                .await
                .context("Cannot bind gateway listener")?;
            tracing::info!(listen = %config.listen, tools = catalog.tools.len(), "gateway ready; workers start on demand");
            let gateway = Gateway::new(config, catalog);
            let server = Server::new(gateway.clone(), token);
            let cancellation = server.cancellation.clone();
            let router = server.router.clone();
            let mut http = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move { cancellation.cancelled().await })
                    .await
            });
            let failure = tokio::select! {
                signal = mcp_gate::platform::shutdown_signal() => { signal?; None },
                result = &mut http => Some(result),
            };
            tracing::info!("gateway shutting down");
            server.shutdown().await;
            gateway.shutdown().await;
            if let Some(result) = failure {
                result??;
            } else {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(3), http).await;
            }
            drop(lock);
        }
    }
    Ok(())
}

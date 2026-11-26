use anyhow::{Context, Result};
mod cli;
mod connect;

use axum::serve::ListenerExt;
use clap::Parser;
use cli::{Action, Cli, apply_json, json_parse_error, plain, print_start};
use fs2::FileExt;
use mcp_gate::{
    catalog::Catalog,
    config::{Config, Ownership},
    deployment,
    gateway::Gateway,
    http::Server,
    install, manage,
};
use std::{fs::OpenOptions, path::PathBuf};

#[tokio::main(worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr() && std::env::args().any(|arg| arg == "--json") {
                println!("{}", json_parse_error(&error));
                std::process::exit(2);
            }
            error.exit();
        }
    };
    let Some(mut action) = cli.command else {
        print_start(cli.json);
        return std::process::ExitCode::SUCCESS;
    };
    apply_json(&mut action, cli.json);
    let management = matches!(
        action,
        Action::Setup(_)
            | Action::Remove { .. }
            | Action::Adopt { .. }
            | Action::Status(_)
            | Action::Idle(_)
            | Action::Share(_)
            | Action::Isolate(_)
    );
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // Dependency diagnostics can contain upstream RPC error payloads.
        .with_env_filter(if management {
            "off"
        } else {
            "off,mcp_gate=info"
        })
        .with_target(false)
        .init();
    let json = match &action {
        Action::Setup(options) | Action::Remove { options, .. } => options.json,
        Action::Status(options) => options.json,
        Action::Idle(options) => options.json,
        Action::Share(options) | Action::Isolate(options) => options.json,
        Action::Adopt { json, .. } | Action::Check { json, .. } => *json,
        _ => false,
    };
    let result = dispatch(action).await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if manage::canceled(&error) {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({"format_version":1,"phase":"canceled"})
                    );
                } else {
                    println!("Canceled. No changes applied.");
                }
                return std::process::ExitCode::SUCCESS;
            }
            let message = plain(&error.to_string());
            if json {
                let value = error.downcast_ref::<manage::ModeFailure>().map_or_else(
                    || serde_json::json!({"format_version":1,"phase":"failed","error":message}),
                    |failure| failure.json(&message),
                );
                println!("{value}");
            }
            eprintln!("{message}");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn dispatch(action: Action) -> Result<()> {
    match action {
        Action::Stage {
            config,
            prefix,
            label,
            artifact_root,
        } => {
            deployment::stage(&config, &prefix, &label, artifact_root.as_deref()).await?;
        }
        Action::Headers { config } => {
            let (_, token) = Config::client_connection(&config)?;
            println!(
                "{}",
                serde_json::json!({"Authorization":format!("Bearer {token}")})
            );
        }
        Action::Connect { name, config } => connect::run(&name, config).await?,
        Action::Setup(options) => manage::setup(options).await?,
        Action::Remove { options, all, name } => manage::remove(options, all, name).await?,
        Action::Adopt {
            name,
            dry_run,
            json,
        } => manage::adopt(name, dry_run, json).await?,
        Action::Status(options) => manage::status(options).await?,
        Action::Idle(options) => manage::idle(options)?,
        Action::Share(options) => manage::change_mode(options, Ownership::Shared)?,
        Action::Isolate(options) => manage::change_mode(options, Ownership::Session)?,
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
        Action::Check { config, json } => {
            let config = Config::load(&config)?;
            config.token()?;
            let catalog = Catalog::load(&config.catalog_file, &config.backend)?;
            config.tool_policy.validate_catalog(&catalog.tools)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({"format_version":1,"phase":"ok","backend":config.backend.version,"tools":catalog.tools.len(),"max_workers":config.max_workers,"unforwarded_extensions":mcp_gate::catalog::unforwarded_extensions(&catalog.server_info)})
                );
            } else {
                println!(
                    "OK: backend {}, {} tools, max {} workers",
                    config.backend.version,
                    catalog.tools.len(),
                    config.max_workers
                );
                if let Some(note) = mcp_gate::catalog::unforwarded_note(&catalog.server_info) {
                    println!("{note}");
                }
            }
        }
        Action::Catalog { config } => {
            let config = Config::load(&config)?;
            std::fs::create_dir_all(&config.state_dir)?;
            let catalog = install::generate_catalog(&config).await?;
            // Generated artifact, not handwritten schemas. Install only after reviewing its diff.
            std::fs::write(&config.catalog_file, serde_json::to_vec_pretty(&catalog)?)?;
            println!("Catalog generated: {} tools", catalog.tools.len());
            if let Some(note) = mcp_gate::catalog::unforwarded_note(&catalog.server_info) {
                println!("{note}");
            }
        }
        Action::Serve { config } => serve(config).await?,
    }
    Ok(())
}

async fn serve(config: PathBuf) -> Result<()> {
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
    let mut shutdown = mcp_gate::platform::ShutdownSignal::new()?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .context("Cannot bind gateway listener")?
        .tap_io(|stream| {
            if let Err(error) = stream.set_nodelay(true) {
                tracing::warn!(%error, "failed to set TCP_NODELAY on accepted connection");
            }
        });
    tracing::info!(listen = %config.listen, tools = catalog.tools.len(), "gateway ready; workers start on demand");
    mcp_gate::catalog::log_unforwarded(&catalog.server_info);
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
        signal = shutdown.wait() => { signal?; None },
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
    Ok(())
}

//! OS boundaries for private storage, process ownership and shutdown.
pub mod docker;
mod storage;
pub use storage::{executable, private_dir, private_file, private_permissions, validate_private};
#[cfg(windows)]
pub(crate) mod windows_job;

pub async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = term.recv() => {},
        }
    }
    #[cfg(windows)]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "mcp-gate.exe"
    } else {
        "mcp-gate"
    }
}

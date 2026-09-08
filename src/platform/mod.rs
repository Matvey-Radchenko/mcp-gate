//! OS boundaries for private storage, process ownership and shutdown.
pub mod docker;
mod storage;
pub use storage::{executable, private_dir, private_file, private_permissions, validate_private};
#[cfg(windows)]
mod windows_acl;
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

/// Resolve physical project identity in the spelling clients use as JSON keys.
/// Rust's Windows verbatim prefix is an API detail, absent from client cwd keys.
pub fn project_path(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let path = path.canonicalize()?;
    #[cfg(windows)]
    {
        use std::{
            os::windows::ffi::{OsStrExt, OsStringExt},
            path::{Component, Prefix},
        };
        let wide: Vec<_> = path.as_os_str().encode_wide().collect();
        if let Some(Component::Prefix(prefix)) = path.components().next() {
            match prefix.kind() {
                Prefix::VerbatimDisk(_) => {
                    return Ok(std::ffi::OsString::from_wide(&wide[4..]).into());
                }
                Prefix::VerbatimUNC(_, _) => {
                    let mut regular = vec![u16::from(b'\\'), u16::from(b'\\')];
                    regular.extend_from_slice(&wide[8..]);
                    return Ok(std::ffi::OsString::from_wide(&regular).into());
                }
                _ => {}
            }
        }
    }
    Ok(path)
}

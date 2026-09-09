//! OS boundaries for private storage, process ownership and shutdown.
pub mod docker;
mod shutdown;
mod storage;
pub use shutdown::ShutdownSignal;
pub use storage::{executable, private_dir, private_file, private_permissions, validate_private};
#[cfg(windows)]
mod windows_acl;
#[cfg(windows)]
pub(crate) mod windows_job;

/// OS paths and locale needed by ordinary runtimes and installed-program lookup.
/// Application credentials remain explicit backend configuration.
pub const BASE_ENVIRONMENT: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "SystemRoot",
    "SystemDrive",
    "WINDIR",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "APPDATA",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "TEMP",
    "TMP",
    "COMSPEC",
    "PATHEXT",
];

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

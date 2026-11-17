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
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "USER",
    "LOGNAME",
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

#[cfg(test)]
mod tests {
    use super::BASE_ENVIRONMENT;

    #[test]
    fn linux_desktop_and_user_context_are_allowlisted() {
        for name in [
            "XDG_RUNTIME_DIR",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XAUTHORITY",
            "DBUS_SESSION_BUS_ADDRESS",
            "USER",
            "LOGNAME",
        ] {
            assert!(BASE_ENVIRONMENT.contains(&name), "{name}");
        }
    }
}

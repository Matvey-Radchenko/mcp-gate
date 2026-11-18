//! Private registry and compare-before-replace file transactions.
use crate::clients::Binding;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub format_version: u32,
    pub gateways: Vec<Record>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    #[serde(default)]
    pub removing: bool,
    pub id: String,
    pub release: PathBuf,
    pub label: String,
    pub binary_hash: String,
    pub binary: PathBuf,
    pub bindings: Vec<Binding>,
}
impl Record {
    pub fn config(&self) -> PathBuf {
        self.release.join("config.toml")
    }
}

pub fn root() -> Result<PathBuf> {
    #[cfg(feature = "test-backend")]
    if let Some(root) = std::env::var_os("MCP_GATE_TEST_ROOT") {
        let root = PathBuf::from(root);
        ensure!(root.is_absolute(), "Test root must be absolute");
        return Ok(root);
    }
    #[cfg(target_os = "macos")]
    return Ok(home()?.join("Library/Application Support/mcp-gate"));
    #[cfg(windows)]
    return Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is missing")?)
            .join("mcp-gate"),
    );
    #[cfg(target_os = "linux")]
    return Ok(xdg_home("XDG_DATA_HOME", ".local/share")?.join("mcp-gate"));
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    anyhow::bail!("Automatic setup is unsupported on this platform")
}

#[cfg(target_os = "linux")]
pub(super) fn xdg_home(variable: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Ok(path);
    }
    let home = home()?;
    ensure!(home.is_absolute(), "User home directory must be absolute");
    Ok(home.join(fallback))
}
pub fn home() -> Result<PathBuf> {
    #[cfg(feature = "test-backend")]
    if let Some(home) = std::env::var_os("MCP_GATE_TEST_HOME") {
        return Ok(PathBuf::from(home));
    }
    Ok(PathBuf::from(
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .context("User home directory is not set")?,
    ))
}
pub fn prepare(root: &Path) -> Result<()> {
    if !root.exists() {
        fs::create_dir_all(root.parent().context("Missing parent")?)?;
        crate::platform::private_dir(root)?;
    }
    ensure!(
        !fs::symlink_metadata(root)?.file_type().is_symlink(),
        "State root cannot be a symlink"
    );
    crate::platform::private_permissions(root)?;
    for name in ["releases", "operations"] {
        let p = root.join(name);
        if !p.exists() {
            crate::platform::private_dir(&p)?;
        }
    }
    Ok(())
}
pub fn load(root: &Path) -> Result<Registry> {
    validate_operations(root)?;
    let p = root.join("registry.json");
    if !p.exists() {
        return Ok(Registry {
            format_version: 1,
            gateways: vec![],
        });
    }
    crate::platform::validate_private(&p)?;
    let registry: Registry =
        serde_json::from_slice(&fs::read(p)?).context("Invalid managed registry")?;
    ensure!(
        registry.format_version == 1,
        "Unsupported registry version; use a compatible mcp-gate version"
    );
    Ok(registry)
}
fn validate_operations(root: &Path) -> Result<()> {
    let directory = root.join("operations");
    if !directory.exists() {
        return Ok(());
    }
    for item in fs::read_dir(directory)? {
        let path = item?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        crate::platform::validate_private(&path)?;
        let journal: Journal = serde_json::from_slice(&fs::read(path)?)
            .map_err(|_| anyhow::anyhow!("Unknown or invalid operation format; no changes made"))?;
        ensure!(
            journal.format_version == 1
                && matches!(
                    journal.phase.as_str(),
                    "preparing"
                        | "committing"
                        | "removing"
                        | "upgrading"
                        | "complete"
                        | "restored"
                        | "needs-attention"
                        | "restored-service-unavailable"
                ),
            "Unsupported operation version/phase; use a compatible installer"
        );
    }
    Ok(())
}
pub fn save(root: &Path, registry: &Registry) -> Result<()> {
    atomic(
        &root.join("registry.json"),
        &serde_json::to_vec_pretty(registry)?,
    )
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_replaceable(path)?;
    let parent = path.parent().context("Missing target parent")?;
    with_temporary(|temporary| {
        let mut file = temporary.open(parent)?;
        file.write_all(bytes)?;
        temporary.publish(file, path)
    })
}

pub(super) fn ensure_replaceable(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => ensure!(
            !meta.file_type().is_symlink(),
            "Refusing to replace a symlink"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(super) struct Temporary {
    path: Option<PathBuf>,
}

/// Owns the staged path until publication, removing it if the operation aborts.
pub(super) fn with_temporary<T>(operation: impl FnOnce(&mut Temporary) -> Result<T>) -> Result<T> {
    let mut temporary = Temporary { path: None };
    let result = operation(&mut temporary);
    let Some(path) = temporary.path else {
        return result;
    };
    match fs::remove_file(path) {
        Ok(()) => result,
        Err(error) if error.kind() == io::ErrorKind::NotFound => result,
        Err(cleanup) => match result {
            Ok(_) => Err(cleanup).context("Cannot remove staged file"),
            Err(error) => Err(error.context(format!("Cannot remove staged file: {cleanup}"))),
        },
    }
}

impl Temporary {
    pub(super) fn open(&mut self, parent: &Path) -> Result<fs::File> {
        ensure!(self.path.is_none(), "Temporary file already open");
        let path = parent.join(format!(".mcp-gate-{}.tmp", uuid::Uuid::new_v4()));
        let file = crate::platform::private_file(&path);
        // A Windows ACL failure can leave a file behind. An existing file belongs
        // to another writer and must be left alone.
        let already_exists = file
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<io::Error>())
            .is_some_and(|error| error.kind() == io::ErrorKind::AlreadyExists);
        if !already_exists {
            self.path = Some(path);
        }
        file
    }

    pub(super) fn publish(&mut self, file: fs::File, target: &Path) -> Result<()> {
        file.sync_all()?;
        drop(file);
        let parent = target.parent().context("Missing target parent")?;
        let path = self.path.as_mut().context("Missing staged file")?;
        if path.parent() != Some(parent) {
            // A copy hashed during staging only learns its final directory after EOF.
            // Move the staged file into that directory before the atomic replacement.
            let sibling = parent.join(path.file_name().context("Missing temporary file name")?);
            fs::rename(&*path, &sibling)?;
            *path = sibling;
        }
        ensure_replaceable(target)?;
        replace(path, target)?;
        self.path = None;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

fn replace(temporary: &Path, path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::rename(temporary, path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let a: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        ensure!(
            // SAFETY: both paths are nul-terminated, owned UTF-16 buffers.
            unsafe {
                MoveFileExW(
                    a.as_ptr(),
                    b.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } != 0,
            "Cannot atomically replace file"
        );
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    pub existed: bool,
    pub path: PathBuf,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub format_version: u32,
    pub phase: String,
    pub services: Vec<Record>,
    #[serde(default)]
    pub restart: Vec<Record>,
    pub changes: Vec<FileChange>,
}
impl Journal {
    pub fn save(&self, path: &Path) -> Result<()> {
        atomic(path, &serde_json::to_vec_pretty(self)?)
    }
    pub fn write(
        &mut self,
        journal: &Path,
        path: &Path,
        before: Vec<u8>,
        after: Vec<u8>,
    ) -> Result<()> {
        ensure!(
            read_optional(path)? == before,
            "Configuration changed concurrently; no write performed"
        );
        self.changes.push(FileChange {
            path: path.into(),
            existed: path.exists(),
            before,
            after,
        });
        self.save(journal)?;
        let change = self.changes.last().context("Missing planned change")?;
        ensure!(
            read_optional(path)? == change.before,
            "Configuration changed concurrently; rerun setup"
        );
        atomic(path, &change.after)
    }
    pub fn restore(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for change in self.changes.iter().rev() {
            match read_optional(&change.path) {
                Ok(current) if current == change.after => {
                    if (if change.existed {
                        atomic(&change.path, &change.before)
                    } else {
                        fs::remove_file(&change.path).map_err(Into::into)
                    })
                    .is_err()
                    {
                        issues.push(format!("Could not restore {}", change.path.display()));
                    }
                }
                Ok(current) if current == change.before => {}
                _ => issues.push(format!(
                    "Later edits preserved in {}; manual recovery required",
                    change.path.display()
                )),
            }
        }
        issues
    }
}

pub fn read_optional(path: &Path) -> Result<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_relocated_temporary_when_replacement_fails() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("version-hash");
        let target = directory.join("occupied");
        fs::create_dir_all(&target).unwrap();
        let result = with_temporary(|temporary| {
            let mut file = temporary.open(root.path())?;
            file.write_all(b"staged bytes")?;
            temporary.publish(file, &target)
        });
        assert!(result.is_err());
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
    }
}

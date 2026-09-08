//! Private registry and compare-before-replace file transactions.
use crate::clients::Binding;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
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
    #[cfg(not(any(target_os = "macos", windows)))]
    anyhow::bail!("Automatic setup supports macOS and Windows only")
}
pub fn home() -> Result<PathBuf> {
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
pub fn save(root: &Path, registry: &Registry) -> Result<()> {
    atomic(
        &root.join("registry.json"),
        &serde_json::to_vec_pretty(registry)?,
    )
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        ensure!(
            !fs::symlink_metadata(path)?.file_type().is_symlink(),
            "Refusing to replace a symlink"
        );
    }
    let parent = path.parent().context("Missing target parent")?;
    let temporary = parent.join(format!(".mcp-gate-{}.tmp", uuid::Uuid::new_v4()));
    let mut file = crate::platform::private_file(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    fs::rename(&temporary, path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let a: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: both paths are nul-terminated, owned UTF-16 buffers.
        ensure!(
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
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[derive(Deserialize, Serialize)]
pub struct FileChange {
    pub existed: bool,
    pub path: PathBuf,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}
#[derive(Deserialize, Serialize)]
pub struct Journal {
    pub format_version: u32,
    pub phase: String,
    pub services: Vec<Record>,
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

//! Private registry and compare-before-replace file transactions.
mod temporary;

pub(super) use temporary::{ensure_replaceable, replace, with_temporary};

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

    fn name(&self) -> &str {
        self.label
            .strip_prefix("local.mcp-gate.")
            .unwrap_or(&self.label)
    }
}

impl Registry {
    /// Prefer a unique service identity over the names of its client bindings.
    pub fn select(&self, name: &str) -> Result<&Record> {
        let mut matches: Vec<_> = self
            .gateways
            .iter()
            .filter(|record| record.id == name || record.label == name || record.name() == name)
            .collect();
        if matches.is_empty() {
            matches = self
                .gateways
                .iter()
                .filter(|record| record.bindings.iter().any(|binding| binding.name == name))
                .collect();
        }
        let name = super::ui::safe(name);
        match matches.as_slice() {
            [record] => Ok(record),
            [] => {
                let mut names: Vec<_> = self.gateways.iter().map(Record::name).collect();
                names.sort_unstable();
                names.dedup();
                let known = if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                };
                anyhow::bail!(
                    "No managed gateway matches {name}. Known gateways: {}. Run mcp-gate status or mcp-gate setup.",
                    super::ui::safe(&known)
                );
            }
            _ => anyhow::bail!(
                "Several gateways match {name}; use a gateway ID or full service label"
            ),
        }
    }
}

/// A binary-store deletion must retain the current helper bytes and every
/// registered service's exact path and digest, including pending removals.
/// Call under the setup/remove lock with the latest recovered registry.
pub fn retain_binary(root: &Path, registry: &Registry, binary: &Path) -> Result<bool> {
    if registry
        .gateways
        .iter()
        .any(|record| record.binary == binary)
    {
        return Ok(true);
    }
    let digest = crate::catalog::digest(binary)?;
    Ok(registry
        .gateways
        .iter()
        .any(|record| record.binary_hash == digest)
        || super::current::digest(&super::current::path(root))?.as_deref() == Some(digest.as_str()))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration: Option<super::service::BinaryChange>,
    pub changes: Vec<FileChange>,
}
impl Journal {
    /// Completed operations need no copy of the original service definition.
    pub(super) fn finish_migration(&mut self, path: &Path, phase: &str) -> Result<()> {
        let migration = self.migration.take();
        self.phase = phase.into();
        if let Err(error) = self.save(path) {
            self.migration = migration;
            return Err(error);
        }
        Ok(())
    }

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

//! A binary swap keeps the manager's original definition and activation settings.
use super::Record;
use crate::manage::runtime;
#[cfg(unix)]
use crate::manage::store;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use super::launchd as platform;
#[cfg(target_os = "linux")]
use super::linux::replacement as platform;
#[cfg(windows)]
use super::scheduler as platform;

#[derive(PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Definition {
    pub path: Option<PathBuf>,
    pub bytes: Vec<u8>,
}

impl Definition {
    #[cfg(unix)]
    pub fn file(&self) -> Result<&Path> {
        self.path
            .as_deref()
            .context("Missing service definition path")
    }
}

pub(in crate::manage) struct Snapshot {
    pub binary: PathBuf,
    pub(super) definition: Definition,
}

pub(in crate::manage) fn snapshot(record: &Record) -> Result<Option<Snapshot>> {
    ensure!(
        record
            .label
            .strip_prefix("local.mcp-gate.")
            .is_some_and(crate::manage::agents::valid_name),
        "Invalid gateway service label"
    );
    let current = platform::snapshot(record)?;
    ensure!(
        current
            .as_ref()
            .is_none_or(|current| current.binary == record.binary
                || crate::manage::agents::gateway_binary(&current.binary)),
        "Service executable no longer matches the recorded gateway"
    );
    Ok(current)
}

/// Kept only in the private operation journal. Service definitions can contain
/// credentials and must never be included in preview, diagnostics or errors.
#[derive(Serialize, Deserialize)]
pub struct BinaryChange {
    before: Definition,
    after: Definition,
}

impl BinaryChange {
    pub(in crate::manage) fn prepare(snapshot: Snapshot, target: &Path) -> Result<Self> {
        let before = snapshot.definition;
        let after = Definition {
            bytes: platform::rewrite(&before, &snapshot.binary, target)?,
            path: before.path.clone(),
        };
        Ok(Self { before, after })
    }

    pub(in crate::manage) async fn stop(&self, record: &Record) -> Result<bool> {
        runtime::stop_with(record, || {
            check(record, &self.before)?;
            platform::stop(record)
        })
        .await
    }

    pub(in crate::manage) fn activate(&self, record: &Record) -> Result<()> {
        check(record, &self.before)?;
        publish(record, &self.after)?;
        ensure!(
            snapshot(record)?.is_some_and(|current| current.binary == record.binary),
            "Service manager did not retain the migrated executable"
        );
        platform::start(record)
    }

    pub(in crate::manage) async fn restore(&self, record: &Record) -> Result<()> {
        let current =
            snapshot(record)?.context("Service definition disappeared during migration")?;
        if current.definition != self.before {
            ensure!(
                current.definition == self.after,
                "Later service definition edits preserved; manual recovery required"
            );
            let stopped = runtime::stop_with(record, || {
                check(record, &self.after)?;
                platform::stop(record)
            })
            .await?;
            ensure!(stopped, "Migrated gateway retained: busy or health unknown");
            check(record, &self.after)?;
            publish(record, &self.before)?;
        }
        Ok(())
    }
}

/// Recovery may find the old service running, stopped, or paused.
pub(in crate::manage) async fn resume(record: &Record) -> Result<()> {
    platform::start(record)?;
    let Ok(health) = runtime::health(record).await else {
        return runtime::ready(record).await;
    };
    if health["maintenance"] == true {
        ensure!(
            runtime::maintenance(record, false).await?,
            "Could not resume the previous service"
        );
    }
    Ok(())
}

fn check(record: &Record, expected: &Definition) -> Result<()> {
    ensure!(
        snapshot(record)?.is_some_and(|current| current.definition == *expected),
        "Service definition changed concurrently; later edits preserved"
    );
    Ok(())
}

#[cfg(unix)]
fn publish(_record: &Record, definition: &Definition) -> Result<()> {
    store::atomic(definition.file()?, &definition.bytes)
}

#[cfg(windows)]
fn publish(record: &Record, definition: &Definition) -> Result<()> {
    platform::publish(record, &definition.bytes)
}

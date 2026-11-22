//! Commit reviewed edits under the same lock as setup and remove.
use super::store;
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use std::{fs, path::Path};

pub(super) fn apply(
    root: &Path,
    record: &store::Record,
    before: &[u8],
    after: &[u8],
    command: &str,
    verify_review: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("setup.lock"))?;
    lock.try_lock_exclusive()
        .with_context(|| format!("Another setup/remove/{command} operation is running"))?;
    let registry = store::load(root)?;
    ensure!(
        registry
            .gateways
            .iter()
            .any(|current| current.id == record.id
                && current.config() == record.config()
                && current.label == record.label
                && !current.removing),
        "Managed gateway changed during review; rerun {command}"
    );
    ensure!(
        fs::read(record.config())? == before,
        "Configuration changed during review; no write performed. Rerun {command}"
    );
    verify_review()?;
    // One atomic replacement under the lock, with no service side effects or
    // multi-file changes requiring an installer recovery journal.
    store::atomic(&record.config(), after)
}

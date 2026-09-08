use super::{
    runtime, service,
    store::{self, Journal},
};
use anyhow::{Result, ensure};
use std::path::Path;

pub async fn restore(journal: &mut Journal, journal_path: &std::path::Path) -> Result<Vec<String>> {
    let mut recovery = journal.restore();
    if recovery.is_empty() {
        for record in journal.services.iter().rev() {
            if !runtime::stop_idle(record).await.unwrap_or(false) {
                recovery.push(format!(
                    "Service {} retained: busy or health unknown",
                    record.id
                ));
            }
        }
    }
    if recovery.is_empty() {
        for record in &journal.restart {
            if service::register(record).is_err() || runtime::ready(record).await.is_err() {
                recovery.push(format!(
                    "Previous service {} could not be restored; its backend may have changed",
                    record.id
                ));
            }
        }
    }
    journal.phase = if recovery.is_empty() {
        "restored"
    } else {
        "needs-attention"
    }
    .into();
    journal.save(journal_path)?;
    Ok(recovery)
}

pub async fn pending(root: &Path) -> Result<()> {
    for item in std::fs::read_dir(root.join("operations"))? {
        let path = item?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        crate::platform::validate_private(&path)?;
        let mut journal: Journal = serde_json::from_slice(&store::read_optional(&path)?)?;
        ensure!(
            journal.format_version == 1,
            "Unknown operation format; use the matching installer"
        );
        if matches!(journal.phase.as_str(), "complete" | "restored") {
            continue;
        }
        let issues = restore(&mut journal, &path).await?;
        ensure!(
            issues.is_empty(),
            "Incomplete operation retained: {}. Journal: {}",
            issues.join("; "),
            path.display()
        );
    }
    Ok(())
}

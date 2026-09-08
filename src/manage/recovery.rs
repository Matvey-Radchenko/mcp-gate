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
            if service::installed(record).unwrap_or(true) {
                if runtime::maintenance(record, true).await.unwrap_or(false) {
                    if service::unregister(record).is_err() {
                        recovery.push(format!("Service {} needs attention", record.id));
                    }
                } else {
                    recovery.push(format!(
                        "Service {} retained: busy or health unknown",
                        record.id
                    ));
                }
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
        ensure!(
            !path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("upgrade-")),
            "An interrupted upgrade needs recovery; inspect status before changing this installation"
        );
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

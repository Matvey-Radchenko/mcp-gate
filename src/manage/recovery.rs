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
    let settings_restored = recovery.is_empty();
    if settings_restored {
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
    } else if settings_restored {
        // External backend changes cannot be undone by restoring our files. The
        // failed restart must remain visible, but cannot prevent a fresh setup or
        // remove from repairing this now-idle installation.
        "restored-service-unavailable"
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
        if matches!(
            journal.phase.as_str(),
            "complete" | "restored" | "restored-service-unavailable"
        ) {
            continue;
        }
        let issues = restore(&mut journal, &path).await?;
        ensure!(
            issues.is_empty() || journal.phase == "restored-service-unavailable",
            "Incomplete operation retained: {}. Journal: {}",
            issues.join("; "),
            path.display()
        );
    }
    Ok(())
}

pub async fn diagnostics(root: &Path) -> Result<Vec<serde_json::Value>> {
    let directory = root.join("operations");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut results = Vec::new();
    for item in std::fs::read_dir(directory)? {
        let path = item?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        crate::platform::validate_private(&path)?;
        let journal: Journal = serde_json::from_slice(&store::read_optional(&path)?)?;
        ensure!(
            journal.format_version == 1,
            "Unknown operation format; use the matching installer"
        );
        if matches!(journal.phase.as_str(), "complete" | "restored") {
            continue;
        }
        if journal.phase == "restored-service-unavailable" {
            let mut healthy = true;
            for record in &journal.restart {
                healthy &= runtime::health(record).await.is_ok();
            }
            if healthy {
                continue;
            }
        }
        results.push(serde_json::json!({"journal":path,"phase":journal.phase,
            "reason":if journal.phase == "restored-service-unavailable" {
                "Previous settings restored, but the previous service could not restart. Run setup to repair it, or remove to restore direct connections."
            } else {
                "Incomplete operation retained. Setup/remove attempt safe recovery; later edits and busy services are preserved."
            }}));
    }
    Ok(results)
}

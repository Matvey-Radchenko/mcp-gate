use super::{
    Selection, preview, recovery, runtime, service,
    store::{self, Journal},
};
use crate::clients::{Binding, document};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde_json::json;
use std::{collections::BTreeSet, fs, path::Path};

pub async fn run(options: Selection) -> Result<()> {
    super::explicit(&options)?;
    let root = store::root()?;
    let registry = store::load(&root)?;
    let mut selected = Vec::new();
    for record in &registry.gateways {
        for b in &record.bindings {
            if super::matches_binding(&options, b)
                && (options.dry_run
                    || options.server.contains(&b.name)
                    || super::confirm(
                        &format!(
                            "Restore {} / {} to its direct connection?",
                            b.client, b.name
                        ),
                        false,
                    )?)
            {
                selected.push((record.id.clone(), b.clone()));
            }
        }
    }
    let plan = json!({"format_version":1,"phase":"preview","connections":selected.iter().map(|(_,b)| {
        let mut value=preview::target(b);
        if options.diff { value["diff"]=json!({"before":preview::redact(&b.after),"after":preview::redact(&b.before)}); }
        value
    }).collect::<Vec<_>>(),"packages_and_data":"retained","restart":"each affected application once"});
    if options.dry_run || selected.is_empty() {
        println!("{plan}");
        return Ok(());
    }
    eprintln!("{plan}");
    if !super::confirm("Restore selected direct MCP connections?", options.yes)? {
        return Ok(());
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("setup.lock"))?;
    lock.try_lock_exclusive()
        .context("Another setup/remove operation is running")?;
    recovery::pending(&root).await?;
    let mut registry = store::load(&root)?;
    // Files and registry commit before stopping services. A failed transaction can
    // restore connections while all their gateways are still running.
    let affected = restore_connections(&root, &mut registry, &selected)?;
    let mut pending = Vec::new();
    let mut keep = Vec::new();
    for record in registry.gateways {
        if !record.removing {
            keep.push(record);
            continue;
        }
        let absent = !service::installed(&record).unwrap_or(true);
        if absent || runtime::stop_idle(&record).await.unwrap_or(false) {
            continue;
        }
        let _ = runtime::maintenance(&record, false).await;
        pending.push(format!("{}: direct settings restored; service cleanup pending until clients release it. Rerun remove after restart.",record.id));
        keep.push(record);
    }
    registry.gateways = keep;
    store::save(&root, &registry)
        .context("Direct connections are restored; cleanup registry needs attention")?;
    if options.json {
        println!(
            "{}",
            json!({"format_version":1,"phase":"complete","restart_clients":affected,"pending":pending})
        );
    } else {
        println!("Direct MCP connections restored. Packages and user data retained.");
        for reason in pending {
            println!("{reason}");
        }
        super::restart(&affected);
    }
    Ok(())
}
fn restore_connections(
    root: &Path,
    registry: &mut store::Registry,
    selected: &[(String, Binding)],
) -> Result<BTreeSet<crate::clients::Client>> {
    let path = root
        .join("operations")
        .join(format!("remove-{}.json", uuid::Uuid::new_v4()));
    let mut journal = Journal {
        format_version: 1,
        phase: "removing".into(),
        services: vec![],
        restart: vec![],
        changes: vec![],
    };
    journal.save(&path)?;
    let mut affected = BTreeSet::new();
    let result: Result<()> = (|| {
        for (_, b) in selected {
            b.verify_fallback()?;
            let before = fs::read(&b.target)?;
            let text = std::str::from_utf8(&before)?;
            let actual = document::entry(text, b.toml, &b.path)?;
            ensure!(
                actual == b.before || actual == b.after,
                "Selected MCP changed after setup; preserving later edits"
            );
            if actual == b.after {
                let after = document::replace(text, b.toml, &b.path, &b.after, &b.before)?;
                journal.write(&path, &b.target, before, after.into_bytes())?;
            }
            affected.insert(b.client);
        }
        for record in &mut registry.gateways {
            let chosen: Vec<_> = selected
                .iter()
                .filter(|(id, _)| id == &record.id)
                .map(|(_, b)| b)
                .collect();
            if chosen.len() == record.bindings.len() && !chosen.is_empty() {
                record.removing = true;
            } else {
                record.bindings.retain(|b| {
                    !chosen
                        .iter()
                        .any(|c| c.target == b.target && c.path == b.path)
                });
            }
        }
        let target = root.join("registry.json");
        journal.write(
            &path,
            &target,
            store::read_optional(&target)?,
            serde_json::to_vec_pretty(registry)?,
        )?;
        journal.phase = "complete".into();
        journal.save(&path)
    })();
    if let Err(error) = result {
        let issues = journal.restore();
        journal.phase = if issues.is_empty() {
            "restored"
        } else {
            "needs-attention"
        }
        .into();
        journal.save(&path)?;
        anyhow::bail!(
            "Remove failed: {error}. Recovery: {}",
            if issues.is_empty() {
                "previous settings restored".into()
            } else {
                issues.join("; ")
            }
        );
    }
    Ok(affected)
}

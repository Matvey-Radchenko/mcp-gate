use super::{
    Selection, preview, recovery, runtime, service,
    store::{self, Journal, Registry},
    upgrade,
};
use crate::clients::{self, Candidate, document};
use anyhow::{Context, Result};
use fs2::FileExt;
use serde_json::json;
use std::{collections::BTreeSet, fs, path::Path};

pub async fn run(options: Selection) -> Result<()> {
    super::explicit(&options)?;
    let root = store::root()?;
    // Reading the format precedes all writes, including binary copies.
    let registry = store::load(&root)?;
    let project = options
        .project
        .clone()
        .unwrap_or(std::env::current_dir()?)
        .canonicalize()?;
    let candidates: Vec<_> = clients::discover_current(
        &store::home()?,
        &project,
        &options.client,
        options.project.is_some(),
    )?
    .into_iter()
    .filter(|c| super::matches(&options, c.binding.client, &c.binding.name))
    .filter(|c| {
        !registry.gateways.iter().any(|r| {
            r.bindings
                .iter()
                .any(|b| b.target == c.binding.target && b.path == c.binding.path)
        })
    })
    .collect();
    let managed: Vec<_> = registry
        .gateways
        .iter()
        .filter(|r| {
            r.bindings
                .iter()
                .any(|b| super::matches_binding(&options, b))
        })
        .flat_map(|r| r.bindings.iter().map(preview::target))
        .collect();
    let plan = json!({"servers":candidates.iter().map(|c| {
        let mut v=c.summary(); if options.diff { v["diff"] = preview::diff(c); } v
    }).collect::<Vec<_>>(),"managed":managed,
        "discovery":"Runs the original command. npx, uvx and Docker may download dependencies.",
        "verification":"Gateway readiness does not prove the running application has switched."});
    if options.dry_run {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"format_version":1,"phase":"preview","plan":plan})
            )?
        );
        return Ok(());
    }
    eprintln!("{}", serde_json::to_string_pretty(&plan)?);
    let selected = choose(candidates, &options)?;
    if selected.is_empty() && managed.is_empty() {
        println!(
            "{}",
            json!({"format_version":1,"phase":"unchanged","plan":plan})
        );
        return Ok(());
    }
    if !super::confirm(
        "Apply this setup, including backend discovery and user autostart services?",
        options.yes,
    )? {
        return Ok(());
    }
    store::prepare(&root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("setup.lock"))?;
    lock.try_lock_exclusive()
        .context("Another setup/remove operation is running")?;
    recovery::pending(&root).await?;
    let mut registry = store::load(&root)?;
    let updates = upgrade::apply(&root, &mut registry, &options).await?;
    let affected = install(&root, &mut registry, selected).await?;
    if options.json {
        println!(
            "{}",
            json!({"format_version":1,"phase":"complete","plan":plan,"updates":updates,
            "restart_clients":affected,"connection":"awaiting client restart and a new MCP connection"})
        );
    } else {
        for update in updates {
            println!("{update}");
        }
        println!(
            "Setup complete. Backends start on demand; application connections await verification."
        );
        super::restart(&affected);
    }
    Ok(())
}
fn choose(candidates: Vec<Candidate>, options: &Selection) -> Result<Vec<Candidate>> {
    let mut selected = Vec::new();
    for c in candidates {
        if c.issue.is_none()
            && (options.server.contains(&c.binding.name)
                || super::confirm(
                    &format!(
                        "Migrate {} / {} in {} mode?",
                        c.binding.client,
                        c.binding.name,
                        c.recipe.as_ref().map_or("session", |r| r.mode.as_str())
                    ),
                    false,
                )?)
        {
            selected.push(c);
        }
    }
    Ok(selected)
}
async fn install(
    root: &Path,
    registry: &mut Registry,
    selected: Vec<Candidate>,
) -> Result<BTreeSet<clients::Client>> {
    let path = root
        .join("operations")
        .join(format!("{}.json", uuid::Uuid::new_v4()));
    let mut journal = Journal {
        format_version: 1,
        phase: "preparing".into(),
        services: vec![],
        restart: vec![],
        changes: vec![],
    };
    journal.save(&path)?;
    let mut affected = BTreeSet::new();
    let result: Result<()> = async {
        for candidate in selected {
            candidate.binding.verify_fallback()?;
            let record = if let Some(record) = runtime::reuse(registry, &candidate)? {
                runtime::health(&record)
                    .await
                    .context("Matching shared gateway is unavailable")?;
                record
            } else {
                let record = runtime::prepare(root, &candidate).await?;
                journal.services.push(record.clone());
                journal.save(&path)?;
                service::register(&record)?;
                runtime::ready(&record).await?;
                record
            };
            let b = &record.bindings[0];
            b.verify_fallback()?;
            let before = fs::read(&b.target)?;
            let after = document::replace(
                std::str::from_utf8(&before)?,
                b.toml,
                &b.path,
                &b.before,
                &b.after,
            )?;
            journal.write(&path, &b.target, before, after.into_bytes())?;
            affected.insert(b.client);
            if let Some(existing) = registry.gateways.iter_mut().find(|r| r.id == record.id) {
                existing.bindings.extend(record.bindings);
            } else {
                registry.gateways.push(record);
            }
        }
        journal.phase = "committing".into();
        journal.save(&path)?;
        let target = root.join("registry.json");
        journal.write(
            &path,
            &target,
            store::read_optional(&target)?,
            serde_json::to_vec_pretty(registry)?,
        )?;
        journal.phase = "complete".into();
        journal.save(&path)
    }
    .await;
    if let Err(error) = result {
        let issues = recovery::restore(&mut journal, &path).await?;
        anyhow::bail!(
            "Setup failed: {error}. Recovery: {}. Private journal: {}",
            if issues.is_empty() {
                "previous settings restored".into()
            } else {
                issues.join("; ")
            },
            path.display()
        );
    }
    Ok(affected)
}

use super::{
    Selection, current, helper_migration, recovery, runtime, service,
    store::{self, Journal, Registry},
    upgrade,
};
use crate::clients::{self, Candidate, document};
use anyhow::{Context, Result};
use fs2::FileExt;
use serde_json::json;
use std::{collections::BTreeSet, fs, path::Path};

pub async fn run(mut options: Selection) -> Result<()> {
    anyhow::ensure!(
        !options.all,
        "setup does not accept --all. Select connections in the wizard or use explicit --client and --server."
    );
    super::explicit(&options, false)?;
    let root = store::root()?;
    // Reading the format precedes all writes, including binary copies.
    let registry = store::load(&root)?;
    let Some(chosen) = super::wizard::setup(&mut options, &registry, &root).await? else {
        return Ok(());
    };
    #[cfg(target_os = "linux")]
    if !chosen.gateways.is_empty() || !chosen.candidates.is_empty() {
        require_user_manager(&registry, &chosen.gateways)?;
    }
    let plan = chosen.plan;
    if !options.json {
        println!(
            "Preparing {} new connections and checking {} existing gateways...",
            chosen.candidates.len(),
            chosen.gateways.len()
        );
        if !chosen.candidates.is_empty() {
            println!(
                "Discovery runs the original command; npx, uvx or Docker may download dependencies."
            );
        }
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
    let binary = upgrade::binary(&root)?;
    current::publish(&root, &binary, &chosen.hash)?;
    let mut updates = upgrade::apply(
        &root,
        &mut registry,
        &options,
        &chosen.gateways,
        &binary,
        &chosen.hash,
    )
    .await?;
    let deferred = updates
        .messages
        .iter()
        .any(|message| message.contains("pending"));
    let mut affected = install(
        &root,
        &mut registry,
        chosen.candidates,
        &binary,
        &chosen.hash,
        options.json,
    )
    .await?;
    helper_migration::apply(&root, &mut registry, &chosen.helpers, &mut updates)?;
    if plan["refresh_helper_binary"] == true {
        updates
            .messages
            .push("Current client gateway executable updated".into());
    }
    affected.extend(updates.restart);
    if options.json {
        println!(
            "{}",
            json!({"format_version":1,"phase":"complete","plan":plan,"updates":updates.messages,
            "restart_clients":affected,"connection":if affected.is_empty() {
                "no client restart required"
            } else {
                "awaiting client restart and a new MCP connection"
            }})
        );
    } else {
        for update in updates.messages {
            println!("{update}");
        }
        if deferred {
            println!(
                "Setup finished with deferred changes. Run setup again after affected clients release their connections."
            );
        } else if affected.is_empty() {
            println!("Selected gateways are up to date. No application restart is needed.");
        } else {
            println!(
                "Setup complete. Backends start on demand; application connections await verification."
            );
        }
        super::restart(&affected);
    }
    Ok(())
}
#[cfg(target_os = "linux")]
fn require_user_manager(registry: &Registry, selected: &[String]) -> Result<()> {
    if service::linux::available().is_ok() {
        return Ok(());
    }
    let commands: Vec<_> = registry
        .gateways
        .iter()
        .filter(|record| selected.contains(&record.id))
        .map(|record| {
            let path = record.config();
            // Protect the real path from shell expansions inside double quotes.
            let mut escaped = String::with_capacity(path.as_os_str().len());
            for c in path.to_string_lossy().chars() {
                if matches!(c, '\\' | '"' | '$' | '`') {
                    escaped.push('\\');
                }
                escaped.push(c);
            }
            format!("mcp-gate serve --config \"{escaped}\"")
        })
        .collect();
    let manual = if commands.is_empty() {
        "Prepare a private state directory and a config with absolute paths, then run mcp-gate serve --config \"/absolute/path/config.toml\".".into()
    } else {
        format!(
            "To run an existing gateway manually:\n  {}",
            commands.join("\n  ")
        )
    };
    anyhow::bail!(
        "Automatic setup needs a reachable systemd --user manager. Client configs were not changed. {manual}"
    )
}

/// The readable label is per user, not per state directory. A parallel setup can
/// claim it after the preview, so registration walks the numeric suffix until
/// the user service manager accepts the name.
fn register_free_label(
    mut record: store::Record,
    journal: &mut Journal,
    path: &Path,
) -> Result<store::Record> {
    let name = record
        .bindings
        .first()
        .map(|binding| binding.name.clone())
        .unwrap_or_else(|| "server".into());
    let home = store::home()?;
    let mut reserved = super::labels::taken(
        &Registry {
            format_version: 1,
            gateways: Vec::new(),
        },
        &home,
    );
    for _ in 0..32 {
        // Recovery must know which label this attempt could have published,
        // even if the installer exits before registration returns.
        *journal
            .services
            .last_mut()
            .context("Missing prepared service")? = record.clone();
        journal.save(path)?;
        match service::register(&record) {
            Ok(()) => return Ok(record),
            Err(error) if service::label_in_use(&error) => {
                reserved.insert(record.label.clone());
                record.label = super::labels::allocate(&name, &reserved);
            }
            Err(error) => return Err(error),
        }
    }
    anyhow::bail!("Could not register a free service label")
}

async fn install(
    root: &Path,
    registry: &mut Registry,
    selected: Vec<Candidate>,
    binary: &Path,
    hash: &str,
    json: bool,
) -> Result<BTreeSet<clients::Client>> {
    if selected.is_empty() {
        return Ok(BTreeSet::new());
    }
    let path = root
        .join("operations")
        .join(format!("{}.json", uuid::Uuid::new_v4()));
    let mut journal = Journal {
        format_version: 1,
        phase: "preparing".into(),
        services: vec![],
        restart: vec![],
        migration: None,
        changes: vec![],
    };
    journal.save(&path)?;
    let mut affected = BTreeSet::new();
    let result: Result<()> = async {
        let mut prepared = Vec::new();
        for (index, candidate) in selected.into_iter().enumerate() {
            if !json {
                println!(
                    "  {}. {} / {}: preparing gateway...",
                    index + 1,
                    candidate.binding.client,
                    super::ui::safe(&candidate.binding.name)
                );
            }
            candidate.binding.verify_fallback()?;
            let record = if let Some((record, config)) = runtime::matching(registry, &candidate)? {
                runtime::health(record)
                    .await
                    .context("Matching shared gateway is unavailable")?;
                let mut binding = candidate.binding.clone();
                binding.after = super::remote::binding(&binding, root, &record.config(), &config)?;
                let mut reused = record.clone();
                reused.bindings = vec![binding];
                reused
            } else {
                let mut record =
                    runtime::prepare_with_binary(root, &candidate, binary, hash).await?;
                journal.services.push(record.clone());
                journal.save(&path)?;
                super::checkpoint("prepared")?;
                record = register_free_label(record, &mut journal, &path)?;
                super::checkpoint("registered")?;
                runtime::ready(&record).await?;
                super::checkpoint("ready")?;
                if !json && let Some(note) = extension_note(&record) {
                    println!("     {note}");
                }
                record
            };
            prepared.push(record.clone());
            if let Some(existing) = registry.gateways.iter_mut().find(|r| r.id == record.id) {
                existing.bindings.extend(record.bindings);
            } else {
                registry.gateways.push(record);
            }
        }
        if !json {
            println!("All selected gateways are ready. Applying local client settings...");
        }
        for record in prepared {
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
            super::checkpoint("client-written")?;
            affected.insert(b.client);
            if !json {
                println!("     Local settings prepared for {}.", b.client);
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
        super::checkpoint("registry-written")?;
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

#[cfg(all(test, target_os = "linux"))]
mod tests;

/// Informational only: a read failure here must not fail an otherwise ready setup.
fn extension_note(record: &store::Record) -> Option<String> {
    let config = crate::config::Config::load(&record.config()).ok()?;
    let catalog: crate::catalog::Catalog =
        serde_json::from_slice(&fs::read(&config.catalog_file).ok()?).ok()?;
    crate::catalog::unforwarded_note(&catalog.server_info)
}

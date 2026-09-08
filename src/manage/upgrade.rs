//! Existing installations retain endpoints, credentials and backend state across upgrades.
use super::{
    Selection, runtime, service,
    store::{self, Record, Registry},
};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Outcome {
    pub messages: Vec<String>,
    pub restart: std::collections::BTreeSet<crate::clients::Client>,
}

pub fn binary(root: &Path) -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    let hash = crate::catalog::digest(&current)?;
    let parent = root.join("binaries");
    if !parent.exists() {
        crate::platform::private_dir(&parent)?;
    }
    let directory = parent.join(format!("{}-{}", env!("CARGO_PKG_VERSION"), hash));
    let target = directory.join(crate::platform::binary_name());
    if target.exists() {
        ensure!(
            crate::catalog::digest(&target)? == hash,
            "Permanent binary was modified"
        );
        return Ok(target);
    }
    if !directory.exists() {
        crate::platform::private_dir(&directory)?;
    }
    let bytes = std::fs::read(current)?;
    store::atomic(&target, &bytes)?;
    crate::platform::executable(&target)?;
    Ok(target)
}
pub async fn apply(root: &Path, registry: &mut Registry, options: &Selection) -> Result<Outcome> {
    let hash = crate::catalog::digest(&std::env::current_exe()?)?;
    let mut outcome = Outcome::default();
    let results = &mut outcome.messages;
    for index in 0..registry.gateways.len() {
        let old = registry.gateways[index].clone();
        if !old
            .bindings
            .iter()
            .any(|b| super::matches_binding(options, b))
        {
            continue;
        }
        if old.removing {
            results.push(format!(
                "{}: removal pending; finish remove before setup",
                old.id
            ));
            continue;
        }
        let config = crate::config::Config::load(&old.config())?;
        let changed = crate::catalog::Catalog::load(&config.catalog_file, &config.backend).is_err();
        if old.binary_hash == hash
            && !changed
            && service::installed(&old)?
            && runtime::health(&old)
                .await
                .is_ok_and(|value| value["maintenance"] != true)
        {
            results.push(format!(
                "{}: already current; no token, service or backend restarted",
                old.id
            ));
            continue;
        }
        let path = root
            .join("operations")
            .join(format!("upgrade-{}.json", uuid::Uuid::new_v4()));
        let mut new = old.clone();
        new.binary = binary(root)?;
        new.binary_hash = hash.clone();
        let mut journal = store::Journal {
            format_version: 1,
            phase: "upgrading".into(),
            services: vec![new.clone()],
            restart: vec![old.clone()],
            changes: vec![],
        };
        journal.save(&path)?;
        if !runtime::stop_idle(&old).await.unwrap_or(false) {
            journal.phase = "complete".into();
            journal.save(&path)?;
            results.push(format!(
                "{}: update pending; gateway busy or health unknown",
                old.id
            ));
            continue;
        }
        let result: Result<()> = async {
            super::checkpoint("upgrade-stopped")?;
            if changed {
                refresh(&new, config, &mut journal, &path, options).await?;
            }
            service::register(&new)?;
            super::checkpoint("upgrade-registered")?;
            runtime::ready(&new).await?;
            super::checkpoint("upgrade-ready")?;
            registry.gateways[index] = new;
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
            let issues = super::recovery::restore(&mut journal, &path).await?;
            anyhow::bail!(
                "Upgrade failed: {error}. Recovery: {}. Private journal: {}",
                if issues.is_empty() {
                    "previous settings/service restored".into()
                } else {
                    issues.join("; ")
                },
                path.display()
            );
        }
        results.push(format!(
            "{}: gateway updated{}; endpoint and credentials preserved",
            old.id,
            if changed {
                " and backend catalog refreshed"
            } else {
                ""
            }
        ));
        outcome
            .restart
            .extend(old.bindings.iter().map(|b| b.client));
    }
    Ok(outcome)
}
async fn refresh(
    record: &Record,
    mut config: crate::config::Config,
    journal: &mut store::Journal,
    path: &Path,
    options: &Selection,
) -> Result<()> {
    // Shared ownership remains eligible only while the reviewed launch recipe matches.
    let mut command = vec![config.backend.command.to_string_lossy().into_owned()];
    command.extend(config.backend.command_args.clone());
    if let Some(entry) = &config.backend.entrypoint {
        command.push(entry.to_string_lossy().into_owned());
    }
    command.extend(config.backend.args.clone());
    let candidate = crate::clients::Candidate {
        binding: record
            .bindings
            .first()
            .context("Managed gateway has no bindings")?
            .clone(),
        command,
        cwd: config
            .backend
            .working_directory
            .clone()
            .context("Managed backend has no cwd")?,
        env: config.backend.env.clone(),
        env_files: config.backend.env_files.clone(),
        inherit_env: config.backend.inherit_env.clone(),
        issue: None,
        recipe: None,
    };
    let recipe = crate::clients::recipes::matching(&candidate);
    ensure!(
        config.ownership != crate::config::Ownership::Shared
            || recipe.as_ref().is_some_and(|r| r.mode == "shared"),
        "Changed shared backend no longer matches a reviewed recipe; keep it stopped until a compatible backend is restored or reviewed"
    );
    eprintln!(
        "Refreshing catalog with the original command; npx, uvx or Docker may download dependencies. No tools are called."
    );
    let catalog = crate::install::discover_and_pin(&mut config)
        .await
        .map_err(|_| anyhow::anyhow!("Backend discovery failed; values hidden"))?;
    if let Some(recipe) = recipe {
        ensure!(
            recipe.server_version == config.backend.version,
            "Discovered backend version is outside the reviewed recipe"
        );
    }
    super::preview::catalog_change(&config.catalog_file, &catalog, options)?;
    let target = &config.catalog_file;
    journal.write(
        path,
        target,
        store::read_optional(target)?,
        serde_json::to_vec_pretty(&catalog)?,
    )?;
    let target = record.config();
    journal.write(
        path,
        &target,
        store::read_optional(&target)?,
        toml::to_string_pretty(&config)?.into_bytes(),
    )
}

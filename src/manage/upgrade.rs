//! Existing installations retain endpoints, credentials and backend state across upgrades.
mod migration;

pub(super) use migration::{BinaryState, binary_state, selected};

use super::{
    Selection, runtime, service,
    store::{self, Record, Registry},
};
use crate::catalog;
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Outcome {
    pub messages: Vec<String>,
    pub restart: std::collections::BTreeSet<crate::clients::Client>,
}

pub(super) fn binary_path(root: &Path, hash: &str) -> PathBuf {
    root.join("binaries")
        .join(format!("{}-{hash}", env!("CARGO_PKG_VERSION")))
        .join(crate::platform::binary_name())
}

pub fn binary(root: &Path) -> Result<PathBuf> {
    let parent = root.join("binaries");
    store::ensure_replaceable(&parent)?;
    if !parent.exists() {
        crate::platform::private_dir(&parent)?;
    }
    store::with_temporary(|temporary| {
        let mut staged = None;
        let hash = catalog::current_exe_digest_with(|current| {
            let mut file = temporary.open(&parent)?;
            let hash = catalog::copy_digest(current, &mut file)?;
            staged = Some(file);
            Ok(hash)
        })?;
        let target = binary_path(root, &hash);
        let directory = target
            .parent()
            .context("Missing managed binary directory")?;
        store::ensure_replaceable(directory)?;
        store::ensure_replaceable(&target)?;
        if target.exists() {
            ensure!(
                catalog::digest(&target)? == hash,
                "Permanent binary was modified"
            );
            return Ok(target);
        }
        if !directory.exists() {
            crate::platform::private_dir(directory)?;
        }
        let file = match staged {
            Some(file) => file,
            None => {
                let mut file = temporary.open(directory)?;
                ensure!(
                    catalog::copy_digest(&std::env::current_exe()?, &mut file)? == hash,
                    "Installer executable changed during publication"
                );
                file
            }
        };
        crate::platform::executable(temporary.path()?)?;
        temporary.publish(file, &target)?;
        Ok(target)
    })
}
pub async fn apply(
    root: &Path,
    registry: &mut Registry,
    options: &Selection,
    selected: &[String],
    target: &Path,
    hash: &str,
) -> Result<Outcome> {
    let mut outcome = Outcome::default();
    let results = &mut outcome.messages;
    for index in 0..registry.gateways.len() {
        let old = registry.gateways[index].clone();
        if !selected.contains(&old.id) {
            continue;
        }
        if old.removing {
            results.push(format!(
                "{}: removal pending; finish remove before setup",
                old.id
            ));
            continue;
        }
        if let Some((message, restart_clients)) =
            migration::apply(root, registry, index, &old, target, hash).await?
        {
            results.push(message);
            if restart_clients {
                outcome
                    .restart
                    .extend(old.bindings.iter().map(|binding| binding.client));
            }
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
        new.binary = target.into();
        new.binary_hash = hash.into();
        let mut journal = store::Journal {
            format_version: 1,
            phase: "upgrading".into(),
            services: vec![new.clone()],
            restart: vec![old.clone()],
            migration: None,
            changes: vec![],
        };
        journal.save(&path)?;
        #[cfg(target_os = "linux")]
        let stopped = runtime::stop_idle(&old)
            .await
            .with_context(|| format!("Update incomplete. Private journal: {}", path.display()))?;
        #[cfg(not(target_os = "linux"))]
        let stopped = runtime::stop_idle(&old).await.unwrap_or(false);
        if !stopped {
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
        service_label: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn expected_target(root: &Path) -> PathBuf {
        let hash = catalog::current_exe_digest().unwrap();
        root.join("binaries")
            .join(format!("{}-{hash}", env!("CARGO_PKG_VERSION")))
            .join(crate::platform::binary_name())
    }

    #[test]
    fn binary_installs_and_reuses_versioned_copy() {
        let root = tempfile::tempdir().unwrap();
        let target = binary(root.path()).unwrap();
        assert_eq!(target, expected_target(root.path()));
        assert_eq!(binary(root.path()).unwrap(), target);
        assert_eq!(
            fs::read_dir(root.path().join("binaries")).unwrap().count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn binary_rejects_modified_target() {
        let root = tempfile::tempdir().unwrap();
        let target = binary(root.path()).unwrap();
        fs::write(&target, b"modified binary").unwrap();
        assert_eq!(
            binary(root.path()).unwrap_err().to_string(),
            "Permanent binary was modified"
        );
        assert_eq!(fs::read(target).unwrap(), b"modified binary");
    }

    #[cfg(unix)]
    #[test]
    fn binary_refuses_to_replace_a_dangling_symlink() {
        let root = tempfile::tempdir().unwrap();
        let target = expected_target(root.path());
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let missing = root.path().join("missing");
        std::os::unix::fs::symlink(&missing, &target).unwrap();
        assert_eq!(
            binary(root.path()).unwrap_err().to_string(),
            "Refusing to replace a symlink"
        );
        assert_eq!(fs::read_link(target).unwrap(), missing);
    }
}

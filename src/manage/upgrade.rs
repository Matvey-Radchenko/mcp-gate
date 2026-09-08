//! Existing installations retain endpoints, credentials and backend state across upgrades.
use super::{
    Selection, runtime, service,
    store::{self, Record, Registry},
};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

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
pub async fn apply(
    root: &Path,
    registry: &mut Registry,
    options: &Selection,
) -> Result<Vec<String>> {
    let hash = crate::catalog::digest(&std::env::current_exe()?)?;
    let mut results = Vec::new();
    for index in 0..registry.gateways.len() {
        let old = registry.gateways[index].clone();
        if old.removing {
            results.push(format!(
                "{}: removal pending; finish remove before setting it up again",
                old.id
            ));
            continue;
        }
        if !old
            .bindings
            .iter()
            .any(|b| super::matches(options, b.client, &b.name))
        {
            continue;
        }
        let config = crate::config::Config::load(&old.config())?;
        if crate::catalog::Catalog::load(&config.catalog_file, &config.backend).is_err() {
            results.push(format!(
                "{}: backend/catalog changed; remove and setup with discovery required",
                old.id
            ));
            continue;
        }
        if old.binary_hash == hash {
            results.push(format!(
                "{}: already current; no token, service or backend restarted",
                old.id
            ));
            continue;
        }
        if !runtime::maintenance(&old, true).await.unwrap_or(false) {
            results.push(format!(
                "{}: update pending; gateway busy or unreachable",
                old.id
            ));
            continue;
        }
        let mut new = old.clone();
        new.binary = binary(root)?;
        new.binary_hash = hash.clone();
        let pending = root
            .join("operations")
            .join(format!("upgrade-{}.json", old.id));
        store::atomic(
            &pending,
            &serde_json::to_vec(&Upgrade {
                format_version: 1,
                old: old.clone(),
                new: new.clone(),
            })?,
        )?;
        if let Err(error) = replace(&old, &new).await {
            let _ = runtime::maintenance(&old, false).await;
            anyhow::bail!(
                "Upgrade failed; private recovery record {}: {error}",
                pending.display()
            );
        }
        registry.gateways[index] = new;
        store::save(root, registry)
            .context("New gateway retained; registry commit needs recovery")?;
        std::fs::remove_file(pending)?;
        results.push(format!(
            "{}: background binary updated; endpoint and credentials preserved",
            old.id
        ));
    }
    Ok(results)
}
#[derive(serde::Deserialize, serde::Serialize)]
struct Upgrade {
    format_version: u32,
    old: Record,
    new: Record,
}
async fn replace(old: &Record, new: &Record) -> Result<()> {
    service::unregister(old)?;
    let result: Result<()> = async {
        service::register(new)?;
        runtime::ready(new).await
    }
    .await;
    if result.is_ok() {
        return Ok(());
    }
    // A client may have connected while readiness was being checked. Do not interrupt it.
    if service::installed(new).unwrap_or(true) {
        ensure!(
            runtime::maintenance(new, true).await.unwrap_or(false),
            "Replacement gateway retained: active sessions or unknown health; recovery is unsafe"
        );
        service::unregister(new)?;
    }
    service::register(old)?;
    runtime::ready(old).await?;
    result.context("Previous service restored")
}

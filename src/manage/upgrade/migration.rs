//! Select adopted gateways without claiming their external client connections.
use crate::manage::{
    Selection,
    inventory::Connection,
    recovery, runtime, service,
    store::{self, Journal, Record, Registry},
};
use anyhow::{Context, Result, ensure};
use std::path::Path;
use uuid::Uuid;

pub(in crate::manage) fn selected(
    record: &Record,
    options: &Selection,
    connections: &[Connection],
) -> bool {
    if !record.bindings.is_empty() {
        return record
            .bindings
            .iter()
            .any(|binding| super::super::matches_binding(options, binding));
    }
    (options.client.is_empty() && options.server.is_empty() && options.project.is_none())
        || (options.project.is_none()
            && options
                .server
                .iter()
                .any(|name| name == &record.id || name == &record.label))
        || connections.iter().any(|connection| {
            super::super::inventory::is_external(connection.kind)
                && connection
                    .gateway
                    .as_deref()
                    .is_some_and(|config| Path::new(config) == record.config())
        })
}

fn outside_store(root: &Path, binary: &Path) -> bool {
    let store = root.join("binaries");
    let version_and_hash = binary
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    let valid_directory = version_and_hash
        .and_then(|name| name.rsplit_once('-'))
        .is_some_and(|(version, hash)| {
            !version.is_empty()
                && hash.len() == 64
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
    binary.parent().and_then(Path::parent) != Some(store.as_path())
        || !valid_directory
        || binary
            .file_name()
            .is_none_or(|name| name != crate::platform::binary_name())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::manage) enum BinaryState {
    Current,
    Migration,
    Upgrade,
}

pub(in crate::manage) fn binary_state(
    root: &Path,
    record: &Record,
    target: &Path,
    hash: &str,
    observed: Option<&Path>,
) -> BinaryState {
    let actual = observed.unwrap_or(&record.binary);
    if actual == target && record.binary == target && record.binary_hash == hash {
        BinaryState::Current
    } else if record.bindings.is_empty()
        || outside_store(root, &record.binary)
        || outside_store(root, actual)
    {
        BinaryState::Migration
    } else {
        BinaryState::Upgrade
    }
}

pub(super) async fn apply(
    root: &Path,
    registry: &mut Registry,
    index: usize,
    old: &Record,
    target: &Path,
    hash: &str,
) -> Result<Option<(String, bool)>> {
    let legacy_record = outside_store(root, &old.binary);
    let unbound = old.bindings.is_empty();
    let snapshot = service::snapshot(old)?;
    let Some(snapshot) = snapshot else {
        ensure!(
            !legacy_record && !unbound,
            "Gateway {} has no service definition to migrate; restore it before setup",
            old.id
        );
        return Ok(None);
    };
    let state = binary_state(root, old, target, hash, Some(&snapshot.binary));
    if state == BinaryState::Current {
        if unbound {
            ensure!(
                crate::catalog::digest(target)? == hash,
                "Permanent binary was modified"
            );
            return Ok(Some((
                format!(
                    "{}: already current; no token, service or backend restarted",
                    old.id
                ),
                false,
            )));
        }
        return Ok(None);
    }
    let legacy = state == BinaryState::Migration;
    let config = crate::config::Config::load(&old.config())?;
    // Keep catalog discovery and review in the established upgrade path
    // for a bound gateway with a backend that has changed.
    if !legacy
        && (snapshot.binary == target
            || crate::catalog::Catalog::load(&config.catalog_file, &config.backend).is_err())
    {
        return Ok(None);
    }
    let mut new = old.clone();
    new.binary = super::binary(root)?;
    new.binary_hash = hash.into();
    let change = service::BinaryChange::prepare(snapshot, &new.binary)?;
    let path = root
        .join("operations")
        .join(format!("upgrade-{}.json", Uuid::new_v4()));
    let mut journal = Journal {
        format_version: 1,
        phase: "upgrading".into(),
        services: vec![new.clone()],
        restart: vec![old.clone()],
        changes: Vec::new(),
        migration: Some(change),
    };
    journal.save(&path)?;
    if !swap(&mut journal, &path, old, &new).await? {
        let action = if legacy { "migration" } else { "update" };
        return Ok(Some((
            format!(
                "{}: {action} pending; gateway busy or health unknown",
                old.id
            ),
            false,
        )));
    }
    registry.gateways[index] = new;
    if let Err(error) = commit_registry(&mut journal, &path, root, registry) {
        return Err(restore_error(&mut journal, &path, error).await?);
    }
    let message = if legacy {
        let clients = if unbound {
            "; no saved bindings, client settings left alone"
        } else {
            "; client settings preserved"
        };
        format!(
            "{}: gateway binary migrated to the managed store{clients}",
            old.id
        )
    } else {
        format!(
            "{}: gateway updated; endpoint and credentials preserved",
            old.id
        )
    };
    Ok(Some((message, !legacy)))
}

fn commit_registry(
    journal: &mut Journal,
    path: &Path,
    root: &Path,
    registry: &Registry,
) -> Result<()> {
    let target = root.join("registry.json");
    journal.write(
        path,
        &target,
        store::read_optional(&target)?,
        serde_json::to_vec_pretty(registry)?,
    )?;
    journal.finish_migration(path, "complete")
}

async fn swap(journal: &mut Journal, path: &Path, old: &Record, new: &Record) -> Result<bool> {
    let change = journal
        .migration
        .as_ref()
        .context("Missing service migration")?;
    let result = async {
        if !change.stop(old).await? {
            return Ok(false);
        }
        super::super::checkpoint("upgrade-stopped")?;
        change.activate(new)?;
        super::super::checkpoint("upgrade-registered")?;
        runtime::ready(new).await?;
        super::super::checkpoint("upgrade-ready")?;
        Ok(true)
    }
    .await;
    match result {
        Ok(true) => Ok(true),
        Ok(false) => {
            journal.finish_migration(path, "complete")?;
            Ok(false)
        }
        Err(error) => Err(restore_error(journal, path, error).await?),
    }
}

async fn restore_error(
    journal: &mut Journal,
    path: &Path,
    error: anyhow::Error,
) -> Result<anyhow::Error> {
    let issues = recovery::restore(journal, path).await?;
    let recovery = if issues.is_empty() {
        "previous settings/service restored".into()
    } else {
        issues.join("; ")
    };
    Ok(anyhow::anyhow!(
        "Migration failed: {error}. Recovery: {recovery}. Private journal: {}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::Client;
    use std::path::PathBuf;

    fn record() -> Record {
        Record {
            removing: false,
            id: "historical-id".into(),
            release: PathBuf::from("release"),
            label: "local.mcp-gate.historical-id".into(),
            binary: PathBuf::from(crate::platform::binary_name()),
            binary_hash: "same bytes".into(),
            bindings: vec![],
        }
    }

    #[test]
    fn unbound_gateway_is_selectable_without_claiming_an_external_connection() {
        let record = record();
        let mut options = Selection::default();
        assert!(selected(&record, &options, &[]));
        options.server = vec!["external-fixture".into()];
        options.client = vec![Client::Codex];
        assert!(!selected(&record, &options, &[]));
        let mut connection = Connection {
            client: Client::Codex,
            name: "external-fixture".into(),
            source: PathBuf::new(),
            project: None,
            kind: "external",
            state: "not checked",
            reason: None,
            gateway: Some(record.config().display().to_string()),
        };
        assert!(selected(
            &record,
            &options,
            std::slice::from_ref(&connection)
        ));
        connection.gateway = Some("different/config.toml".into());
        assert!(!selected(&record, &options, &[connection]));
        options.server = vec![record.label.clone()];
        assert!(selected(&record, &options, &[]));
        assert!(record.bindings.is_empty());
    }

    #[test]
    fn current_bytes_need_migration_until_registry_and_service_use_the_store() {
        let root = Path::new("installation");
        let hash = "0".repeat(64);
        let target = super::super::binary_path(root, &hash);
        let mut record = record();
        let legacy = record.binary.clone();
        record.binary_hash = hash.clone();
        assert!(
            binary_state(root, &record, &target, &hash, Some(&legacy)) == BinaryState::Migration
        );
        record.binary = target.clone();
        assert!(
            binary_state(root, &record, &target, &hash, Some(&legacy)) == BinaryState::Migration
        );
        assert!(binary_state(root, &record, &target, &hash, Some(&target)) == BinaryState::Current);
        assert!(outside_store(
            root,
            &root
                .join("binaries/legacy")
                .join(crate::platform::binary_name())
        ));
    }
}

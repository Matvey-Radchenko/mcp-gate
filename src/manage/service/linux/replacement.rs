//! Replace the effective ExecStart without touching enablement or other directives.
use super::{Record, Unit, location, manager, regular_file, unit_file, unit_name};
use crate::manage::service::definition::{Definition, Snapshot};
use anyhow::{Context, Result, ensure};
use std::path::Path;

pub(in crate::manage::service) fn snapshot(record: &Record) -> Result<Option<Snapshot>> {
    let unit = unit_name(record);
    let current = Unit::query(&unit)?;
    if current.gone() {
        return Ok(None);
    }
    ensure!(
        current.get("Id") == unit
            && current.get("LoadState") == "loaded"
            && current.fragment() == location(&unit)?
            && regular_file(current.fragment())?,
        "Systemd gateway does not own its expected user unit"
    );
    // During recovery the fragment can already contain the replacement while
    // the stopped manager still has the previous unit loaded.
    let (identity, path, text) =
        unit_file::effective(current.fragment(), current.get("DropInPaths"))
            .context("Cannot identify the systemd gateway ExecStart; values hidden")?;
    ensure!(
        identity.config == record.config(),
        "Systemd service configuration no longer matches the registry"
    );
    let drop_in = current.fragment().with_file_name(format!("{unit}.d"));
    ensure!(
        regular_file(&path)?
            && (path == current.fragment()
                || (path.parent() == Some(drop_in.as_path())
                    && drop_in.canonicalize()? == drop_in)),
        "Effective ExecStart is outside this gateway's user unit or its drop-ins"
    );
    Ok(Some(Snapshot {
        binary: identity.binary,
        definition: Definition {
            path: Some(path),
            bytes: text.into_bytes(),
        },
    }))
}

pub(in crate::manage::service) fn rewrite(
    definition: &Definition,
    old: &Path,
    target: &Path,
) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(&definition.bytes).context("Invalid UTF-8 in systemd unit")?;
    Ok(unit_file::with_binary(text, old, target)?.into_bytes())
}

pub(in crate::manage::service) fn stop(record: &Record) -> Result<()> {
    let unit = unit_name(record);
    if !Unit::query(&unit)?.stopped() {
        manager::run(&["stop", &unit])?;
    }
    ensure!(
        Unit::query(&unit)?.stopped(),
        "Systemd gateway is still stopping"
    );
    Ok(())
}

pub(in crate::manage::service) fn start(record: &Record) -> Result<()> {
    let unit = unit_name(record);
    let current = Unit::query(&unit)?;
    if current.stopped() || current.get("NeedDaemonReload") != "no" {
        manager::run(&["daemon-reload"])?;
    }
    let selected = Unit::query(&unit)?;
    let expected = snapshot(record)?.context("Systemd gateway definition disappeared")?;
    ensure!(
        selected
            .identity()
            .is_some_and(|identity| identity.binary == expected.binary),
        "Systemd did not load the migrated gateway definition"
    );
    if selected.stopped() {
        manager::start(&unit)?;
    }
    Ok(())
}

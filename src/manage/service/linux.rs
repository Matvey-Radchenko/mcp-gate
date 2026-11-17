//! Own only the matching fragment in the user's XDG systemd directory.
mod manager;
mod unit_file;

use super::{Record, label_taken, shell_literal};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

pub(in crate::manage) use manager::available;
pub(super) use manager::collision;
pub(in crate::manage) use unit_file::Identity;

const STATE: &str = "Id,LoadState,ActiveState,UnitFileState,MainPID,ControlPID,FragmentPath,DropInPaths,NeedDaemonReload";

struct Unit {
    name: String,
    fields: manager::Properties,
}

impl Unit {
    fn query(name: &str) -> Result<Self> {
        let fields = manager::show(name, STATE)?;
        ensure!(
            fields.contains_key("LoadState") && fields.contains_key("ActiveState"),
            "Missing unit state from systemd --user"
        );
        Ok(Self {
            name: name.into(),
            fields,
        })
    }

    fn get(&self, key: &str) -> &str {
        self.fields.get(key).map_or("", String::as_str)
    }

    fn stopped(&self) -> bool {
        matches!(self.get("ActiveState"), "inactive" | "failed")
            && ["MainPID", "ControlPID"]
                .iter()
                .all(|key| matches!(self.get(key), "" | "0"))
    }

    fn gone(&self) -> bool {
        self.get("LoadState") == "not-found" && self.stopped()
    }

    fn fragment(&self) -> &Path {
        Path::new(self.get("FragmentPath"))
    }

    fn identity(&self) -> Option<Identity> {
        if self.get("LoadState") != "loaded"
            || self.get("Id") != self.name
            || self.get("NeedDaemonReload") != "no"
        {
            return None;
        }
        unit_file::identity(self.fragment(), self.get("DropInPaths"))
    }

    fn matches(&self, record: &Record) -> bool {
        self.identity()
            .is_some_and(|identity| identity.matches(record))
    }

    fn state(&self, registered: bool) -> ServiceState {
        ServiceState {
            registered,
            enabled: registered
                && matches!(self.get("UnitFileState"), "enabled" | "enabled-runtime"),
            active: registered
                && matches!(
                    self.get("ActiveState"),
                    "active" | "reloading" | "refreshing"
                ),
        }
    }
}

#[derive(Serialize)]
pub(in crate::manage) struct ServiceState {
    pub registered: bool,
    pub enabled: bool,
    pub active: bool,
}

fn unit_name(record: &Record) -> String {
    format!("{}.service", record.label)
}

pub(in crate::manage) fn label(unit: &str) -> Option<&str> {
    let label = unit.strip_suffix(".service")?;
    let name = label.strip_prefix("local.mcp-gate.")?;
    super::super::agents::valid_name(name).then_some(label)
}

fn location(unit: &str) -> Result<PathBuf> {
    ensure!(label(unit).is_some(), "Invalid systemd user service label");
    let directory =
        super::super::store::xdg_home("XDG_CONFIG_HOME", ".config")?.join("systemd/user");
    // Resolve directory aliases without following a symlink at the unit path.
    match directory.canonicalize() {
        Ok(directory) => Ok(directory.join(unit)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(directory.join(unit)),
        Err(error) => Err(error).context("Cannot resolve systemd user unit directory"),
    }
}

pub(super) fn register(record: &Record) -> Result<()> {
    let unit = unit_name(record);
    let path = location(&unit)?;
    if metadata(&path)?.is_some() || !Unit::query(&unit)?.gone() {
        return Err(label_taken(&record.label));
    }
    let directory = path
        .parent()
        .context("Missing systemd user unit directory")?;
    fs::create_dir_all(directory)?;
    let path = location(&unit)?;
    let bytes = unit_file::render(record)?;
    write_new(&path, bytes.as_bytes(), &record.label)?;

    // Leave the fragment for journal recovery on errors after publishing it.
    // A reload may pick a higher-precedence fragment installed concurrently.
    manager::run(&["daemon-reload"])?;
    let selected = Unit::query(&unit)?;
    if selected.get("LoadState") == "masked"
        || selected.get("UnitFileState") == "transient"
        || !selected.fragment().as_os_str().is_empty() && selected.fragment() != path
    {
        discard_unstarted(&path, &bytes)?;
        return Err(label_taken(&record.label));
    }
    if selected.fragment() != path || !selected.matches(record) {
        discard_unstarted(&path, &bytes)?;
        anyhow::bail!(
            "The systemd user manager did not load the expected unit from {}",
            path.display()
        );
    }
    if let Err(error) = manager::enable(&unit) {
        if super::label_in_use(&error) {
            // The enable this unit requested has only one installation link.
            // Leave the conflicting link alone and withdraw our fragment.
            discard_unstarted(&path, &bytes)?;
        }
        return Err(error);
    }
    Ok(())
}

fn discard_unstarted(path: &Path, bytes: &str) -> Result<()> {
    ensure!(
        regular_file(path)? && fs::read_to_string(path)? == bytes,
        "Systemd unit file changed during registration"
    );
    fs::remove_file(path)?;
    manager::run(&["daemon-reload"])?;
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8], label: &str) -> Result<()> {
    let temporary = path.with_file_name(format!(".mcp-gate-{}.tmp", uuid::Uuid::new_v4()));
    let mut file = crate::platform::private_file(&temporary)?;
    // Publish only a complete fragment, with the same no-clobber guarantee
    // as create_new even if a second installer claims the path during reload.
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let linked = written.and_then(|()| fs::hard_link(&temporary, path));
    fs::remove_file(temporary)?;
    match linked {
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Err(label_taken(label)),
        result => result.context("Cannot write systemd user unit")?,
    }
    fs::File::open(
        path.parent()
            .context("Missing systemd user unit directory")?,
    )?
    .sync_all()?;
    Ok(())
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("Cannot inspect systemd user unit path"),
    }
}

fn regular_file(path: &Path) -> Result<bool> {
    Ok(metadata(path)?.is_some_and(|metadata| metadata.is_file()))
}

pub(super) fn unregister(record: &Record) -> Result<()> {
    let unit = unit_name(record);
    let path = location(&unit)?;
    let current = Unit::query(&unit)?;
    if metadata(&path)?.is_none() {
        if current.gone() {
            return Ok(());
        }
        // A previous attempt may have removed the file before reloading.
        // Never send stop/disable without proving ownership of the definition.
        ensure!(
            current.fragment() == path && current.stopped(),
            "Systemd user service is still present without its expected unit file"
        );
    } else {
        ensure!(
            regular_file(&path)?,
            "Systemd user unit path is not a regular file"
        );
        let contents = fs::read_to_string(&path)?;
        let owned = if current.gone() {
            contents == unit_file::render(record)?
        } else {
            current.fragment() == path
                && current
                    .identity()
                    .is_some_and(|identity| identity.matches(record))
                && unit_file::identity(&path, "").is_some_and(|identity| identity.matches(record))
        };
        ensure!(
            owned,
            "Systemd user service no longer matches its recorded unit"
        );
        if !current.gone() {
            manager::run(&["stop", &unit])?;
            let stopped = Unit::query(&unit)?;
            ensure!(
                stopped.fragment() == path
                    && stopped.stopped()
                    && stopped
                        .identity()
                        .is_some_and(|identity| identity.matches(record)),
                "Systemd user service changed or is still stopping"
            );
            manager::run(&["disable", &unit])?;
        }
        // Refuse to delete a replacement or edits that arrived while stopping.
        ensure!(
            regular_file(&path)? && fs::read_to_string(&path)? == contents,
            "Systemd unit file changed during removal"
        );
        fs::remove_file(&path).context("Cannot remove systemd user unit")?;
    }
    manager::run(&["daemon-reload"])?;
    for _ in 0..100 {
        if Unit::query(&unit)?.gone() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("Service removal is still pending in the systemd user service manager")
}

/// A journal may name a label before create-new, including one claimed by
/// another installer. Only the exact fragment for this release is ours.
pub(in crate::manage) fn rollback_needed(record: &Record) -> Result<bool> {
    let unit = unit_name(record);
    let path = location(&unit)?;
    if regular_file(&path)? {
        return Ok(fs::read_to_string(&path)? == unit_file::render(record)?);
    }
    let current = Unit::query(&unit)?;
    Ok(current.fragment() == path && current.stopped())
}

pub(in crate::manage) fn state(record: &Record) -> Result<ServiceState> {
    let unit = unit_name(record);
    ensure!(label(&unit).is_some(), "Invalid systemd user service label");
    let current = Unit::query(&unit)?;
    Ok(current.state(current.matches(record)))
}

/// Resume an old installation left stopped between stop and file deletion.
pub(in crate::manage) fn resume_registered(record: &Record) -> Result<bool> {
    let unit = unit_name(record);
    let path = location(&unit)?;
    let current = Unit::query(&unit)?;
    if !current.matches(record) {
        return Ok(false);
    }
    if current.stopped() {
        ensure!(
            current.fragment() == path && regular_file(&path)?,
            "The previous systemd user service no longer owns its expected unit file"
        );
        manager::enable(&unit)?;
    }
    Ok(true)
}

pub(in crate::manage) fn definitions() -> Result<Vec<(String, Identity, ServiceState)>> {
    crate::manage::trace::log("linux::definitions: units start");
    let mut definitions = Vec::new();
    for unit in manager::units()? {
        crate::manage::trace::log(&format!("linux::definitions: query {unit}"));
        let current = Unit::query(&unit)?;
        if let Some(identity) = current.identity()
            && let Some(label) = label(&unit)
        {
            definitions.push((label.to_owned(), identity, current.state(true)));
        }
    }
    Ok(definitions)
}

pub(in crate::manage) fn labels() -> Result<impl Iterator<Item = String>> {
    Ok(manager::units()?
        .into_iter()
        .filter_map(|unit| label(&unit).map(str::to_owned)))
}

pub(super) fn restart_command(record: &Record) -> String {
    format!(
        "systemctl --user restart {}",
        shell_literal(&unit_name(record))
    )
}

pub(super) fn log_hint(record: &Record) -> String {
    format!(
        "Inspect service logs with journalctl --user -u {}",
        shell_literal(&unit_name(record))
    )
}

pub(super) fn diagnostics(record: &Record) -> String {
    let unit = unit_name(record);
    if label(&unit).is_some()
        && let Ok(fields) = manager::show(&unit, "ActiveState,Result,ExecMainStatus")
    {
        let safe: Vec<_> = ["ActiveState", "Result", "ExecMainStatus"]
            .into_iter()
            .filter_map(|key| {
                let value = fields.get(key)?;
                (!value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
                .then(|| format!("{key}={value}"))
            })
            .collect();
        if !safe.is_empty() {
            return safe.join("; ");
        }
    }
    "systemd --user service state unavailable".into()
}

#[cfg(test)]
mod tests;

//! Inspect and edit a LaunchAgent with plutil so binary plists and typed keys survive.
use super::{
    Record, agent_path,
    definition::{Definition, Snapshot},
    domain,
};
use crate::manage::store;
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
};

pub(in crate::manage) fn identity(path: &Path) -> Result<(String, PathBuf, PathBuf)> {
    let label = extract(path, "Label", "raw")?.context("LaunchAgent has no Label")?;
    let (binary, config) = invocation(path)?;
    Ok((label, binary, config))
}

pub(in crate::manage) fn invocation(path: &Path) -> Result<(PathBuf, PathBuf)> {
    let args =
        extract(path, "ProgramArguments", "json")?.context("LaunchAgent has no arguments")?;
    let args: Vec<String> = serde_json::from_str(&args)
        .map_err(|_| anyhow::anyhow!("Invalid LaunchAgent arguments; values hidden"))?;
    let program = extract(path, "Program", "raw")?;
    let binary = PathBuf::from(
        program
            .as_ref()
            .or(args.first())
            .context("LaunchAgent has no executable")?,
    );
    let config = super::config_argument(args.iter().skip(1).map(String::as_str))
        .context("LaunchAgent has no literal gateway configuration")?;
    ensure!(
        binary.is_absolute(),
        "LaunchAgent executable must be absolute"
    );
    Ok((binary, config))
}

fn extract(path: &Path, key: &str, format: &str) -> Result<Option<String>> {
    #[cfg(feature = "test-backend")]
    if let Ok(bytes) = fs::read(path)
        && let Ok(document) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        // Old discovery fixtures use JSON plists with the same literal keys.
        let value = key.split('.').try_fold(&document, |value, name| {
            if let Ok(index) = name.parse::<usize>() {
                value.get(index)
            } else {
                value.get(name)
            }
        });
        return Ok(value.and_then(|value| {
            if format == "raw" {
                value.as_str().map(str::to_owned)
            } else {
                Some(value.to_string())
            }
        }));
    }
    let mut command = Command::new("/usr/bin/plutil");
    command.args(["-extract", key, format]);
    if format == "raw" {
        command.args(["-expect", "string", "-n"]);
    }
    let output = command.args(["-o", "-", "--"]).arg(path).output()?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8(output.stdout).context("Invalid LaunchAgent encoding")?,
    ))
}

pub(super) fn snapshot(record: &Record) -> Result<Option<Snapshot>> {
    let path = agent_path(record)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            ensure!(
                !super::installed(record)?,
                "Registered LaunchAgent has no expected plist"
            );
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file(),
        "LaunchAgent plist is not a regular file"
    );
    let (label, binary, config) = identity(&path)?;
    ensure!(
        label == record.label && config == record.config(),
        "LaunchAgent identity no longer matches the registry"
    );
    let loaded = inspect(record)?;
    if loaded.status.success() {
        let text = String::from_utf8_lossy(&loaded.stdout);
        let registered_path = text
            .lines()
            .find_map(|line| line.trim_start().strip_prefix("path = "));
        ensure!(
            registered_path.is_some_and(|entry| Path::new(entry) == path),
            "LaunchAgent is registered from a different plist"
        );
    }
    Ok(Some(Snapshot {
        binary,
        definition: Definition {
            bytes: fs::read(&path)?,
            path: Some(path),
        },
    }))
}

pub(super) fn rewrite(definition: &Definition, old: &Path, target: &Path) -> Result<Vec<u8>> {
    let directory = definition
        .file()?
        .parent()
        .context("Missing LaunchAgent directory")?;
    let target = target
        .to_str()
        .context("LaunchAgent executable must be UTF-8")?;
    store::with_temporary(|temporary| {
        let mut file = temporary.open(directory)?;
        file.write_all(&definition.bytes)?;
        drop(file);
        let path = temporary.path()?;
        let program = extract(path, "Program", "raw")?;
        let first = extract(path, "ProgramArguments.0", "raw")?;
        ensure!(
            program
                .as_ref()
                .or(first.as_ref())
                .is_some_and(|value| Path::new(value) == old),
            "LaunchAgent executable changed"
        );
        if program.is_some() {
            replace(path, "Program", target)?;
        }
        if first.as_ref().is_some_and(|value| Path::new(value) == old) {
            replace(path, "ProgramArguments.0", target)?;
        }
        Ok(fs::read(path)?)
    })
}

fn replace(path: &Path, key: &str, value: &str) -> Result<()> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-replace", key, "-string", value, "--"])
        .arg(path)
        .output()?;
    ensure!(
        output.status.success(),
        "Cannot update LaunchAgent executable; values hidden"
    );
    Ok(())
}

fn target(record: &Record) -> String {
    format!("{}/{}", domain(), record.label)
}

fn inspect(record: &Record) -> Result<Output> {
    Ok(Command::new("launchctl")
        .args(["print", &target(record)])
        .output()?)
}

pub(super) fn stop(record: &Record) -> Result<()> {
    if !inspect(record)?.status.success() {
        return Ok(());
    }
    let output = Command::new("launchctl")
        .args(["bootout", &target(record)])
        .output()?;
    ensure!(
        output.status.success(),
        "LaunchAgent could not be stopped ({})",
        output.status
    );
    for _ in 0..100 {
        if !inspect(record)?.status.success() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("LaunchAgent is still stopping")
}

pub(super) fn start(record: &Record) -> Result<()> {
    if !inspect(record)?.status.success() {
        let output = Command::new("launchctl")
            .args(["bootstrap", &domain()])
            .arg(agent_path(record)?)
            .output()?;
        ensure!(
            output.status.success(),
            "LaunchAgent could not be loaded ({})",
            output.status
        );
        let output = Command::new("launchctl")
            .args(["kickstart", &target(record)])
            .output()?;
        ensure!(
            output.status.success(),
            "LaunchAgent could not be started ({})",
            output.status
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;

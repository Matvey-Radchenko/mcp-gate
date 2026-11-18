//! Read and replace the action of a current-user logon task.
use super::{
    Record,
    definition::{Definition, Snapshot},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use std::{
    io::Write,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};
use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};

#[derive(Deserialize)]
struct Task {
    label: String,
    binary: PathBuf,
    arguments: String,
    xml: Option<String>,
}

impl Task {
    fn config(&self) -> Option<PathBuf> {
        let arguments = arguments(&self.arguments)?;
        super::config_argument(arguments.iter().map(String::as_str))
    }
}

fn arguments(text: &str) -> Option<Vec<String>> {
    // The shell parser applies special rules to argv[0]; supply a dummy name.
    let text: Vec<_> = format!("gateway {text}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut count = 0;
    // SAFETY: text is NUL-terminated and count is writable.
    let values = unsafe { CommandLineToArgvW(text.as_ptr(), &mut count) };
    if values.is_null() {
        return None;
    }
    // SAFETY: the returned array has count NUL-terminated strings and remains
    // allocated until LocalFree. Copy everything before releasing it.
    let result = unsafe {
        let result = (1..count)
            .map(|index| {
                let ptr = *values.add(index as usize);
                let end = (0..).find(|&index| *ptr.add(index) == 0)?;
                let word = std::ffi::OsString::from_wide(std::slice::from_raw_parts(ptr, end));
                word.into_string().ok()
            })
            .collect();
        LocalFree(values.cast());
        result
    };
    result
}

fn command(mode: &str) -> Command {
    static SCRIPT: OnceLock<String> = OnceLock::new();
    let encoded = SCRIPT.get_or_init(|| {
        let bytes: Vec<_> = include_str!("scheduler.ps1")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        STANDARD.encode(bytes)
    });
    let mut command = Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", encoded]);
    command
        .env("MCP_GATE_TASK_MODE", mode)
        .env_remove("MCP_GATE_TASK_LABEL");
    command
}

fn run(command: &mut Command, input: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Cannot run Task Scheduler adapter")?;
    // The adapter consumes the complete document before emitting any output.
    let written = if let Some(bytes) = input {
        child
            .stdin
            .take()
            .context("Task Scheduler input unavailable")?
            .write_all(bytes)
    } else {
        Ok(())
    };
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "Task Scheduler operation failed ({}); values hidden",
        output.status
    );
    written?;
    Ok(output.stdout)
}

fn tasks(label: Option<&str>) -> Result<Vec<Task>> {
    let mut command = command(if label.is_some() {
        "inspect"
    } else {
        "discover"
    });
    if let Some(label) = label {
        command.env("MCP_GATE_TASK_LABEL", label);
    }
    serde_json::from_slice(&run(&mut command, None)?)
        .map_err(|_| anyhow::anyhow!("Invalid Task Scheduler response; values hidden"))
}

pub(in crate::manage) fn definitions() -> Result<Vec<(String, PathBuf, PathBuf)>> {
    Ok(tasks(None)?
        .into_iter()
        .filter_map(|task| {
            let config = task.config()?;
            Some((task.label, task.binary, config))
        })
        .collect())
}

pub(super) fn snapshot(record: &Record) -> Result<Option<Snapshot>> {
    let Some(task) = tasks(Some(&record.label))?.pop() else {
        return Ok(None);
    };
    ensure!(
        task.binary.is_absolute() && task.config().as_deref() == Some(record.config().as_path()),
        "Task Scheduler action no longer matches the registry"
    );
    Ok(Some(Snapshot {
        binary: task.binary,
        definition: Definition {
            path: None,
            bytes: task
                .xml
                .context("Task definition unavailable")?
                .into_bytes(),
        },
    }))
}

pub(super) fn rewrite(definition: &Definition, old: &Path, target: &Path) -> Result<Vec<u8>> {
    run(
        command("rewrite")
            .env("MCP_GATE_TASK_OLD", old)
            .env("MCP_GATE_TASK_BINARY", target),
        Some(&definition.bytes),
    )
}

pub(super) fn publish(record: &Record, bytes: &[u8]) -> Result<()> {
    run(
        command("publish").env("MCP_GATE_TASK_LABEL", &record.label),
        Some(bytes),
    )?;
    Ok(())
}

pub(super) fn stop(record: &Record) -> Result<()> {
    run(
        command("stop").env("MCP_GATE_TASK_LABEL", &record.label),
        None,
    )?;
    Ok(())
}

pub(super) fn start(record: &Record) -> Result<()> {
    run(
        command("start").env("MCP_GATE_TASK_LABEL", &record.label),
        None,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_only_the_task_command_and_hides_xml_errors() {
        let old = Path::new(r"C:\legacy path\mcp-gate.exe");
        let target = Path::new(r"C:\managed & store\mcp-gate.exe");
        let xml = r#"<?xml version="1.0" encoding="UTF-16"?><Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers><Actions><Exec><Command>C:\legacy path\mcp-gate.exe</Command><Arguments>serve --config "C:\state folder\config.toml" --header "private fixture"</Arguments><WorkingDirectory>C:\working dir</WorkingDirectory></Exec></Actions></Task>"#;
        let definition = Definition {
            path: None,
            bytes: xml.as_bytes().to_vec(),
        };
        let updated = String::from_utf8(rewrite(&definition, old, target).unwrap()).unwrap();
        assert_eq!(
            updated,
            xml.replace(
                r"C:\legacy path\mcp-gate.exe",
                r"C:\managed &amp; store\mcp-gate.exe"
            )
        );
        let task = Task {
            label: "local.mcp-gate.fixture".into(),
            binary: old.into(),
            arguments: r#"serve --config "C:\state folder\config.toml" --header "private fixture""#
                .into(),
            xml: None,
        };
        assert_eq!(
            task.config().unwrap(),
            Path::new(r"C:\state folder\config.toml")
        );
        let error = rewrite(&definition, target, old).unwrap_err();
        assert!(!format!("{error:#}").contains("private fixture"));
    }
}

//! Bounded availability probe and sanitized systemctl requests.
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn command() -> Command {
    let mut command = Command::new("systemctl");
    command
        .args(["--user", "--no-pager"])
        .env("LC_ALL", "C")
        .env("SYSTEMD_COLORS", "0")
        .stdin(Stdio::null());
    command
}

pub(in crate::manage) fn available() -> Result<()> {
    crate::manage::trace::log("systemctl: probe start (show Version, 2s bound)");
    let started = Instant::now();
    let mut child = command()
        .args(["show", "--property=Version", "--value"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Cannot run systemctl")?;
    let deadline = Instant::now() + Duration::from_secs(2);
    // Version fits in the pipe. Only this process owns the probe; a timed-out
    // probe is killed and reaped without sending anything to the manager.
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => {
                let killed = child.kill();
                child.wait().context("Could not reap the systemctl probe")?;
                killed.context("Could not stop the systemctl probe")?;
                result.context("Could not wait for the systemctl probe")?;
                crate::manage::trace::log(&format!(
                    "systemctl: probe timed out after {}ms",
                    started.elapsed().as_millis()
                ));
                bail!("systemd --user manager probe timed out");
            }
        }
    }
    let output = child
        .wait_with_output()
        .context("Cannot read systemctl response")?;
    crate::manage::trace::log(&format!(
        "systemctl: probe done exit={} duration_ms={}",
        output.status,
        started.elapsed().as_millis()
    ));
    ensure!(
        output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "Cannot reach the systemd --user manager"
    );
    Ok(())
}

pub(super) fn run(args: &[&str]) -> Result<String> {
    let output = invoke(args)?;
    check(args[0], &output)?;
    String::from_utf8(output.stdout).map_err(|_| anyhow::anyhow!("Invalid systemctl response"))
}

pub(super) fn enable(unit: &str) -> Result<()> {
    let output = invoke(&["enable", "--now", unit])?;
    if !output.status.success()
        && String::from_utf8_lossy(&output.stderr)
            .lines()
            .any(collision)
    {
        return Err(super::super::label_taken(
            super::label(unit).unwrap_or(unit),
        ));
    }
    check("enable --now", &output)
}

fn invoke(args: &[&str]) -> Result<Output> {
    let label = format!("systemctl --user {}", args.join(" "));
    crate::manage::trace::log(&format!("{label}: start"));
    let started = Instant::now();
    let result = command().args(args).output();
    let duration_ms = started.elapsed().as_millis();
    match &result {
        Ok(output) => crate::manage::trace::log(&format!(
            "{label}: exit={} duration_ms={duration_ms}",
            output.status
        )),
        Err(error) => {
            crate::manage::trace::log(&format!("{label}: error={error} duration_ms={duration_ms}"))
        }
    }
    result.context("Cannot run systemctl")
}

fn check(action: &str, output: &Output) -> Result<()> {
    // systemctl may include arbitrary unit text in a parser error.
    ensure!(
        output.status.success(),
        "systemctl --user {action} failed ({}); check the user service manager",
        output.status
    );
    Ok(())
}

pub(in crate::manage::service) fn collision(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    let line = line.trim();
    let unit_error = line.strip_prefix("failed to enable unit: ").unwrap_or(line);
    let occupied = unit_error.starts_with("unit ")
        && [" is masked", " is transient or generated"]
            .iter()
            .any(|suffix| unit_error.trim_end_matches('.').ends_with(suffix));
    let exists = line
        .strip_prefix("failed to enable unit: ")
        .is_some_and(|detail| {
            let detail = detail.trim_end_matches('.');
            detail == "file exists"
                || detail.ends_with(": file exists")
                || (detail.starts_with("file ") && detail.contains(" already exists"))
        });
    occupied || exists
}

pub(super) type Properties = BTreeMap<String, String>;

pub(super) fn show(unit: &str, properties: &str) -> Result<Properties> {
    let output = invoke(&["show", "--all", "--property", properties, unit])?;
    let fields = std::str::from_utf8(&output.stdout)
        .map_err(|_| anyhow::anyhow!("Invalid systemctl response"))?
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect::<Properties>();
    // Some systemd releases exit nonzero for a missing unit while still
    // reporting LoadState explicitly. A bus error has no such state.
    if fields
        .get("LoadState")
        .is_none_or(|value| value != "not-found")
    {
        check("show", &output)?;
    }
    Ok(fields)
}

pub(super) fn units() -> Result<BTreeSet<String>> {
    crate::manage::trace::log("systemctl: units start");
    available()?;
    let pattern = "local.mcp-gate.*.service";
    let files = run(&["list-unit-files", "--no-legend", "--full", pattern])?;
    let loaded = run(&["list-units", "--all", "--no-legend", "--full", pattern])?;
    crate::manage::trace::log("systemctl: units listed");
    Ok(files
        .lines()
        .chain(loaded.lines())
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let first = columns.next()?;
            Some(if first == "●" {
                columns.next()?
            } else {
                first
            })
        })
        .filter(|unit| super::label(unit).is_some())
        .map(str::to_owned)
        .collect())
}

//! Edit only the persisted idle policy. Never contact a running gateway, read
//! authentication files, or change client settings and service registrations.
use super::{service, store, ui};
use crate::config::Config;
use anyhow::{Context, Result, ensure};
use clap::Args;
use fs2::FileExt;
use serde_json::json;
use std::{fs, path::Path};

#[derive(Args)]
pub struct Idle {
    /// Managed gateway ID, service label/suffix, or unambiguous MCP server name.
    pub name: String,
    /// Seconds, or an integer with s/m/h suffix; off (or 0) disables expiry.
    /// Omit to show the configured value and effective ownership default.
    #[arg(value_parser = parse_duration, value_name = "DURATION|off")]
    pub duration: Option<u64>,
    /// Show the proposed configuration change without writing or restarting.
    #[arg(long)]
    pub dry_run: bool,
    /// Save this gateway's setting without prompting; never restart it.
    #[arg(long)]
    pub yes: bool,
    /// Set from the global --json flag.
    #[arg(skip)]
    pub json: bool,
}

fn parse_duration(value: &str) -> Result<u64, String> {
    if value == "off" {
        return Ok(0);
    }
    let (digits, multiplier) = match value.as_bytes().last() {
        Some(b's') => (&value[..value.len() - 1], 1),
        Some(b'm') => (&value[..value.len() - 1], 60),
        Some(b'h') => (&value[..value.len() - 1], 3600),
        _ => (value, 1),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Use whole seconds, an integer with s/m/h suffix, or off".into());
    }
    digits
        .parse::<u64>()
        .ok()
        .and_then(|seconds| seconds.checked_mul(multiplier))
        .filter(|seconds| *seconds <= i64::MAX as u64)
        .ok_or_else(|| "Duration exceeds the TOML integer limit".into())
}

pub fn run(options: Idle) -> Result<()> {
    let root = store::root()?;
    let registry = store::load(&root)?;
    let record = registry.select(&options.name)?;
    ensure!(
        !record.removing,
        "Gateway removal is pending; finish remove first"
    );
    let before = fs::read(record.config()).context("Cannot read gateway configuration")?;
    let mut config: Config = std::str::from_utf8(&before)
        .ok()
        .and_then(|text| toml::from_str(text).ok())
        .context("Invalid gateway configuration; values hidden")?;
    config.validate()?;
    let previous = config.idle_timeout().map_or(0, |idle| idle.as_secs());
    let Some(seconds) = options.duration else {
        report(&options, record, "current", &config, previous);
        return Ok(());
    };
    config.idle_seconds = Some(seconds);
    config.validate()?;
    let after = edit(&before, seconds)?;
    let phase = if after == before {
        "unchanged"
    } else {
        "complete"
    };
    if options.dry_run || phase == "unchanged" {
        report(
            &options,
            record,
            if options.dry_run { "preview" } else { phase },
            &config,
            previous,
        );
        return Ok(());
    }
    ensure!(
        options.yes || (!options.json && ui::terminal()),
        "No changes applied. Use idle --dry-run to preview or idle --yes to save this gateway's setting"
    );
    if !options.yes {
        report(&options, record, "preview", &config, previous);
        if !super::confirm("Save this idle setting?", false)? {
            ui::canceled(options.json);
            return Ok(());
        }
    }
    apply(&root, record, &before, &after)?;
    report(&options, record, phase, &config, previous);
    Ok(())
}

fn edit(before: &[u8], seconds: u64) -> Result<Vec<u8>> {
    // Change one root field, preserving backend/client paths, comments and table order.
    let mut document: toml_edit::DocumentMut = std::str::from_utf8(before)?
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid gateway configuration; values hidden"))?;
    let table = document.as_table_mut();
    // Config validation guarantees at most one of these keys, with a scalar value.
    let previous = [
        "idle_seconds",
        "session_idle_seconds",
        "shared_idle_seconds",
    ]
    .into_iter()
    .find_map(|name| table.remove_entry(name));
    let mut key = toml_edit::Key::new("idle_seconds");
    let mut value = toml_edit::Value::from(i64::try_from(seconds)?);
    if let Some((old_key, old_value)) = previous {
        key = key.with_leaf_decor(old_key.leaf_decor().clone());
        *value.decor_mut() = old_value
            .as_value()
            .context("Idle setting must be a scalar")?
            .decor()
            .clone();
    }
    table.insert_formatted(&key, toml_edit::Item::Value(value));
    Ok(document.to_string().into_bytes())
}

fn apply(root: &Path, record: &store::Record, before: &[u8], after: &[u8]) -> Result<()> {
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("setup.lock"))?;
    lock.try_lock_exclusive()
        .context("Another setup/remove/idle operation is running")?;
    let registry = store::load(root)?;
    ensure!(
        registry
            .gateways
            .iter()
            .any(|current| current.id == record.id
                && current.config() == record.config()
                && current.label == record.label
                && !current.removing),
        "Managed gateway changed during review; rerun idle"
    );
    ensure!(
        fs::read(record.config())? == before,
        "Configuration changed during review; no write performed. Rerun idle"
    );
    // One atomic replacement under the setup/remove lock; there are no service
    // side effects or multi-file changes requiring an installer recovery journal.
    store::atomic(&record.config(), after)
}

fn report(options: &Idle, record: &store::Record, phase: &str, config: &Config, previous: u64) {
    let seconds = config.idle_timeout().map_or(0, |idle| idle.as_secs());
    let value = if seconds == 0 {
        "off".into()
    } else {
        format!("{seconds}s")
    };
    let restart = service::restart_command(record);
    let message = match phase {
        "current" => {
            "Configured idle policy; the running service may still use its previous setting."
        }
        "preview" if options.dry_run => {
            "Dry run. The configuration and running service stay as they are."
        }
        "preview" => "Proposed idle policy. Saving it requires a later gateway restart.",
        "unchanged" => {
            "Idle setting is already saved. The running service may still need a restart."
        }
        _ => "Idle setting saved. Restart required to apply it to the running service.",
    };
    if options.json {
        println!(
            "{}",
            json!({"format_version":1,"phase":phase,"gateway":record.id,
            "config":record.config(),"ownership":config.ownership,"idle_seconds":seconds,
            "uses_ownership_default":config.idle_seconds.is_none(),"message":message,
            "previous_idle_seconds":previous,
            "restart_command":restart,"restart_required":phase != "current"})
        );
    } else {
        println!(
            "Gateway {}: idle {value}{}",
            ui::safe(&record.id),
            if config.idle_seconds.is_none() {
                " (ownership default)"
            } else {
                ""
            }
        );
        if seconds != previous {
            let previous = if previous == 0 {
                "off".into()
            } else {
                format!("{previous}s")
            };
            println!("  Previous idle setting: {previous}");
        }
        println!("{message}");
        println!("After current actions finish, restart the gateway: {restart}");
    }
}

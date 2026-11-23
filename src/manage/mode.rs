//! Review and save an explicit ownership decision for one managed gateway.
mod plan;

use super::{config_edit, service, store, ui};
use crate::{
    advisor::{self, Advice, Recommendation},
    catalog::Catalog,
    config::Ownership,
};
use anyhow::{Context, Result, ensure};
use clap::Args;
use plan::{Change, Plan};
use serde_json::{Value, json};
use std::{fmt, fs, io, path::Path};

#[derive(Args)]
pub struct Mode {
    /// Managed gateway ID, service label/suffix, or unambiguous MCP server name.
    pub name: String,
    /// Show advice and the proposed edits without writing or restarting.
    #[arg(long)]
    pub dry_run: bool,
    /// Save this gateway's ownership without prompting; never restart it.
    #[arg(long)]
    pub yes: bool,
    /// Set from the global --json flag.
    #[arg(skip)]
    pub json: bool,
}

pub fn run(options: Mode, ownership: Ownership) -> Result<()> {
    let command = match ownership {
        Ownership::Shared => "share",
        Ownership::Session => "isolate",
    };
    let root = store::root()?;
    let registry = store::load(&root)?;
    let record = registry.select(&options.name)?;
    let before = fs::read(record.config()).context("Cannot read gateway configuration")?;
    let (source, config) = plan::parse(&before)?;
    let snapshot = read_catalog(&config.catalog_file);
    // Read the snapshot alone. Runtime catalog validation additionally reads
    // backend artifacts and rejects capabilities the advisor needs to report.
    let catalog = snapshot
        .as_deref()
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Catalog>(bytes).ok());
    let advice = advisor::advise(catalog.as_ref(), &config.backend);
    let warning = (ownership == Ownership::Shared && advice.recommendation == Recommendation::Session)
        .then_some("Advisor recommends session ownership. Sharing can expose backend state across clients.");
    let mut report = Report {
        record,
        advice,
        warning,
        changes: Vec::new(),
    };
    if !options.json {
        report.print_advice();
    }
    let result = (|| {
        ensure!(
            !record.removing,
            "Gateway removal is pending; finish remove first"
        );
        config.validate()?;
        if config.ownership == ownership {
            return Ok(Phase::Unchanged(ownership));
        }
        let plan = Plan::new(source, &config, ownership)?;
        report.changes = plan.changes;
        if !options.json {
            report.print_changes();
        }
        plan::parse(plan.after.as_bytes())?.1.validate()?;
        if options.dry_run {
            return Ok(Phase::Preview);
        }
        // Gateway configs deny unknown fields, so an older service binary would
        // refuse to start after reading `ownership_source`.
        ensure!(
            record.binary_hash == crate::catalog::current_exe_digest()?,
            "This gateway runs a different mcp-gate binary that may not accept the new setting. Run mcp-gate setup to update it, then rerun {command}"
        );
        if !approved(&options, command)? {
            return Ok(Phase::Canceled);
        }
        config_edit::apply(
            &root,
            record,
            &before,
            plan.after.as_bytes(),
            command,
            || {
                ensure!(
                    read_catalog(&config.catalog_file) == snapshot,
                    "Saved catalog changed during review; no write performed. Rerun {command}"
                );
                Ok(())
            },
        )?;
        Ok(Phase::Complete)
    })();
    match result {
        Ok(phase) => report.finish(phase, options.json),
        Err(error) if super::canceled(&error) => report.finish(Phase::Canceled, options.json),
        Err(error) if options.json => {
            return Err(ModeFailure {
                error,
                report: report.json("failed"),
            }
            .into());
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn read_catalog(path: &Path) -> Result<Vec<u8>, io::ErrorKind> {
    fs::read(path).map_err(|error| error.kind())
}

fn approved(options: &Mode, command: &str) -> Result<bool> {
    ensure!(
        options.yes || (!options.json && ui::terminal()),
        "No changes applied. Use {command} --dry-run to preview or {command} --yes to save this gateway's ownership"
    );
    super::confirm("Save this gateway's ownership?", options.yes)
}

enum Phase {
    Preview,
    Unchanged(Ownership),
    Canceled,
    Complete,
}

struct Report<'a> {
    record: &'a store::Record,
    advice: Advice,
    warning: Option<&'static str>,
    changes: Vec<Change>,
}

impl Report<'_> {
    fn json(&self, phase: &str) -> Value {
        let mut result = json!({
            "format_version": 1, "phase": phase, "gateway": self.record.id,
            "config": self.record.config(), "advice": self.advice, "changes": self.changes,
        });
        if let Some(warning) = self.warning {
            result["warning"] = json!(warning);
        }
        result
    }

    fn print_advice(&self) {
        println!(
            "Gateway {}. {}",
            ui::safe(&self.record.id),
            self.advice.message
        );
        for signal in &self.advice.signals {
            println!("  {}", ui::safe(&signal.message));
        }
        println!("{}", self.advice.annotation_note);
        if let Some(warning) = self.warning {
            println!("Warning: {warning}");
        }
    }

    fn print_changes(&self) {
        for Change {
            field,
            previous,
            new,
        } in &self.changes
        {
            let display = |value: &Value| {
                if value.is_null() {
                    "(unset)".into()
                } else {
                    value.to_string()
                }
            };
            println!("  {field}: {} -> {}", display(previous), display(new));
        }
    }

    fn finish(&self, phase: Phase, json_output: bool) {
        let (name, message) = match phase {
            Phase::Preview => ("preview", "Dry run. Ownership is unchanged.".into()),
            Phase::Unchanged(ownership) => {
                let mode = match ownership {
                    Ownership::Shared => "shared",
                    Ownership::Session => "session",
                };
                let id = ui::safe(&self.record.id);
                (
                    "unchanged",
                    format!("Nothing to change: {id} is already {mode}."),
                )
            }
            Phase::Canceled => ("canceled", "Canceled. No changes applied.".into()),
            Phase::Complete => (
                "complete",
                "Ownership saved. Restart required to apply it.".into(),
            ),
        };
        let restart = matches!(phase, Phase::Preview | Phase::Complete)
            .then(|| service::restart_command(self.record));
        if json_output {
            let mut value = self.json(name);
            value["message"] = json!(message);
            value["restart_required"] = json!(restart.is_some());
            if let Some(command) = restart {
                value["restart_command"] = json!(command);
            }
            println!("{value}");
        } else {
            println!("{message}");
            if let Some(command) = restart {
                println!("After current actions finish, restart the gateway: {command}");
            }
        }
    }
}

/// Preserve advice in the single JSON document when a reviewed edit fails.
#[derive(Debug)]
pub struct ModeFailure {
    error: anyhow::Error,
    report: Value,
}

impl ModeFailure {
    pub fn json(&self, error: &str) -> Value {
        let mut report = self.report.clone();
        report["error"] = json!(error);
        report
    }
}

impl fmt::Display for ModeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for ModeFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

//! Select and review exact targets before any service, backend or client mutation.
use super::{Selection, inventory, preview, store::Registry, ui};
use crate::clients::Candidate;
use anyhow::{Result, ensure};
use serde_json::{Value, json};

pub struct Setup {
    pub candidates: Vec<Candidate>,
    pub gateways: Vec<String>,
    pub plan: Value,
}

#[derive(Debug)]
struct Back;
impl std::fmt::Display for Back {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Back to scope")
    }
}
impl std::error::Error for Back {}

pub async fn setup(options: &mut Selection, registry: &Registry) -> Result<Option<Setup>> {
    let project_filter = options.project.clone();
    loop {
        match setup_scope(options, registry).await {
            Err(error) if error.is::<Back>() && project_filter.is_none() => {
                options.project = None;
            }
            Err(error) if error.is::<Back>() => {
                ui::canceled(options.json);
                return Ok(None);
            }
            result => return result,
        }
    }
}

async fn setup_scope(options: &mut Selection, registry: &Registry) -> Result<Option<Setup>> {
    let interactive = ui::interactive(options);
    if interactive && !ui::scope(options)? {
        ui::canceled(false);
        return Ok(None);
    }
    let mut inventory = inventory::read(options, registry, false).await?;
    let hash = crate::catalog::digest(&std::env::current_exe()?)?;
    let managed: Vec<_> = registry
        .gateways
        .iter()
        .filter(|r| {
            r.bindings
                .iter()
                .any(|b| super::matches_binding(options, b))
        })
        .collect();
    let count = inspect(&mut inventory, options, managed.len());
    let clarifiable = interactive
        && inventory
            .candidates
            .iter()
            .any(super::context::needs_directory);
    if count == 0 && managed.is_empty() && !clarifiable {
        empty(&inventory, options);
        if interactive {
            while ui::select(
                "No connections can be changed in this scope.",
                vec!["Show connections and reasons".into(), "Finish".into()],
            )? == Some(0)
            {
                inventory::overview(&inventory, true);
            }
        }
        return Ok(None);
    }
    if !interactive && !options.dry_run && !options.yes {
        ensure!(
            false,
            "No changes applied. Run setup in a terminal to choose connections, or use --dry-run. Automation requires explicit --client and --server with --yes."
        );
    }
    let eligible: Vec<_> = inventory
        .candidates
        .into_iter()
        .filter(|c| c.issue.is_none() || (interactive && super::context::needs_directory(c)))
        .collect();
    loop {
        let mut labels: Vec<_> = eligible
            .iter()
            .map(|c| {
                format!(
                    "{} / {} — {} [{}]",
                    c.binding.client,
                    ui::safe(&c.binding.name),
                    if super::context::needs_directory(c) {
                        "choose directory"
                    } else {
                        c.recipe.as_ref().map_or("session", |r| r.mode.as_str())
                    },
                    scope_label(&c.binding)
                )
            })
            .collect();
        labels.extend(managed.iter().map(|r| {
            format!(
                "Manage existing: {}",
                r.bindings
                    .iter()
                    .map(|b| format!("{} / {}", b.client, ui::safe(&b.name)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }));
        let indices = if interactive {
            let Some(indices) = ui::multiple(
                "Choose MCP connections (nothing selected by default):",
                labels,
            )?
            else {
                return Err(Back.into());
            };
            indices
        } else {
            (0..labels.len()).collect()
        };
        if indices.is_empty() {
            ui::canceled(options.json);
            return Ok(None);
        }
        let mut candidates: Vec<_> = eligible
            .iter()
            .enumerate()
            .filter(|(i, _)| indices.contains(i))
            .map(|(_, c)| c.clone())
            .collect();
        if interactive {
            let mut reviewed = Vec::new();
            for candidate in candidates {
                if let Some(c) = super::context::review(candidate)? {
                    reviewed.push(c);
                }
            }
            candidates = reviewed;
        }
        let selected: Vec<_> = managed
            .iter()
            .enumerate()
            .filter(|(i, _)| indices.contains(&(i + eligible.len())))
            .map(|(_, r)| *r)
            .collect();
        if candidates.is_empty() && selected.is_empty() {
            continue;
        }
        let plan = json!({"servers":candidates.iter().map(|c| { let mut v=c.summary(); if options.diff { v["diff"]=preview::diff(c); } v }).collect::<Vec<_>>(),
            "managed":selected.iter().map(|r| preview::managed(r, &hash)).collect::<Vec<_>>(),
            "connections":inventory.connections,"source_issues":inventory.issues,
            "discovery":"Runs the original command; npx, uvx and Docker may download dependencies.",
            "verification":"Gateway readiness does not confirm the application has switched."});
        if options.dry_run {
            if options.json {
                println!(
                    "{}",
                    json!({"format_version":1,"phase":"preview","plan":plan})
                );
            } else {
                preview::display(&plan);
                println!("Dry run. No backends started and no changes applied.");
            }
            return Ok(None);
        }
        if !options.json {
            preview::display(&plan);
        }
        if !interactive || review_setup(&candidates)? {
            return Ok(Some(Setup {
                candidates,
                gateways: selected.iter().map(|r| r.id.clone()).collect(),
                plan,
            }));
        }
    }
}

fn inspect(inventory: &mut inventory::Inventory, options: &Selection, managed: usize) -> usize {
    // Validate launch prerequisites without running commands. Do not claim transitive dependencies are pinned.
    for c in &mut inventory.candidates {
        if c.issue.is_none() {
            c.issue = if !c.cwd.is_dir() {
                Some("Backend working directory is missing; restore it before setup".into())
            } else {
                super::launch::environment(c)
                    .and_then(|env| super::launch::executable(c, &env))
                    .err()
                    .map(|e| e.to_string())
            };
        }
        if let Some(connection) = inventory.connections.iter_mut().find(|connection| {
            connection.client == c.binding.client
                && connection.name == c.binding.name
                && connection.source == c.binding.source
                && connection.project == c.binding.project
        }) && c.issue.is_some()
        {
            connection.reason = c.issue.clone();
            if connection.kind == "direct" && connection.state != "disabled" {
                connection.state = if super::context::needs_directory(c) {
                    "setup needs a directory"
                } else {
                    "setup needs attention"
                };
            }
        }
    }
    let count = inventory
        .candidates
        .iter()
        .filter(|c| c.issue.is_none())
        .count();
    if !options.json {
        println!(
            "Found {} connections: {count} ready to connect, {} managed.",
            inventory.connections.len(),
            managed
        );
        let directories = inventory
            .candidates
            .iter()
            .filter(|c| super::context::needs_directory(c))
            .count();
        println!(
            "{directories} personal connections need an explicit directory. Other connections and limitations: setup --details."
        );
        if options.details {
            inventory::overview(inventory, true);
        }
    }
    count
}

pub fn scope_label(binding: &crate::clients::Binding) -> String {
    binding.project.as_ref().map_or_else(
        || "personal".into(),
        |p| format!("project {}", ui::safe(&p.display().to_string())),
    )
}

fn empty(inventory: &inventory::Inventory, options: &Selection) {
    if options.json {
        println!(
            "{}",
            json!({"format_version":1,"phase":if options.dry_run {"preview"} else {"unchanged"},
            "plan":{"servers":[],"managed":[],"connections":inventory.connections,"source_issues":inventory.issues}})
        );
    } else {
        let external = inventory
            .connections
            .iter()
            .filter(|c| super::inventory::is_external(c.kind))
            .count();
        let http = inventory
            .connections
            .iter()
            .filter(|c| c.kind == "http")
            .count();
        println!(
            "No setup changes available in this scope. {}; {} remain as configured.",
            super::ui::counted(
                external,
                "external gateway connection",
                "external gateway connections"
            ),
            super::ui::counted(http, "other HTTP connection", "other HTTP connections")
        );
        println!(
            "Use setup --details for source files and next steps. For a Claude Code connection tied to a project, use setup --project <directory>."
        );
        if !inventory.issues.is_empty() {
            println!(
                "{} configuration sources need attention; use --details.",
                inventory.issues.len()
            );
        }
    }
}

fn review_setup(candidates: &[Candidate]) -> Result<bool> {
    loop {
        match ui::select(
            "Review setup:",
            vec![
                "Apply setup".into(),
                "Show redacted diff".into(),
                "Back to selection".into(),
                "Cancel".into(),
            ],
        )? {
            Some(0) => return Ok(true),
            Some(1) => {
                for c in candidates {
                    println!(
                        "{} / {}\n{}",
                        c.binding.client,
                        ui::safe(&c.binding.name),
                        serde_json::to_string_pretty(&preview::diff(c))?
                    );
                }
                if candidates.is_empty() {
                    println!(
                        "Existing gateway update preserves client configuration, endpoint and credentials. Changed backend catalogs are reviewed separately during discovery."
                    );
                }
            }
            Some(2) | None => return Ok(false),
            _ => return Err(ui::Canceled.into()),
        }
    }
}

use super::{Selection, inventory, preview, store::Registry, ui, wizard};
use crate::clients::Binding;
use anyhow::{Result, ensure};
use serde_json::json;

pub async fn choose(options: &Selection, registry: &Registry) -> Result<Vec<(String, Binding)>> {
    let available: Vec<_> = registry
        .gateways
        .iter()
        .flat_map(|r| {
            r.bindings
                .iter()
                .filter(|b| super::matches_binding(options, b))
                .map(|b| (r.id.clone(), b.clone()))
        })
        .collect();
    if available.is_empty() {
        let inventory = inventory::read(options, registry, false).await?;
        let legacy = inventory
            .connections
            .iter()
            .filter(|c| c.kind == "legacy")
            .count();
        if options.json {
            println!(
                "{}",
                json!({"format_version":1,"phase":"unchanged","connections":[],
                "discovered_connections":inventory.connections,"source_issues":inventory.issues,"packages_and_data":"retained"})
            );
        } else {
            println!(
                "Nothing to remove: no connections managed by this installation match this scope."
            );
            if legacy > 0 {
                println!(
                    "Found {legacy} legacy gateway connections. Their original settings were not saved by this installer, so they cannot be restored automatically."
                );
            }
            println!(
                "Inspect connections with mcp-gate status --details. Packages and data are unchanged."
            );
            if options.details {
                inventory::overview(&inventory, true);
            }
        }
        return Ok(vec![]);
    }
    let interactive = ui::interactive(options);
    ensure!(
        interactive || options.dry_run || options.yes,
        "No changes applied. Run remove in a terminal to select connections, or use remove --all --yes for all managed connections. Use --dry-run to preview."
    );
    loop {
        let selected = if interactive && !options.all {
            let action = ui::select(
                "Restore direct MCP connections:",
                vec![
                    "Choose connections".into(),
                    "All managed connections in this scope".into(),
                    "Cancel".into(),
                ],
            )?;
            match action {
                Some(0) => {
                    let labels = available
                        .iter()
                        .map(|(_, b)| {
                            format!(
                                "{} / {} [{}]",
                                b.client,
                                ui::safe(&b.name),
                                wizard::scope_label(b)
                            )
                        })
                        .collect();
                    let Some(indices) =
                        ui::multiple("Which connections should return to direct access?", labels)?
                    else {
                        continue;
                    };
                    indices
                        .into_iter()
                        .map(|i| available[i].clone())
                        .collect::<Vec<_>>()
                }
                Some(1) => available.clone(),
                _ => {
                    ui::canceled(false);
                    return Ok(vec![]);
                }
            }
        } else {
            available.clone()
        };
        if selected.is_empty() {
            ui::canceled(options.json);
            return Ok(vec![]);
        }
        let connections: Vec<_> = selected.iter().map(|(_, b)| {
            let mut value = preview::target(b);
            if options.diff { value["diff"] = json!({"before":preview::redact(&b.after),"after":preview::redact(&b.before)}); }
            value
        }).collect();
        if options.dry_run {
            if options.json {
                println!(
                    "{}",
                    json!({"format_version":1,"phase":"preview","connections":connections,"packages_and_data":"retained"})
                );
            } else {
                display(&selected, registry, options.diff)?;
                println!("Dry run. No changes applied.");
            }
            return Ok(vec![]);
        }
        if !options.json {
            display(&selected, registry, options.diff)?;
        }
        if !interactive {
            return Ok(selected);
        }
        loop {
            match ui::select(
                "Review removal:",
                vec![
                    "Restore direct connections".into(),
                    "Show redacted diff".into(),
                    "Back to selection".into(),
                    "Cancel".into(),
                ],
            )? {
                Some(0) => return Ok(selected),
                Some(1) => display(&selected, registry, true)?,
                Some(2) | None => break,
                _ => {
                    ui::canceled(false);
                    return Ok(vec![]);
                }
            }
        }
    }
}

fn display(selected: &[(String, Binding)], registry: &Registry, diff: bool) -> Result<()> {
    println!(
        "\nRemove preview — restore {} direct connections",
        selected.len()
    );
    for (_, b) in selected {
        println!(
            "  {} / {} [{}]",
            b.client,
            ui::safe(&b.name),
            wizard::scope_label(b)
        );
        println!(
            "    Local settings: {}",
            ui::safe(&b.target.display().to_string())
        );
        if diff {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"before":preview::redact(&b.after),"after":preview::redact(&b.before)})
                )?
            );
        }
    }
    let retained = registry
        .gateways
        .iter()
        .filter(|r| {
            let count = selected.iter().filter(|(id, _)| id == &r.id).count();
            count > 0 && count < r.bindings.len()
        })
        .count();
    println!(
        "{retained} gateways stay available to other connections. Busy gateways stay running until clients release them; rerun remove to finish cleanup."
    );
    println!(
        "MCP packages, permissions, user data and artifacts are retained. Restart each affected application once."
    );
    Ok(())
}

//! Turn a reviewed selection into a preview. Dry-run stops before any mutation.
use super::{inventory, preview, ui, wizard};
use crate::clients::{Candidate, SourceIssue};
use anyhow::Result;
use serde_json::{Value, json};

pub(super) struct Choice<'a> {
    pub options: &'a super::Selection,
    pub registry: &'a super::store::Registry,
    pub eligible: &'a [Candidate],
    pub managed: &'a [&'a super::store::Record],
    pub connections: &'a [inventory::Connection],
    pub issues: &'a [SourceIssue],
    pub root: &'a std::path::Path,
    pub hash: &'a str,
    pub interactive: bool,
    pub helpers: &'a [super::helper_migration::Rewrite],
    pub helper_action: bool,
    pub refresh: bool,
}

pub(super) fn select(choice: Choice<'_>) -> Result<Option<wizard::Setup>> {
    loop {
        let Some(indices) = selected_indices(&choice)? else {
            return Ok(None);
        };
        let candidates = picked_candidates(&choice, &indices)?;
        let selected = picked_gateways(&choice, &indices);
        let helper_selected = choice.helper_action
            && indices.contains(&(choice.eligible.len() + choice.managed.len()));
        if candidates.is_empty() && selected.is_empty() && !helper_selected {
            continue;
        }
        let plan = selection_plan(&choice, &candidates, &selected);
        if choice.options.dry_run {
            publish_preview(&plan, choice.options.json);
            return Ok(None);
        }
        if !choice.options.json {
            preview::display(&plan, false);
        }
        if !choice.interactive || wizard::review_setup(&candidates, !choice.helpers.is_empty())? {
            return Ok(Some(wizard::Setup {
                candidates,
                gateways: selected.iter().map(|record| record.id.clone()).collect(),
                helpers: choice.helpers.to_vec(),
                hash: choice.hash.into(),
                plan,
            }));
        }
    }
}

fn selected_indices(choice: &Choice<'_>) -> Result<Option<Vec<usize>>> {
    let mut labels: Vec<_> = choice.eligible.iter().map(choice_label).collect();
    labels.extend(choice.managed.iter().map(|record| {
        if record.bindings.is_empty() {
            return format!(
                "Manage existing gateway: {} (no saved client bindings)",
                ui::safe(&record.label)
            );
        }
        format!(
            "Manage existing: {}",
            record
                .bindings
                .iter()
                .map(|binding| format!("{} / {}", binding.client, ui::safe(&binding.name)))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }));
    if choice.helper_action {
        labels.push("Update client gateway executables".into());
    }
    if !choice.interactive {
        return Ok(Some((0..labels.len()).collect()));
    }
    super::trace::log("choose: ui::multiple start");
    match ui::multiple(
        "Choose MCP connections (nothing selected by default):",
        labels,
    )? {
        None => Err(wizard::Back.into()),
        Some(indices) if indices.is_empty() => Ok(None),
        Some(indices) => Ok(Some(indices)),
    }
}

fn choice_label(candidate: &Candidate) -> String {
    format!(
        "{} / {} — {} [{}]",
        candidate.binding.client,
        ui::safe(&candidate.binding.name),
        if super::context::needs_directory(candidate) {
            "choose directory"
        } else {
            candidate
                .recipe
                .as_ref()
                .map_or("session", |recipe| recipe.mode.as_str())
        },
        wizard::scope_label(&candidate.binding)
    )
}

fn picked_candidates(choice: &Choice<'_>, indices: &[usize]) -> Result<Vec<Candidate>> {
    let mut candidates: Vec<_> = choice
        .eligible
        .iter()
        .enumerate()
        .filter(|(index, _)| indices.contains(index))
        .map(|(_, candidate)| candidate.clone())
        .collect();
    if !choice.interactive {
        return Ok(candidates);
    }
    let mut reviewed = Vec::new();
    for candidate in candidates.drain(..) {
        if let Some(candidate) = super::context::review(candidate)? {
            reviewed.push(candidate);
        }
    }
    Ok(reviewed)
}

fn picked_gateways<'a>(choice: &Choice<'a>, indices: &[usize]) -> Vec<&'a super::store::Record> {
    choice
        .managed
        .iter()
        .enumerate()
        .filter(|(index, _)| indices.contains(&(index + choice.eligible.len())))
        .map(|(_, record)| *record)
        .collect()
}

fn selection_plan(
    choice: &Choice<'_>,
    candidates: &[Candidate],
    selected: &[&super::store::Record],
) -> Value {
    let mut servers: Vec<_> = candidates
        .iter()
        .map(|candidate| wizard::server_plan(candidate, choice.options.diff))
        .collect();
    super::labels::note_shared_gateways(&mut servers, choice.registry);
    json!({"servers":servers,
        "helper_migrations":choice.helpers.iter().map(super::helper_migration::Rewrite::summary).collect::<Vec<_>>(),
        "refresh_helper_binary":choice.refresh,
        "managed":selected.iter().map(|record| preview::managed(record, choice.root, choice.hash)).collect::<Vec<_>>(),
        "connections":choice.connections,"source_issues":choice.issues,
        "discovery":"Runs the original command; npx, uvx and Docker may download dependencies.",
        "verification":"Gateway readiness does not confirm the application has switched."})
}

fn publish_preview(plan: &Value, json: bool) {
    if json {
        println!(
            "{}",
            json!({"format_version":1,"phase":"preview","plan":plan})
        );
    } else {
        preview::display(plan, true);
        println!("Dry run. No backends started and no changes applied.");
    }
}

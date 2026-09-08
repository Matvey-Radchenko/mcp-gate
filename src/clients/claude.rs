use super::{Candidate, Client, document, scan::candidate};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

pub(super) fn local(target: &Path, project: &Path, out: &mut Vec<Candidate>) -> Result<()> {
    if !target.is_file() {
        return Ok(());
    }
    let source = std::fs::read_to_string(target)?;
    let project_key = project
        .to_str()
        .context("Project path must be Unicode")?
        .to_owned();
    let base = vec!["projects".to_owned(), project_key, "mcpServers".to_owned()];
    let local = document::entry(&source, false, &base)?;
    // Local scope wins over the shared project file and user scope for this name.
    let names: Vec<_> = out
        .iter()
        .filter(|c| c.binding.client == Client::ClaudeCode)
        .map(|c| c.binding.name.clone())
        .collect();
    for name in names {
        let indices: Vec<_> = out
            .iter()
            .enumerate()
            .filter(|(_, c)| c.binding.client == Client::ClaudeCode && c.binding.name == name)
            .map(|(i, _)| i)
            .collect();
        let index = *indices.last().context("Missing Claude entry")?;
        for i in indices.into_iter().filter(|i| *i != index) {
            out[i].issue = Some("A higher-priority project definition is selected".into());
        }
        let c = &mut out[index];
        let mut keys = base.clone();
        keys.push(name.clone());
        if let Some(value) = local.get(&name) {
            *c = candidate(
                Client::ClaudeCode,
                &name,
                target,
                keys.clone(),
                value,
                project,
                false,
            )?;
        }
        if c.command.is_empty() {
            continue;
        }
        c.binding.target = target.to_path_buf();
        c.binding.path = keys;
        c.binding.before = local.get(&name).cloned().unwrap_or(Value::Null);
        c.binding.project = Some(project.into());
        if c.issue.as_deref().is_some_and(|issue| {
            issue.starts_with("Project file") || issue.starts_with("Global command")
        }) {
            c.issue = None;
        }
    }
    if let Some(local) = local.as_object() {
        for (name, value) in local {
            if out
                .iter()
                .any(|c| c.binding.client == Client::ClaudeCode && c.binding.name == *name)
            {
                continue;
            }
            let mut keys = base.clone();
            keys.push(name.clone());
            let mut c = candidate(
                Client::ClaudeCode,
                name,
                target,
                keys,
                value,
                project,
                false,
            )?;
            c.binding.project = Some(project.into());
            if c.issue
                .as_deref()
                .is_some_and(|i| i.starts_with("Global command"))
            {
                c.issue = None;
            }
            out.push(c);
        }
    }
    Ok(())
}

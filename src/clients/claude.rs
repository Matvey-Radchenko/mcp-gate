use super::{Candidate, Client, document, scan::candidate};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Claude stores physical project keys with forward slashes on Windows too.
/// Keep this client-specific spelling separate from backend filesystem paths.
pub fn project_key(project: &Path) -> Result<String> {
    let key = project.to_str().context("Project path must be Unicode")?;
    #[cfg(windows)]
    return Ok(key.replace('\\', "/"));
    #[cfg(not(windows))]
    Ok(key.to_owned())
}

pub(super) fn local(target: &Path, project: &Path, out: &mut Vec<Candidate>) -> Result<()> {
    if !target.is_file() {
        return Ok(());
    }
    let source = std::fs::read_to_string(target)?;
    let base = vec![
        "projects".to_owned(),
        project_key(project)?,
        "mcpServers".to_owned(),
    ];
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
        if local.get(&name).is_none()
            && c.binding
                .source
                .file_name()
                .is_some_and(|n| n == ".mcp.json")
            && !approved(target, project, &name)?
        {
            c.issue=Some("Project MCP is not explicitly approved in personal settings; migrating it would bypass the client's project approval. Approve it in Claude first.".into());
            continue;
        }
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

fn approved(personal: &Path, project: &Path, name: &str) -> Result<bool> {
    let document = super::document::parse(&std::fs::read_to_string(personal)?, false)?;
    let scope = &document["projects"][project_key(project)?];
    let contains = |value: &Value| {
        value
            .as_array()
            .is_some_and(|a| a.iter().any(|n| n.as_str() == Some(name)))
    };
    let mut allowed = contains(&scope["enabledMcpjsonServers"]);
    let mut disabled = contains(&scope["disabledMcpjsonServers"]);
    let parent = personal
        .parent()
        .context("Personal configuration has no parent")?;
    let user_settings = if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        parent.join("settings.json")
    } else {
        parent.join(".claude/settings.json")
    };
    for path in [
        user_settings,
        project.join(".claude/settings.json"),
        project.join(".claude/settings.local.json"),
    ] {
        if !path.is_file() {
            continue;
        }
        let settings = super::document::parse(&std::fs::read_to_string(path)?, false)?;
        allowed |= settings["enableAllProjectMcpServers"] == true
            || contains(&settings["enabledMcpjsonServers"]);
        disabled |= contains(&settings["disabledMcpjsonServers"]);
    }
    Ok(allowed && !disabled)
}

//! Discover entries in descriptor order and resolve project precedence before expansion.
use super::{Binding, Candidate, Client, document, sources};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Scan {
    pub candidates: Vec<Candidate>,
    pub issues: Vec<SourceIssue>,
}
#[derive(Serialize)]
pub struct SourceIssue {
    pub client: Client,
    pub source: PathBuf,
    pub reason: String,
}

pub fn discover(
    home: &Path,
    project: &Path,
    clients: &[Client],
    project_selected: bool,
) -> Result<Vec<Candidate>> {
    Ok(collect(
        home,
        project,
        clients,
        project_selected,
        |_| None,
        std::env::consts::OS,
    )
    .candidates)
}
pub fn discover_current(
    home: &Path,
    project: &Path,
    clients: &[Client],
    project_selected: bool,
) -> Result<Vec<Candidate>> {
    Ok(inventory_current(home, project, clients, project_selected)?.candidates)
}
pub fn inventory_current(
    home: &Path,
    project: &Path,
    clients: &[Client],
    project_selected: bool,
) -> Result<Scan> {
    Ok(collect(
        home,
        project,
        clients,
        project_selected,
        |name| std::env::var_os(name),
        std::env::consts::OS,
    ))
}

fn collect(
    home: &Path,
    project: &Path,
    clients: &[Client],
    project_selected: bool,
    env: impl Fn(&str) -> Option<OsString>,
    os: &str,
) -> Scan {
    let mut scan = Scan::default();
    let project_scope = sources::selected_project(home, project, project_selected);
    let mut local_scopes = Vec::new();
    for &client in Client::all() {
        if !clients.is_empty() && !clients.contains(&client) {
            continue;
        }
        let descriptor = client.descriptor();
        let personal = sources::personal_paths_for_os(Some(home), descriptor, &env, os);
        if descriptor.claude_local_projects
            && let (Some(project), Some(target)) = (project_scope, personal.first())
        {
            local_scopes.push((client, project, target.clone()));
        }
        scan_sources(
            client,
            sources::paths(descriptor, personal, project_scope),
            project,
            &mut scan,
        );
    }
    // Local overrides are resolved after every file is read, preserving source issue order.
    for (client, project, target) in local_scopes {
        super::claude::apply_local(client, &target, project, &mut scan);
    }
    finish(&mut scan.candidates, home);
    scan
}

fn scan_sources(client: Client, sources: Vec<sources::Source>, project: &Path, scan: &mut Scan) {
    for source in sources {
        if source.path.is_file()
            && read_source(client, &source, project, &mut scan.candidates).is_err()
        {
            scan.issues.push(SourceIssue { client, source: source.path,
                reason: "Cannot read this configuration or its MCP schema. Fix its syntax and string-valued command/environment fields, then retry; values hidden.".into() });
        }
    }
}

fn finish(out: &mut [Candidate], home: &Path) {
    // Never edit a lower-precedence global definition shadowed by a project entry.
    let shadowed: Vec<_> = out
        .iter()
        .filter(|c| c.binding.project.is_some())
        .map(|c| (c.binding.client, c.binding.name.clone()))
        .collect();
    for c in out {
        // A bridge owns its connection settings. It is never a direct
        // executable that setup should expand, match against recipes, or probe.
        if bridge_command(&c.binding.direct).is_some() {
            continue;
        }
        if c.binding.project.is_none()
            && shadowed.contains(&(c.binding.client, c.binding.name.clone()))
        {
            c.issue = Some(
                "A project definition takes precedence; global settings left unchanged".into(),
            );
        }
        if let Err(error) = super::expand::apply(c, home) {
            c.issue = Some(error.to_string());
        }
        c.recipe = super::recipes::matching(c);
    }
}

fn read_source(
    client: Client,
    source: &sources::Source,
    project: &Path,
    out: &mut Vec<Candidate>,
) -> Result<()> {
    let descriptor = client.descriptor();
    let value = document::parse(
        &std::fs::read_to_string(&source.path)?,
        descriptor.format.is_toml(),
    )?;
    let Some(mut servers) = value.get(&descriptor.section) else {
        return Ok(());
    };
    let mut base = vec![descriptor.section.clone()];
    if let Some(key) = &descriptor.nested_section
        && let Some(nested) = servers.get(key)
    {
        servers = nested;
        base.push(key.clone());
    }
    let servers = servers
        .as_object()
        .context("MCP entries must be an object")?;
    let mut parsed = Vec::new();
    for (name, config) in servers {
        let mut keys = base.clone();
        keys.push(name.clone());
        parsed.push(candidate(
            client,
            name,
            &source.path,
            keys,
            config,
            project,
            source.project,
        )?);
    }
    out.extend(parsed);
    Ok(())
}

pub(super) fn candidate(
    client: Client,
    name: &str,
    source: &Path,
    path: Vec<String>,
    config: &Value,
    project: &Path,
    project_scoped: bool,
) -> Result<Candidate> {
    let command = command(config)?;
    let env: BTreeMap<String, String> = serde_json::from_value(
        config
            .get("env")
            .or_else(|| config.get("environment"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(|_| anyhow::anyhow!("MCP environment must contain strings; values hidden"))?;
    let cwd = config
        .get("cwd")
        .and_then(Value::as_str)
        .map(|p| project.join(p))
        .unwrap_or_else(|| project.to_path_buf());
    let issue = command_issue(config, &command, project_scoped);
    Ok(Candidate {
        recipe: None,
        env_files: BTreeMap::new(),
        binding: Binding {
            client,
            name: name.into(),
            source: source.into(),
            source_path: path.clone(),
            target: source.into(),
            path,
            toml: client.descriptor().format.is_toml(),
            before: config.clone(),
            direct: config.clone(),
            project: project_scoped.then(|| project.to_path_buf()),
            after: Value::Null,
        },
        command,
        cwd,
        env,
        service_label: None,
        inherit_env: serde_json::from_value(
            config
                .get("env_vars")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|_| {
            anyhow::anyhow!("Inherited environment names must be strings; values hidden")
        })?,
        issue,
    })
}

fn command(config: &Value) -> Result<Vec<String>> {
    Ok(
        if let Some(command) = config.get("command").and_then(Value::as_array) {
            command
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .context("Command arguments must be strings")
                })
                .collect::<Result<Vec<_>>>()?
        } else if let Some(command) = config.get("command").and_then(Value::as_str) {
            let mut args = vec![command.to_string()];
            if let Some(values) = config.get("args").and_then(Value::as_array) {
                for value in values {
                    args.push(value.as_str().context("Arguments must be strings")?.into());
                }
            }
            args
        } else {
            Vec::new()
        },
    )
}

fn command_issue(config: &Value, command: &[String], project_scoped: bool) -> Option<String> {
    let mut issue = None;
    if command.is_empty() {
        issue = Some("Existing HTTP, managed or non-stdio entry: left unchanged".into());
    }
    if !command.is_empty() && project_scoped {
        issue=Some("Project file is read-only to setup. A verified machine-local override is required; use a client local-scope definition, then rerun setup".into());
    } else if !command.is_empty() && config.get("cwd").is_none() {
        issue=Some("Global command has no fixed working directory. Setup can ask you to choose one explicitly for use across every project. If this MCP needs a different directory per project, keep it direct or use a verified project-local scope.".into());
    }
    if let Some(executable) = command
        .first()
        .and_then(|s| Path::new(s).file_stem())
        .and_then(|s| s.to_str())
    {
        if executable == "docker" {
            if let Err(error) = crate::platform::docker::validate(&command[1..]) {
                issue = Some(error.to_string());
            }
        } else if executable == "mcp-session-gateway" {
            issue =
                Some("External gateway is recognized but is not owned by this installation".into());
        }
    }
    if bridge_command(config).is_some() {
        issue = Some("External gateway stdio bridge is already configured; left unchanged".into());
    }
    if config.get("enabled") == Some(&Value::Bool(false))
        || config.get("disabled") == Some(&Value::Bool(true))
    {
        issue = Some("Disabled MCP: left unchanged".into());
    }
    issue
}

pub(crate) fn bridge_command(entry: &Value) -> Option<&str> {
    let command = entry.get("command")?.as_str()?;
    let name = Path::new(command).file_name()?.to_str()?;
    let subcommand = entry.get("args")?.as_array()?.first()?.as_str()?;
    (matches!(name, "mcp-gate" | "mcp-gate.exe") && subcommand == "connect").then_some(command)
}

#[cfg(test)]
mod tests;

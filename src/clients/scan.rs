use super::{Binding, Candidate, Client, document};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
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
    Ok(collect(home, project, clients, project_selected, &BTreeMap::new())?.candidates)
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
    let mut overrides = BTreeMap::new();
    if let Some(path) = std::env::var_os("CODEX_HOME") {
        overrides.insert(Client::Codex, vec![PathBuf::from(path).join("config.toml")]);
    }
    if let Some(path) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        overrides.insert(
            Client::ClaudeCode,
            vec![PathBuf::from(path).join(".claude.json")],
        );
    }
    let mut opencode = Vec::new();
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        for name in ["opencode.json", "opencode.jsonc"] {
            opencode.push(PathBuf::from(&path).join("opencode").join(name));
        }
    }
    if let Some(path) = std::env::var_os("OPENCODE_CONFIG") {
        opencode.push(PathBuf::from(path));
    }
    if !opencode.is_empty() {
        overrides.insert(Client::Opencode, opencode);
    }
    collect(home, project, clients, project_selected, &overrides)
}
fn collect(
    home: &Path,
    project: &Path,
    clients: &[Client],
    project_selected: bool,
    overrides: &BTreeMap<Client, Vec<PathBuf>>,
) -> Result<Scan> {
    let mut out = Vec::new();
    let mut issues = Vec::new();
    for client in [Client::Codex, Client::Opencode, Client::ClaudeCode] {
        if !clients.is_empty() && !clients.contains(&client) {
            continue;
        }
        for source in super::sources::paths(
            home,
            project,
            client,
            project_selected,
            overrides.get(&client),
        ) {
            if !source.path.is_file() {
                continue;
            }
            if read_source(client, &source, project, &mut out).is_err() {
                issues.push(SourceIssue { client, source: source.path,
                    reason: "Cannot read this configuration or its MCP schema. Fix its syntax and string-valued command/environment fields, then retry; values hidden.".into() });
            }
        }
    }
    if project_selected
        && super::sources::identity(home) != super::sources::identity(project)
        && (clients.is_empty() || clients.contains(&Client::ClaudeCode))
    {
        let target = overrides
            .get(&Client::ClaudeCode)
            .and_then(|p| p.first())
            .cloned()
            .unwrap_or_else(|| home.join(".claude.json"));
        // A failed local-scope read must not leave partially prepared overrides eligible.
        let mut local = out.clone();
        if super::claude::local(&target, project, &mut local).is_ok() {
            out = local;
        } else {
            for c in out
                .iter_mut()
                .filter(|c| c.binding.client == Client::ClaudeCode)
            {
                c.issue = Some("Cannot verify Claude local scope and approvals; fix its personal/project configuration first".into());
            }
            issues.push(SourceIssue {
                client: Client::ClaudeCode,
                source: target,
                reason: "Cannot verify local project settings or approvals; values hidden".into(),
            });
        }
    }
    // Never edit a lower-precedence global definition shadowed by a project entry.
    let shadowed: Vec<_> = out
        .iter()
        .filter(|c| c.binding.project.is_some())
        .map(|c| (c.binding.client, c.binding.name.clone()))
        .collect();
    for c in &mut out {
        if c.binding.project.is_none()
            && shadowed.contains(&(c.binding.client, c.binding.name.clone()))
        {
            c.issue = Some(
                "A project definition takes precedence; global settings left unchanged".into(),
            );
        }
    }
    for c in &mut out {
        if let Err(error) = super::expand::apply(c, home) {
            c.issue = Some(error.to_string());
        }
        c.recipe = super::recipes::matching(c);
    }
    Ok(Scan {
        candidates: out,
        issues,
    })
}

fn read_source(
    client: Client,
    source: &super::sources::Source,
    project: &Path,
    out: &mut Vec<Candidate>,
) -> Result<()> {
    let value = document::parse(
        &std::fs::read_to_string(&source.path)?,
        client == Client::Codex,
    )?;
    let Some(mut servers) = value.get(source.section) else {
        return Ok(());
    };
    let mut base = vec![source.section.to_string()];
    if client == Client::Opencode && servers.get("servers").is_some() {
        servers = &servers["servers"];
        base.push("servers".into());
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
    let command = if let Some(command) = config.get("command").and_then(Value::as_array) {
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
    };
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
            issue = Some("Legacy gateway is recognized but is not owned by this installer".into());
        }
    }
    if config.get("enabled") == Some(&Value::Bool(false))
        || config.get("disabled") == Some(&Value::Bool(true))
    {
        issue = Some("Disabled MCP: left unchanged".into());
    }
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
            toml: client == Client::Codex,
            before: config.clone(),
            direct: config.clone(),
            project: project_scoped.then(|| project.to_path_buf()),
            after: Value::Null,
        },
        command,
        cwd,
        env,
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

use super::{Binding, Candidate, Client, document};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub fn discover(
    home: &Path,
    project: &Path,
    clients: &[Client],
    project_selected: bool,
) -> Result<Vec<Candidate>> {
    collect(home, project, clients, project_selected, &BTreeMap::new())
}
pub fn discover_current(
    home: &Path,
    project: &Path,
    clients: &[Client],
    project_selected: bool,
) -> Result<Vec<Candidate>> {
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
) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for client in [Client::Codex, Client::Opencode, Client::ClaudeCode] {
        if !clients.is_empty() && !clients.contains(&client) {
            continue;
        }
        let mut paths: Vec<(PathBuf, bool, &str)> = match client {
            Client::Codex => vec![
                (home.join(".codex/config.toml"), false, "mcp_servers"),
                (project.join(".codex/config.toml"), true, "mcp_servers"),
            ],
            Client::Opencode => vec![
                (home.join(".config/opencode/opencode.json"), false, "mcp"),
                (home.join(".config/opencode/opencode.jsonc"), false, "mcp"),
                (project.join("opencode.json"), true, "mcp"),
                (project.join("opencode.jsonc"), true, "mcp"),
            ],
            Client::ClaudeCode => vec![
                (home.join(".claude.json"), false, "mcpServers"),
                (project.join(".mcp.json"), true, "mcpServers"),
            ],
        };
        if let Some(personal) = overrides.get(&client) {
            let section = paths[0].2;
            paths.retain(|(_, project, _)| *project);
            for path in personal {
                paths.insert(0, (path.clone(), false, section));
            }
        }
        for (path, project_scoped, section) in paths {
            if !path.is_file() {
                continue;
            }
            let toml = client == Client::Codex;
            let text = std::fs::read_to_string(&path)?;
            let value = document::parse(&text, toml)
                .with_context(|| format!("Cannot parse {}", path.display()))?;
            let Some(mut servers) = value.get(section) else {
                continue;
            };
            let mut base = vec![section.to_string()];
            if client == Client::Opencode && servers.get("servers").is_some() {
                servers = &servers["servers"];
                base.push("servers".into());
            }
            let Some(servers) = servers.as_object() else {
                continue;
            };
            for (name, config) in servers {
                let mut keys = base.clone();
                keys.push(name.clone());
                out.push(candidate(
                    client,
                    name,
                    &path,
                    keys,
                    config,
                    project,
                    project_scoped,
                )?);
            }
        }
    }
    if project_selected && (clients.is_empty() || clients.contains(&Client::ClaudeCode)) {
        let target = overrides
            .get(&Client::ClaudeCode)
            .and_then(|p| p.first())
            .cloned()
            .unwrap_or_else(|| home.join(".claude.json"));
        super::claude::local(&target, project, &mut out)?;
    }
    // Never edit a lower-precedence global definition shadowed by a project entry.
    let shadowed: Vec<_> = out
        .iter()
        .filter(|c| c.binding.source.starts_with(project))
        .map(|c| (c.binding.client, c.binding.name.clone()))
        .collect();
    for c in &mut out {
        if !c.binding.source.starts_with(project)
            && shadowed.contains(&(c.binding.client, c.binding.name.clone()))
        {
            c.issue = Some(
                "A project definition takes precedence; global settings left unchanged".into(),
            );
        }
    }
    for c in &mut out {
        c.recipe = super::recipes::matching(c);
    }
    Ok(out)
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
    .context("MCP environment must contain strings")?;
    let cwd = config
        .get("cwd")
        .and_then(Value::as_str)
        .map(|p| project.join(p))
        .unwrap_or_else(|| project.to_path_buf());
    let mut issue = None;
    if command.is_empty() {
        issue = Some("Existing HTTP, managed or non-stdio entry: left unchanged".into());
    }
    if config.get("enabled") == Some(&Value::Bool(false))
        || config.get("disabled") == Some(&Value::Bool(true))
    {
        issue = Some("Disabled MCP: left unchanged".into());
    }
    if !command.is_empty() && project_scoped {
        issue=Some("Project file is read-only to setup. A verified machine-local override is required; use a client local-scope definition, then rerun setup".into());
    } else if !command.is_empty() && config.get("cwd").is_none() {
        issue=Some("Global command has a project-dependent working directory. Set an explicit cwd in your personal client configuration before migration".into());
    }
    if env
        .values()
        .any(|v| v.contains("{env:") || v.contains("{file:") || v.contains("${"))
    {
        issue = Some(
            "Dynamic environment reference requires a client-specific resolver; left unchanged"
                .into(),
        );
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
    Ok(Candidate {
        recipe: None,
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
        )?,
        issue,
    })
}

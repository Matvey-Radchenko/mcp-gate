//! Resolve executables from the backend's retained environment, not setup's PATH.
use crate::clients::Candidate;
use anyhow::{Context, Result};
use std::{collections::BTreeMap, path::PathBuf};

fn key(name: &str) -> String {
    if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.into()
    }
}

pub fn environment(candidate: &Candidate) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for &name in crate::platform::BASE_ENVIRONMENT {
        if let Ok(value) = std::env::var(name) {
            values.insert(key(name), value);
        }
    }
    for name in &candidate.inherit_env {
        let value = std::env::var(name).with_context(|| {
            format!(
                "Declared inherited environment variable {name} is unavailable in the setup process"
            )
        })?;
        values.insert(key(name), value);
    }
    for (name, value) in &candidate.env {
        values.insert(key(name), value.clone());
    }
    // File values remain live references and must not be copied into the config.
    for name in candidate.env_files.keys() {
        values.remove(&key(name));
    }
    Ok(values)
}

fn value(
    candidate: &Candidate,
    env: &BTreeMap<String, String>,
    name: &str,
) -> Result<Option<String>> {
    if let Some((_, path)) = candidate.env_files.iter().find(|(k, _)| key(k) == name) {
        crate::platform::validate_private(path)?;
        return Ok(Some(
            std::fs::read_to_string(path)?
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        ));
    }
    Ok(env.get(name).cloned())
}

pub fn executable(candidate: &Candidate, env: &BTreeMap<String, String>) -> Result<PathBuf> {
    let command = candidate
        .command
        .first()
        .context("Missing backend command")?;
    let path = std::path::Path::new(command);
    let suffixes = if cfg!(windows) && path.extension().is_none() {
        let extensions =
            value(candidate, env, "PATHEXT")?.unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        extensions
            .split(';')
            .map(str::to_ascii_lowercase)
            .filter(|s| matches!(s.as_str(), ".com" | ".exe" | ".bat" | ".cmd"))
            .chain(std::iter::once(String::new()))
            .collect::<Vec<_>>()
    } else {
        vec![String::new()]
    };
    let directories = if path.is_absolute() || path.components().count() > 1 {
        vec![candidate.cwd.clone()]
    } else {
        let paths = value(candidate, env, "PATH")?.context("Backend PATH is not set")?;
        std::env::split_paths(&paths)
            .map(|p| candidate.cwd.join(p))
            .collect()
    };
    for directory in directories {
        for suffix in &suffixes {
            let found = directory.join(format!("{command}{suffix}"));
            if is_executable(&found) {
                return Ok(found);
            }
        }
    }
    anyhow::bail!("MCP executable is unavailable in its configured PATH/context; install it first")
}

/// Two clients can share a gateway when the backend definition is the same.
/// Session mode still starts one backend per HTTP session; shared mode still
/// keeps a single backend. Different ownership modes are never combined.
/// New clients use the candidate's own mode even for operator-configured gateways.
pub fn same_backend(config: &crate::config::Config, candidate: &Candidate) -> Result<bool> {
    if ownership(config) != mode(candidate) || !context_matches(config, candidate)? {
        return Ok(false);
    }
    let env = environment(candidate)?;
    if config.backend.command != executable(candidate, &env)? {
        return Ok(false);
    }
    Ok(arguments_match(config, candidate))
}

fn ownership(config: &crate::config::Config) -> &'static str {
    match config.ownership {
        crate::config::Ownership::Shared => "shared",
        crate::config::Ownership::Session => "session",
    }
}

fn mode(candidate: &Candidate) -> &str {
    candidate
        .recipe
        .as_ref()
        .map_or("session", |recipe| recipe.mode.as_str())
}

fn context_matches(config: &crate::config::Config, candidate: &Candidate) -> Result<bool> {
    let disabled = candidate
        .recipe
        .as_ref()
        .map(|recipe| recipe.disabled_tools.clone())
        .unwrap_or_default();
    let (directory_env, working_directory_env) = candidate.recipe.as_ref().map_or_else(
        || (Vec::new(), None),
        |recipe| {
            (
                recipe.directory_env.clone(),
                recipe.working_directory_env.clone(),
            )
        },
    );
    let docker = candidate
        .command
        .first()
        .and_then(|command| std::path::Path::new(command).file_stem())
        .and_then(|stem| stem.to_str())
        == Some("docker");
    Ok(
        config.backend.working_directory.as_ref() == Some(&candidate.cwd)
            && config.backend.env == environment(candidate)?
            && config.backend.env_files == candidate.env_files
            && config.backend.inherit_env == candidate.inherit_env
            && config.backend.docker == docker
            && config.tool_policy.disabled == disabled
            && config.backend.directory_env == directory_env
            && config.backend.working_directory_env == working_directory_env,
    )
}

fn arguments_match(config: &crate::config::Config, candidate: &Candidate) -> bool {
    if config.backend.entrypoint.is_none() && config.backend.command_args.is_empty() {
        return config.backend.args == candidate.command[1..];
    }
    let Some(entrypoint) = config.backend.entrypoint.as_deref() else {
        return false;
    };
    let Some(index) = candidate
        .command
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, arg)| candidate.cwd.join(arg) == entrypoint)
        .map(|(index, _)| index)
    else {
        return false;
    };
    config.backend.command_args == candidate.command[1..index]
        && config.backend.args == candidate.command[index + 1..]
}

fn is_executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    metadata.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::{Binding, Client};
    use serde_json::json;

    fn sample(cwd: &std::path::Path, arg: &str) -> (crate::config::Config, Candidate) {
        let command = std::env::current_exe().unwrap();
        let candidate = Candidate {
            binding: Binding {
                client: Client::CODEX,
                name: "fixture".into(),
                source: cwd.join("source"),
                source_path: vec!["mcp_servers".into(), "fixture".into()],
                target: cwd.join("source"),
                path: vec!["mcp_servers".into(), "fixture".into()],
                toml: true,
                before: json!({}),
                direct: json!({}),
                project: None,
                after: json!(null),
            },
            command: vec![command.to_string_lossy().into_owned(), arg.into()],
            cwd: cwd.to_path_buf(),
            env: Default::default(),
            env_files: Default::default(),
            inherit_env: Vec::new(),
            issue: None,
            recipe: None,
            service_label: None,
        };
        let config = serde_json::from_value(json!({
            "format_version": 2,
            "ownership": "session",
            "listen": "127.0.0.1:1",
            "token_file": cwd.join("token"),
            "catalog_file": cwd.join("catalog.json"),
            "state_dir": cwd,
            "backend": {
                "profile": "stdio",
                "command": command,
                "version": "fixture",
                "args": [arg],
                "working_directory": cwd,
                "env": environment(&candidate).unwrap()
            }
        }))
        .unwrap();
        (config, candidate)
    }

    #[test]
    fn identical_session_backends_match_and_a_different_argument_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = crate::platform::project_path(dir.path()).unwrap();
        let (config, candidate) = sample(&cwd, "--flag");
        assert!(same_backend(&config, &candidate).unwrap());
        let (_, other) = sample(&cwd, "--other");
        assert!(!same_backend(&config, &other).unwrap());
    }

    #[test]
    fn setup_keeps_recipe_less_candidates_out_of_operator_shared_gateways() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = crate::platform::project_path(dir.path()).unwrap();
        let (mut config, candidate) = sample(&cwd, "--flag");
        config.ownership = crate::config::Ownership::Shared;
        config.ownership_source = Some(crate::config::OwnershipSource::Operator);
        config.max_workers = 1;
        assert!(!same_backend(&config, &candidate).unwrap());
    }
}

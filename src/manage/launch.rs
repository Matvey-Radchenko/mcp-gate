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

fn is_executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    true
}

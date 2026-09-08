//! Resolve client substitutions as data, retaining argv boundaries and source-relative files.
use super::{Candidate, Client};
use anyhow::{Context, Result, ensure};
use std::path::Path;
fn substitute(
    mut text: String,
    open: &str,
    mut resolve: impl FnMut(&str) -> Result<String>,
) -> Result<String> {
    let mut from = 0;
    while let Some(offset) = text[from..].find(open) {
        let start = from + offset;
        let end = start
            + open.len()
            + text[start + open.len()..]
                .find('}')
                .context("Unclosed configuration substitution")?;
        let value = resolve(&text[start + open.len()..end])?;
        text.replace_range(start..=end, &value);
        from = start + value.len();
    }
    Ok(text)
}
pub fn value(
    text: &str,
    client: Client,
    source: &Path,
    home: &Path,
    env: impl Fn(&str) -> Option<String>,
) -> Result<String> {
    if client == Client::ClaudeCode {
        substitute(text.into(), "${", |name| {
            let (key, default) = name
                .split_once(":-")
                .map_or((name, None), |(k, v)| (k, Some(v)));
            env(key)
                .or_else(|| default.map(str::to_owned))
                .context("Required Claude environment reference is unavailable; value hidden")
        })
    } else if client == Client::Opencode {
        let text = substitute(text.into(), "{env:", |name| {
            env(name)
                .context("Required OpenCode environment reference is unavailable; value hidden")
        })?;
        substitute(text, "{file:", |name| {
            let path = if let Some(tail) = name.strip_prefix("~/") {
                home.join(tail)
            } else {
                source
                    .parent()
                    .context("Configuration parent missing")?
                    .join(name)
            };
            let bytes = std::fs::read_to_string(path)
                .context("Referenced configuration file is unavailable")?;
            Ok(bytes.trim().into())
        })
    } else {
        Ok(text.into())
    }
}
pub fn apply(candidate: &mut Candidate, home: &Path) -> Result<()> {
    for arg in &mut candidate.command {
        ensure!(
            !arg.contains("{file:"),
            "File-backed command/argument expansion requires an explicit invocation; left unchanged"
        );
        *arg = value(
            arg,
            candidate.binding.client,
            &candidate.binding.source,
            home,
            |k| std::env::var(k).ok(),
        )?;
    }
    for (key, item) in &mut candidate.env {
        if candidate.binding.client == Client::Opencode && item.contains("{file:") {
            let path = item
                .strip_prefix("{file:")
                .and_then(|s| s.strip_suffix('}'))
                .context(
                    "Composite file-backed environment values require an explicit local command",
                )?;
            let path = if let Some(tail) = path.strip_prefix("~/") {
                home.join(tail)
            } else {
                candidate
                    .binding
                    .source
                    .parent()
                    .context("Configuration parent missing")?
                    .join(path)
            };
            crate::platform::validate_private(&path).map_err(|_|anyhow::anyhow!("Referenced environment file must already have private permissions; original file unchanged"))?;
            candidate.env_files.insert(key.clone(), path);
        }
        *item = value(
            item,
            candidate.binding.client,
            &candidate.binding.source,
            home,
            |k| std::env::var(k).ok(),
        )?;
    }
    ensure!(
        !candidate.command.iter().any(|arg| arg.contains('\0')),
        "Command contains an invalid NUL byte"
    );
    Ok(())
}

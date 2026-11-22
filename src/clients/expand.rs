//! Resolve client substitutions as data, retaining argv boundaries and source-relative files.
use super::{Candidate, Client, descriptors::Expansion};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
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
    match client.descriptor().expansion {
        Expansion::DollarBrace => substitute(text.into(), "${", |name| {
            let (key, default) = name
                .split_once(":-")
                .map_or((name, None), |(k, v)| (k, Some(v)));
            env(key)
                .or_else(|| default.map(str::to_owned))
                .context("Required Claude environment reference is unavailable; value hidden")
        }),
        Expansion::EnvAndFile => {
            let text = substitute(text.into(), "{env:", |name| {
                env(name)
                    .context("Required OpenCode environment reference is unavailable; value hidden")
            })?;
            substitute(text, "{file:", |name| {
                let bytes = std::fs::read_to_string(file_path(name, source, home)?)
                    .context("Referenced configuration file is unavailable")?;
                Ok(bytes.trim().into())
            })
        }
        Expansion::None => Ok(text.into()),
    }
}

fn file_path(name: &str, source: &Path, home: &Path) -> Result<PathBuf> {
    if let Some(tail) = name.strip_prefix("~/") {
        Ok(home.join(tail))
    } else {
        Ok(source
            .parent()
            .context("Configuration parent missing")?
            .join(name))
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
    let expansion = candidate.binding.client.descriptor().expansion;
    for (key, item) in &mut candidate.env {
        if matches!(expansion, Expansion::EnvAndFile) && item.contains("{file:") {
            let path = item
                .strip_prefix("{file:")
                .and_then(|s| s.strip_suffix('}'))
                .context(
                    "Composite file-backed environment values require an explicit local command",
                )?;
            let path = file_path(path, &candidate.binding.source, home)?;
            ensure!(
                path.is_file(),
                "Referenced environment file for {key} is missing: {}. Restore it, then rerun setup; contents hidden",
                path.display()
            );
            crate::platform::validate_private(&path).map_err(|_| anyhow::anyhow!("Referenced environment file for {key} is not private: {}. Restrict access to your user in its file permissions, then rerun setup; original file unchanged", path.display()))?;
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

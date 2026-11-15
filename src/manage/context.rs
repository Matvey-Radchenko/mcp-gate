//! Resolve an unknown launch directory only from an explicit user decision.
use super::ui;
use crate::clients::Candidate;
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};

pub const NEEDS_DIRECTORY: &str =
    "needs a working directory — the interactive setup will ask, or pass --cwd <dir>";

pub fn needs_directory(candidate: &Candidate) -> bool {
    candidate.binding.project.is_none()
        && candidate
            .issue
            .as_deref()
            .is_some_and(|i| i.starts_with("Global command"))
}

/// Expand `~` and require an existing absolute directory. Home is not assumed.
pub fn resolve_directory(path: &Path) -> Result<PathBuf> {
    let text = path.to_string_lossy();
    let path = if text == "~" {
        super::store::home()?
    } else if let Some(tail) = text.strip_prefix("~/") {
        super::store::home()?.join(tail)
    } else {
        path.to_path_buf()
    };
    let path = crate::platform::project_path(&path).map_err(|_| {
        anyhow::anyhow!(
            "Working directory must be an existing absolute directory: {}",
            path.display()
        )
    })?;
    ensure!(
        path.is_dir(),
        "Working directory must be an existing absolute directory: {}",
        path.display()
    );
    Ok(path)
}

pub fn apply_directory(candidate: &mut Candidate, cwd: &Path) -> Result<()> {
    if !needs_directory(candidate) {
        return Ok(());
    }
    candidate.cwd = cwd.to_path_buf();
    candidate.issue = None;
    candidate.recipe = crate::clients::recipes::matching(candidate);
    if let Err(error) = super::launch::environment(candidate)
        .and_then(|env| super::launch::executable(candidate, &env))
    {
        candidate.issue = Some(error.to_string());
    }
    Ok(())
}

pub fn pending_directories(candidates: &[Candidate]) -> Result<()> {
    let pending: Vec<_> = candidates
        .iter()
        .filter(|candidate| needs_directory(candidate))
        .map(|candidate| format!("{} / {}", candidate.binding.client, candidate.binding.name))
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let names = if pending.len() == 1 {
        format!("{} needs a working directory.", pending[0])
    } else {
        format!(
            "These connections need a working directory: {}.",
            pending.join(", ")
        )
    };
    anyhow::bail!(
        "{names} Pass --cwd <dir> to set one for every selected connection that does not declare a directory."
    )
}

pub fn review(mut candidate: Candidate) -> Result<Option<Candidate>> {
    if !needs_directory(&candidate) {
        return Ok(Some(candidate));
    }
    println!(
        "\n{} / {}: the client does not declare a fixed working directory.",
        candidate.binding.client,
        ui::safe(&candidate.binding.name)
    );
    println!(
        "A personal gateway uses one directory in every project. Choose this only if that matches how you use this MCP."
    );
    println!(
        "For a project-dependent Claude Code MCP, go back and choose a project scope. Verified project-local overrides for Codex/OpenCode are not available in this release."
    );
    if ui::select(
        "Backend working directory:",
        vec![
            "Specify a fixed directory for this personal connection".into(),
            "Keep this MCP direct".into(),
        ],
    )? != Some(0)
    {
        return Ok(None);
    }
    let Some(cwd) = ui::directory("Fixed backend directory (absolute path):")? else {
        return Ok(None);
    };
    candidate.cwd = cwd;
    candidate.issue = None;
    candidate.recipe = crate::clients::recipes::matching(&candidate);
    let check = super::launch::environment(&candidate)
        .and_then(|env| super::launch::executable(&candidate, &env));
    if let Err(error) = check {
        println!(
            "Cannot connect this MCP yet: {}",
            ui::safe(&error.to_string())
        );
        return Ok(None);
    }
    Ok(Some(candidate))
}

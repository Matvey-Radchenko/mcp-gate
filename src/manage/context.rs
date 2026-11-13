//! Resolve an unknown launch directory only from an explicit user decision.
use super::ui;
use crate::clients::Candidate;
use anyhow::Result;

pub fn needs_directory(candidate: &Candidate) -> bool {
    candidate.binding.project.is_none()
        && candidate
            .issue
            .as_deref()
            .is_some_and(|i| i.starts_with("Global command"))
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

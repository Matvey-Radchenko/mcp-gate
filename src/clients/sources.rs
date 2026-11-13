//! Bounded configuration locations and physical identity; home is never a project.
use super::Client;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) struct Source {
    pub path: PathBuf,
    pub project: bool,
    pub section: &'static str,
}

pub fn identity(path: &Path) -> PathBuf {
    crate::platform::project_path(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn paths(
    home: &Path,
    project: &Path,
    client: Client,
    selected: bool,
    personal: Option<&Vec<PathBuf>>,
) -> Vec<Source> {
    let (defaults, project_paths, section) = match client {
        Client::Codex => (
            vec![home.join(".codex/config.toml")],
            vec![project.join(".codex/config.toml")],
            "mcp_servers",
        ),
        Client::Opencode => (
            vec![
                home.join(".config/opencode/opencode.json"),
                home.join(".config/opencode/opencode.jsonc"),
            ],
            vec![
                project.join("opencode.json"),
                project.join("opencode.jsonc"),
            ],
            "mcp",
        ),
        Client::ClaudeCode => (
            vec![home.join(".claude.json")],
            vec![project.join(".mcp.json")],
            "mcpServers",
        ),
    };
    let mut paths: Vec<_> = personal
        .unwrap_or(&defaults)
        .iter()
        .map(|path| Source {
            path: path.clone(),
            project: false,
            section,
        })
        .collect();
    if selected && identity(home) != identity(project) {
        paths.extend(project_paths.into_iter().map(|path| Source {
            path,
            project: true,
            section,
        }));
    }
    // Prefer the personal scope when a project path aliases that same file.
    let mut seen = BTreeSet::new();
    paths.retain(|source| seen.insert(identity(&source.path)));
    paths
}

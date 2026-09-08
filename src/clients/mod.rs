mod claude;
pub mod document;
pub mod expand;
pub mod recipes;
mod scan;
pub use scan::{discover, discover_current};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum Client {
    Codex,
    Opencode,
    ClaudeCode,
}
impl std::fmt::Display for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::ClaudeCode => "claude-code",
        })
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Binding {
    pub client: Client,
    pub name: String,
    pub source: PathBuf,
    pub source_path: Vec<String>,
    pub target: PathBuf,
    pub path: Vec<String>,
    pub toml: bool,
    pub before: Value,
    pub direct: Value,
    pub project: Option<PathBuf>,
    pub after: Value,
}

pub struct Candidate {
    pub binding: Binding,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub env_files: BTreeMap<String, PathBuf>,
    pub inherit_env: Vec<String>,
    pub issue: Option<String>,
    pub recipe: Option<recipes::Recipe>,
}

impl Candidate {
    pub fn summary(&self) -> Value {
        serde_json::json!({"client":self.binding.client,"name":self.binding.name,
            "source":self.binding.source,"target":self.binding.target,
            "transition":"stdio -> authenticated loopback HTTP", "mode": self.recipe.as_ref().map_or("session",|r|r.mode.as_str()),
            "recipe":self.recipe.as_ref().map(|r|serde_json::json!({"id":r.id,"conditions":r.conditions,"disabled_tools":r.disabled_tools})),
            "command":"preserved (values hidden)","permissions":"preserved",
            "platform_note":cfg!(target_os = "macos").then_some("macOS privacy grants of the agent application do not transfer to a background gateway; access to protected folders may require a separate system approval."),
            "autostart": if cfg!(windows) { "Task Scheduler" } else { "LaunchAgent" },
            "issue":self.issue})
    }
}

impl Binding {
    pub fn verify_fallback(&self) -> anyhow::Result<()> {
        if self.before.is_null() {
            let text = std::fs::read_to_string(&self.source)?;
            anyhow::ensure!(
                document::entry(&text, self.toml, &self.source_path)? == self.direct,
                "The original project/user MCP changed; review its settings before removing or installing a local override"
            );
        }
        Ok(())
    }
}

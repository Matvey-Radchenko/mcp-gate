use mcp_gate::manage::{service, store};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

pub struct Installation {
    _directory: tempfile::TempDir,
    pub root: PathBuf,
    pub project: PathBuf,
    pub settings: PathBuf,
    pub backend: PathBuf,
    pub original: Vec<u8>,
}
impl Installation {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project space Юникод");
        fs::create_dir(&project).unwrap();
        let project = mcp_gate::platform::project_path(&project).unwrap();
        let personal = directory.path().join("claude");
        fs::create_dir(&personal).unwrap();
        let backend = directory.path().join(if cfg!(windows) {
            "fixture.exe"
        } else {
            "fixture"
        });
        fs::copy(env!("CARGO_BIN_EXE_mock-backend"), &backend).unwrap();
        mcp_gate::platform::executable(&backend).unwrap();
        let original = serde_json::to_vec(&json!({"projects":{mcp_gate::clients::claude_project_key(&project).unwrap():{
            "mcpServers":{"fixture":{"type":"stdio","command":backend,"args":[],"env":{"FIXTURE_SECRET":"never-print-fixture-secret"}}},
            "allowedTools":[],"deniedTools":["mcp__fixture__danger"]}}})).unwrap();
        let settings = personal.join(".claude.json");
        fs::write(&settings, &original).unwrap();
        let root = directory.path().join("state");
        Self {
            _directory: directory,
            root,
            project,
            settings,
            backend,
            original,
        }
    }
    pub fn command(&self, action: &str) -> Command {
        self.command_with(std::path::Path::new(env!("CARGO_BIN_EXE_mcp-gate")), action)
    }
    pub fn command_with(&self, binary: &std::path::Path, action: &str) -> Command {
        let mut c = Command::new(binary);
        c.arg(action)
            .arg("--json")
            .env("CLAUDE_CONFIG_DIR", self.settings.parent().unwrap())
            .env("MCP_GATE_TEST_ROOT", &self.root);
        if action != "status" {
            c.args([
                "--client",
                "claude-code",
                "--server",
                "fixture",
                "--yes",
                "--project",
            ])
            .arg(&self.project);
        }
        c
    }
    pub fn run(&self, action: &str) -> Value {
        let output = self.command(action).output().unwrap();
        Self::check(&output);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    pub fn check(output: &Output) {
        assert!(!String::from_utf8_lossy(&output.stdout).contains("never-print-fixture-secret"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("never-print-fixture-secret"));
    }
    pub fn journals(&self) -> Vec<store::Journal> {
        fs::read_dir(self.root.join("operations"))
            .unwrap()
            .map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap())
            .collect()
    }
}
impl Drop for Installation {
    fn drop(&mut self) {
        if self.root.join("operations").exists() {
            for j in self.journals() {
                for record in j.services {
                    let _ = service::unregister(&record);
                }
            }
        }
        if let Ok(registry) = store::load(&self.root) {
            for record in registry.gateways {
                let _ = service::unregister(&record);
            }
        }
    }
}

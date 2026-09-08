#![cfg(feature = "test-backend")]
use mcp_gate::{
    clients::document,
    manage::{service, store},
};
use serde_json::json;
use std::{fs, process::Command};
struct Installation {
    root: std::path::PathBuf,
}
impl Drop for Installation {
    fn drop(&mut self) {
        if let Ok(entries) = fs::read_dir(self.root.join("operations")) {
            for path in entries.flatten().map(|e| e.path()) {
                if let Ok(bytes) = fs::read(path)
                    && let Ok(journal) = serde_json::from_slice::<store::Journal>(&bytes)
                {
                    for record in journal.services {
                        let _ = service::unregister(&record);
                    }
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
#[test]
#[ignore = "Registers an isolated temporary user service; requires a logged-in macOS/Windows user session"]
fn actual_service_setup_repeat_remove_without_changing_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project space Юникод");
    fs::create_dir(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let personal = dir.path().join("claude");
    fs::create_dir(&personal).unwrap();
    let root = dir.path().join("state");
    let _cleanup = Installation { root: root.clone() };
    let source = json!({"projects":{project.to_str().unwrap():{"mcpServers":{"fixture":{
        "type":"stdio","command":env!("CARGO_BIN_EXE_mock-backend"),"args":[],"env":{"FIXTURE_SECRET":"do-not-print-this"}
    }},"allowedTools":[],"deniedTools":["mcp__fixture__danger"]}}});
    let settings = personal.join(".claude.json");
    fs::write(&settings, source.to_string()).unwrap();
    let shared = project.join(".mcp.json");
    let untouched = b"{\"mcpServers\":{}}\n";
    fs::write(&shared, untouched).unwrap();
    let run = |command: &str, dry: bool| {
        let mut process = Command::new(env!("CARGO_BIN_EXE_mcp-gate"));
        process
            .args([
                command,
                "--client",
                "claude-code",
                "--server",
                "fixture",
                "--yes",
                "--json",
                "--project",
            ])
            .arg(&project)
            .env("CLAUDE_CONFIG_DIR", &personal)
            .env("MCP_GATE_TEST_ROOT", &root);
        if dry {
            process.arg("--dry-run");
        }
        let output = process.output().unwrap();
        if !output.status.success()
            && let Ok(releases) = fs::read_dir(root.join("releases"))
        {
            for release in releases.flatten() {
                if let Ok(log) = fs::read_to_string(release.path().join("logs/stderr.log")) {
                    eprintln!("Fixture service: {log}");
                }
            }
        }
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("do-not-print-this"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("do-not-print-this"));
        output.stdout
    };
    run("setup", true);
    assert!(!root.exists());
    let output = run("setup", false);
    let result: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["restart_clients"], json!(["claude-code"]));
    let registry = store::load(&root).unwrap();
    assert_eq!(registry.gateways.len(), 1);
    let record = &registry.gateways[0];
    assert!(service::installed(record).unwrap());
    let config = mcp_gate::config::Config::load(&record.config()).unwrap();
    assert_eq!(
        config.backend.working_directory.as_deref(),
        Some(project.as_path())
    );
    let token = config.token().unwrap();
    run("setup", false);
    let repeated = store::load(&root).unwrap();
    assert_eq!(repeated.gateways[0].id, record.id);
    assert_eq!(
        mcp_gate::config::Config::load(&repeated.gateways[0].config())
            .unwrap()
            .token()
            .unwrap(),
        token
    );
    assert_eq!(fs::read(&shared).unwrap(), untouched);
    run("remove", true);
    assert!(service::installed(record).unwrap());
    run("remove", false);
    assert!(store::load(&root).unwrap().gateways.is_empty());
    assert!(!service::installed(record).unwrap());
    assert_eq!(
        document::parse(&fs::read_to_string(&settings).unwrap(), false).unwrap(),
        source
    );
    assert_eq!(fs::read(&shared).unwrap(), untouched);
    assert!(record.release.exists());
}

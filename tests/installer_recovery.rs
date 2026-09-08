#![cfg(feature = "test-backend")]
#[path = "support/installation.rs"]
mod installation;
use installation::Installation;
use mcp_gate::manage::{service, store};
use serde_json::{Value, json};
use std::fs;

#[test]
#[ignore = "Registers isolated temporary user services; requires a logged-in macOS/Windows user session"]
fn failures_restore_owned_changes_and_allow_retry() {
    let fixture = Installation::new();
    for stage in [
        "prepared",
        "registered",
        "ready",
        "client-written",
        "registry-written",
    ] {
        let output = fixture
            .command("setup")
            .env("MCP_GATE_TEST_FAIL", stage)
            .output()
            .unwrap();
        Installation::check(&output);
        assert!(!output.status.success(), "Fault {stage} did not trigger");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("previous settings restored"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&fixture.settings).unwrap(), fixture.original);
        assert!(store::load(&fixture.root).unwrap().gateways.is_empty());
        for journal in fixture.journals() {
            assert_eq!(journal.phase, "restored");
            for record in journal.services {
                assert!(
                    !service::installed(&record).unwrap(),
                    "Stage {stage}, service {}: {}",
                    record.id,
                    service::diagnostics(&record)
                );
            }
        }
    }
    fixture.run("setup");
    let before = fs::read(&fixture.settings).unwrap();
    for stage in ["upgrade-stopped", "upgrade-registered", "upgrade-ready"] {
        let mut registry = store::load(&fixture.root).unwrap();
        registry.gateways[0].binary_hash = "previous-fixture-build".into();
        store::save(&fixture.root, &registry).unwrap();
        let output = fixture
            .command("setup")
            .env("MCP_GATE_TEST_FAIL", stage)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("previous settings/service restored"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&fixture.settings).unwrap(), before);
        let status = fixture.run("status");
        assert_eq!(status["gateways"][0]["health"]["workers"], 0);
        assert_eq!(status["operations"], json!([]));
    }
    let updated = fixture.run("setup");
    assert_eq!(updated["restart_clients"], json!(["claude-code"]));
    fixture.run("remove");
}

#[test]
#[ignore = "Registers isolated temporary user services; requires a logged-in macOS/Windows user session"]
fn unavailable_previous_backend_does_not_trap_future_setup() {
    let fixture = Installation::new();
    fixture.run("setup");
    let backend_bytes = fs::read(&fixture.backend).unwrap();
    fs::remove_file(&fixture.backend).unwrap();
    let failed = fixture.command("setup").output().unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("could not be restored"));
    let status = fixture.run("status");
    assert_eq!(
        status["operations"][0]["phase"],
        "restored-service-unavailable"
    );
    fs::write(&fixture.backend, backend_bytes).unwrap();
    mcp_gate::platform::executable(&fixture.backend).unwrap();
    fixture.run("setup");
    assert_eq!(fixture.run("status")["operations"], json!([]));
    fixture.run("remove");
}

#[test]
#[ignore = "Registers an isolated temporary user service; requires a logged-in macOS/Windows user session"]
fn removing_one_client_keeps_other_connection_and_service() {
    let fixture = Installation::new();
    fixture.run("setup");
    let mut registry = store::load(&fixture.root).unwrap();
    let record = &mut registry.gateways[0];
    // A second independently configured client owns a binding to this same
    // endpoint. Removal must use binding ownership, not the selected client count.
    let mut second = record.bindings[0].clone();
    second.client = mcp_gate::clients::Client::Codex;
    second.toml = true;
    second.project = None;
    second.source = fixture.project.join("personal-codex.toml");
    second.target = second.source.clone();
    second.path = vec!["mcp_servers".into(), "fixture".into()];
    second.source_path = second.path.clone();
    second.before = json!({"command":fixture.backend,"args":[],"enabled_tools":["echo"]});
    second.direct = second.before.clone();
    let config = mcp_gate::config::Config::load(&record.config()).unwrap();
    second.after = json!({"url":format!("http://{}/mcp",config.listen),"enabled_tools":["echo"]});
    let text = toml::to_string(&json!({"mcp_servers":{"fixture":second.after}})).unwrap();
    fs::write(&second.target, &text).unwrap();
    record.bindings.push(second.clone());
    let record = record.clone();
    store::save(&fixture.root, &registry).unwrap();
    let pid = fixture.run("status")["gateways"][0]["health"]["pid"].clone();
    assert_eq!(
        fixture.run("remove")["restart_clients"],
        json!(["claude-code"])
    );
    let remaining = store::load(&fixture.root).unwrap();
    assert_eq!(remaining.gateways[0].bindings.len(), 1);
    assert!(service::installed(&record).unwrap());
    assert_eq!(fixture.run("status")["gateways"][0]["health"]["pid"], pid);
    assert_eq!(fs::read_to_string(&second.target).unwrap(), text);
    let original: Value = serde_json::from_slice(&fixture.original).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&fixture.settings).unwrap()).unwrap(),
        original
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
        .args([
            "remove", "--client", "codex", "--server", "fixture", "--yes", "--json",
        ])
        .env("MCP_GATE_TEST_ROOT", &fixture.root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(store::load(&fixture.root).unwrap().gateways.is_empty());
    assert!(!service::installed(&record).unwrap());
}

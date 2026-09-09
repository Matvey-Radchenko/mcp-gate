//! Two production release builds; no test-only state-root or fault hooks.
#![cfg(feature = "test-backend")]
#[path = "support/installation.rs"]
#[allow(dead_code, reason = "Shared temporary service cleanup fixture")]
mod installation;
#[allow(dead_code, reason = "Shared isolated MCP protocol fixture")]
mod support;
use installation::Installation;
use mcp_gate::{config::Config, manage::store};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn run(fixture: &Installation, binary: &Path, action: &str) -> Value {
    let home = fixture.settings.parent().unwrap();
    let output = fixture
        .command_with(binary, action)
        .env_remove("MCP_GATE_TEST_ROOT")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        .output()
        .unwrap();
    Installation::check(&output);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn health(fixture: &Installation, binary: &Path, version: &str) -> Value {
    let result = run(fixture, binary, "status");
    assert_eq!(result["operations"], json!([]));
    assert_eq!(result["gateways"][0]["issues"], json!([]));
    let h = result["gateways"][0]["health"].clone();
    assert_eq!(h["version"], version);
    h
}

#[tokio::test]
#[ignore = "Requires MCP_GATE_RELEASE_BINARY and MCP_GATE_COMPAT_BINARY; registers only a temporary user service"]
async fn separately_versioned_releases_update_and_downgrade_without_replay() {
    let current = std::env::var_os("MCP_GATE_RELEASE_BINARY").unwrap();
    let current = Path::new(&current);
    let previous = std::env::var_os("MCP_GATE_COMPAT_BINARY").unwrap();
    let previous = Path::new(&previous);
    let version = env!("CARGO_PKG_VERSION");
    let older = format!("{version}-compat-fixture");
    let mut fixture = Installation::new();
    let home = fixture.settings.parent().unwrap();
    fixture.root = home.join(if cfg!(windows) {
        "AppData/Local/mcp-gate"
    } else {
        "Library/Application Support/mcp-gate"
    });
    fs::create_dir_all(home.join("AppData/Local")).unwrap();
    let shared = fixture.project.join(".mcp.json");
    let shared_bytes = b"{\"mcpServers\":{}}\n";
    fs::write(&shared, shared_bytes).unwrap();
    run(&fixture, previous, "setup");
    let old_record = store::load(&fixture.root).unwrap().gateways.remove(0);
    assert!(old_record.binary.to_string_lossy().contains(&older));
    let old_hash = old_record.binary_hash.clone();
    let config = Config::load(&old_record.config()).unwrap();
    let token = config.token().unwrap();
    let endpoint = format!("http://{}", config.listen);
    let client_bytes = fs::read(&fixture.settings).unwrap();
    let config_bytes = fs::read(old_record.config()).unwrap();
    let artifact = config.state_dir.join("retained-user-artifact.txt");
    fs::write(&artifact, b"preserve this artifact").unwrap();
    let old_health = health(&fixture, previous, &older);
    assert_eq!(old_health["workers"], 0);
    let mut session = support::Session::authenticated(&endpoint, false, &token).await;
    assert_eq!(
        support::mock_value(
            &session
                .call("state", json!({"value":"previous action"}))
                .await
        )["value"],
        "previous action"
    );
    let pending = run(&fixture, current, "setup");
    assert!(pending["updates"][0].as_str().unwrap().contains("pending"));
    assert_eq!(health(&fixture, previous, &older)["pid"], old_health["pid"]);
    assert_eq!(
        support::mock_value(&session.call("state", json!({})).await)["value"],
        "previous action"
    );
    session.close().await;
    let upgraded = run(&fixture, current, "setup");
    assert_eq!(upgraded["restart_clients"], json!(["claude-code"]));
    let new_record = store::load(&fixture.root).unwrap().gateways.remove(0);
    assert_ne!(new_record.binary_hash, old_hash);
    assert_ne!(new_record.binary, old_record.binary);
    let new_health = health(&fixture, current, version);
    assert_ne!(new_health["pid"], old_health["pid"]);
    assert_eq!(new_health["workers"], 0);
    assert_eq!(new_health["sessions"], 0);
    // The previous gateway closed its HTTP sockets. Probe the old MCP session
    // ID over a fresh transport, without retrying any possibly executed action.
    session.client = reqwest::Client::new();
    assert_eq!(
        session
            .post(json!({"jsonrpc":"2.0","id":90,"method":"tools/list","params":{}}))
            .await
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let fresh = support::Session::authenticated(&endpoint, false, &token).await;
    assert_eq!(
        support::mock_value(&fresh.call("state", json!({})).await)["value"],
        Value::Null,
        "The old action must not be replayed into a new backend"
    );
    fresh.close().await;
    run(&fixture, previous, "setup");
    let restored = store::load(&fixture.root).unwrap().gateways.remove(0);
    assert_eq!(restored.binary_hash, old_hash);
    assert_eq!(restored.binary, old_record.binary);
    assert_eq!(restored.id, old_record.id);
    let restored_health = health(&fixture, previous, &older);
    assert_ne!(restored_health["pid"], new_health["pid"]);
    assert_eq!(restored_health["workers"], 0);
    assert_eq!(restored_health["sessions"], 0);
    assert_eq!(fs::read(restored.config()).unwrap(), config_bytes);
    assert_eq!(fs::read(&fixture.settings).unwrap(), client_bytes);
    assert_eq!(fs::read(&artifact).unwrap(), b"preserve this artifact");
    assert_eq!(fs::read(&shared).unwrap(), shared_bytes);
    run(&fixture, previous, "setup");
    assert_eq!(
        health(&fixture, previous, &older)["pid"],
        restored_health["pid"]
    );
    run(&fixture, previous, "remove");
    assert!(store::load(&fixture.root).unwrap().gateways.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&fixture.settings).unwrap()).unwrap(),
        serde_json::from_slice::<Value>(&fixture.original).unwrap()
    );
    assert_eq!(fs::read(&shared).unwrap(), shared_bytes);
    assert!(old_record.binary.exists());
    assert!(new_record.binary.exists());
    assert!(artifact.exists());
}

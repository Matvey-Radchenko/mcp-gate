#![cfg(feature = "test-backend")]
#[allow(dead_code, reason = "Fixtures are shared with lifecycle suites")]
mod support;

use mcp_gate::config::Config;
use serde_json::json;
use std::sync::atomic::Ordering;
use support::{Harness, content};

#[tokio::test]
async fn default_shared_backend_wire_matches_baseline() {
    let mut h = Harness::shared_with(4, |config| {
        let wire = config.state_dir.with_file_name("serial-wire");
        config
            .backend
            .env
            .insert("MOCK_CORE_CATALOG".into(), "1".into());
        config
            .backend
            .env
            .insert("MOCK_RAW_WIRE_FILE".into(), wire.to_str().unwrap().into());
    })
    .await;
    assert_default_metadata(&h).await;
    let path = h.config.with_file_name("serial-wire");
    // Exclude the explicit catalog command. The first call verifies the same
    // catalog again during lazy startup.
    std::fs::remove_file(&path).unwrap();
    let a = h.session().await;
    let first = a.call("state", json!({"value":"baseline"})).await;
    assert!(first["result"].is_object());
    let second = a.request("tools/call", json!({
        "name":"pause", "arguments":{"delay_ms":100}, "_meta":{"progressToken":"test-progress"}
    })).await;
    assert_eq!(a.progress.load(Ordering::SeqCst), 1);
    assert_eq!(content(&second), "done");
    for (method, params) in [
        ("resources/read", json!({"uri":"fixture://first"})),
        (
            "prompts/get",
            json!({"name":"echo","arguments":{"text":"baseline"}}),
        ),
    ] {
        assert!(a.request(method, params).await["result"].is_object());
    }
    h.stop();
    let wire = std::fs::read(path).unwrap();
    // Captured from 9180c4e with no normalization.
    assert_eq!(
        std::str::from_utf8(&wire).unwrap(),
        include_str!("fixtures/serial_wire.golden")
    );
}

async fn assert_default_metadata(h: &Harness) {
    let saved = std::fs::read_to_string(&h.config).unwrap();
    assert!(!saved.contains("max_concurrent_calls"));
    assert_eq!(
        toml::to_string(&Config::load(&h.config).unwrap()).unwrap(),
        saved
    );
    assert_eq!(
        h.health().await.unwrap()["runtime"],
        json!({
            "ownership":"shared", "shared_client_roots":"reject", "concurrency_per_backend":1,
            "max_pending_calls":4, "queue_timeout_seconds":20, "idle_seconds":null,
            "shared_state":"dormant",
            "recovery":{"state":"dormant", "policy":"manual", "generation":0, "phase":"ready",
                "retry_after_seconds":0, "last_failure":null}
        })
    );
}

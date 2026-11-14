#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared fixture helpers are used by separate suites"
)]
mod support;
use mcp_gate::config::{Config, SharedRecovery};
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};
use support::*;

async fn recovering(timeout: u64) -> Harness {
    let mut h = Harness::generic("shared", 4, timeout, 15).await;
    h.stop();
    let mut c = Config::load(&h.config).unwrap();
    c.shared_recovery = SharedRecovery::Restart;
    c.backend.env.insert(
        "MOCK_DISCOVERY_OFFLINE_FILE".into(),
        c.state_dir
            .join("discovery-offline")
            .to_string_lossy()
            .into_owned(),
    );
    c.backend.env.insert(
        "MOCK_ACTION_JOURNAL".into(),
        c.state_dir.join("actions").to_string_lossy().into_owned(),
    );
    c.backend.env.insert(
        "MOCK_OFFLINE_FILE".into(),
        c.state_dir.join("offline").to_string_lossy().into_owned(),
    );
    h.restart_with_config(c).await;
    h
}

async fn ready(h: &Harness) -> Value {
    for _ in 0..200 {
        let health = h.health().await.unwrap();
        if health["runtime"]["shared_state"] == "dormant" {
            return health;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Gateway did not recover: {:?}", h.health().await);
}

async fn pause(session: &Session) -> tokio::task::JoinHandle<Value> {
    let c = session.clone();
    let task = tokio::spawn(async move {
        c.request("tools/call", json!({"name":"pause","arguments":{"delay_ms":10000},"_meta":{"progressToken":"test-progress"}})).await
    });
    for _ in 0..100 {
        if session.progress.load(Ordering::SeqCst) > 0 {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Pause was not dispatched");
}

async fn assert_fresh_clients(h: &Harness, old_pid: u32) {
    let (b, c) = tokio::join!(h.session(), h.session());
    let (br, cr) = tokio::join!(b.call("state", json!({})), c.call("state", json!({})));
    let br = mock_value(&br);
    let cr = mock_value(&cr);
    assert_eq!(br["pid"], cr["pid"]);
    assert_ne!(br["pid"], old_pid);
    assert!(br["value"].is_null());
    assert!(!alive(old_pid));
    h.workers(1).await;
    // Late cleanup of the old generation must never take the new worker.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        mock_value(&b.call("state", json!({})).await)["pid"],
        br["pid"]
    );
    b.close().await;
    c.close().await;
}

#[tokio::test]
async fn timeout_recovers_without_replaying_and_rejects_old_queue() {
    let mut h = recovering(1).await;
    let a = h.session().await;
    let b = h.session().await;
    let pid = mock_value(
        &a.call("state", json!({"value":"lost-volatile-state"}))
            .await,
    )["pid"]
        .as_u64()
        .unwrap() as u32;
    let running = pause(&a).await;
    let queued =
        tokio::spawn(async move { b.call("state", json!({"value":"must-not-execute"})).await });
    assert!(content(&running.await.unwrap()).contains("NOT retried"));
    let queued = queued.await.unwrap();
    assert!(content(&queued).contains("no action was dispatched"));
    let status = ready(&h).await;
    assert_eq!(
        status["runtime"]["recovery"]["last_failure"]["reason"],
        "timeout"
    );
    assert_fresh_clients(&h, pid).await;
    let config = Config::load(&h.config).unwrap();
    let actions = std::fs::read_to_string(config.state_dir.join("actions")).unwrap();
    assert_eq!(actions.lines().filter(|s| *s == "pause").count(), 1);
    assert_eq!(actions.lines().filter(|s| *s == "state").count(), 4);
    let journal = config.state_dir.join("recovery.jsonl");
    mcp_gate::platform::validate_private(&journal).unwrap();
    let history = std::fs::read_to_string(journal).unwrap();
    assert!(history.contains("timeout"));
    assert!(!history.contains("lost-volatile-state"));
    assert!(!history.contains(TOKEN));
    h.stop();
}

#[tokio::test]
async fn explicit_cancellation_and_client_close_recover() {
    for close in [false, true] {
        let mut h = recovering(15).await;
        let a = h.session().await;
        let pid = mock_value(&a.call("state", json!({})).await)["pid"]
            .as_u64()
            .unwrap() as u32;
        let id = a.next.load(Ordering::SeqCst);
        let running = pause(&a).await;
        if close {
            a.close().await;
        } else {
            a.post(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}})).await;
        }
        tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap();
        let status = ready(&h).await;
        assert!(matches!(
            status["runtime"]["recovery"]["last_failure"]["reason"].as_str(),
            Some("cancelled" | "handler_dropped")
        ));
        assert_fresh_clients(&h, pid).await;
        h.stop();
    }
}

#[tokio::test]
async fn crash_recovers_but_catalog_drift_stays_blocked() {
    let mut h = recovering(10).await;
    let a = h.session().await;
    let pid = mock_value(&a.call("state", json!({})).await)["pid"]
        .as_u64()
        .unwrap() as u32;
    a.call("crash", json!({})).await;
    ready(&h).await;
    assert_fresh_clients(&h, pid).await;
    a.call("state", json!({"notify_catalog":"changed"})).await;
    assert!(content(&a.call("state", json!({})).await).contains("catalog changed"));
    h.workers(0).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        h.health().await.unwrap()["runtime"]["shared_state"],
        "failed"
    );
    assert!(content(&a.call("state", json!({})).await).contains("catalog changed"));
    h.stop();
}

#[tokio::test]
async fn network_errors_preserve_backend_and_resume_when_connection_returns() {
    let mut h = recovering(5).await;
    let a = h.session().await;
    let first = mock_value(&a.call("state", json!({"value":"retained"})).await);
    let offline = Config::load(&h.config).unwrap().state_dir.join("offline");
    std::fs::write(&offline, "fixture").unwrap();
    for _ in 0..5 {
        assert_eq!(a.call("state", json!({})).await["result"]["isError"], true);
    }
    assert_eq!(
        h.health().await.unwrap()["runtime"]["shared_state"],
        "running"
    );
    std::fs::remove_file(offline).unwrap();
    assert!(
        a.call("state", json!({"protocol_error":true}))
            .await
            .get("error")
            .is_some()
    );
    let later = mock_value(&a.call("state", json!({})).await);
    assert_eq!(later["pid"], first["pid"]);
    assert_eq!(later["value"], "retained");
    assert!(h.health().await.unwrap()["runtime"]["recovery"]["last_failure"].is_null());
    h.stop();
}

#[tokio::test]
async fn dropped_http_response_keeps_shared_call_and_process() {
    let mut h = recovering(10).await;
    let a = h.session().await;
    let first = mock_value(&a.call("state", json!({})).await);
    let get = a.req(reqwest::Method::GET).send().await.unwrap();
    let response = a.post(json!({"jsonrpc":"2.0","id":900,"method":"tools/call","params":{"name":"pause","arguments":{"delay_ms":1200}}})).await;
    drop(response);
    drop(get);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        mock_value(&a.call("state", json!({})).await)["pid"],
        first["pid"]
    );
    assert!(h.health().await.unwrap()["runtime"]["recovery"]["last_failure"].is_null());
    h.stop();
}

#[tokio::test]
async fn unavailable_discovery_recovers_without_claiming_catalog_drift() {
    let mut h = recovering(5).await;
    let config = Config::load(&h.config).unwrap();
    let offline = config.state_dir.join("discovery-offline");
    std::fs::write(&offline, "fixture").unwrap();
    let a = h.session().await;
    assert!(content(&a.call("state", json!({})).await).contains("discovery failed"));
    let health = ready(&h).await;
    assert_eq!(
        health["runtime"]["recovery"]["last_failure"]["reason"],
        "startup_failed"
    );
    assert!(!config.state_dir.join("actions").exists());
    std::fs::remove_file(offline).unwrap();
    assert!(mock_value(&a.call("state", json!({})).await)["pid"].is_number());
    h.stop();
}

#[tokio::test]
async fn repeated_failures_open_cooldown_without_respawning_on_every_request() {
    let mut h = recovering(5).await;
    let a = h.session().await;
    for attempt in 0..3 {
        assert!(content(&a.call("crash", json!({})).await).contains("NOT retried"));
        if attempt < 2 {
            ready(&h).await;
        }
    }
    h.workers(0).await;
    let health = h.health().await.unwrap();
    assert_eq!(health["runtime"]["shared_state"], "recovering");
    assert!(
        health["runtime"]["recovery"]["retry_after_seconds"]
            .as_u64()
            .unwrap()
            > 20
    );
    for _ in 0..5 {
        assert!(content(&a.call("state", json!({})).await).contains("no action was dispatched"));
    }
    h.workers(0).await;
    let actions =
        std::fs::read_to_string(Config::load(&h.config).unwrap().state_dir.join("actions"))
            .unwrap();
    assert_eq!(actions.lines().count(), 3);
    h.stop();
}

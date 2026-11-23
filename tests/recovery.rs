#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared fixture helpers are used by separate suites"
)]
mod support;
use mcp_gate::config::{Config, SharedRecovery};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Duration,
};
use support::*;

const CANCEL_ID: u64 = 900;
const WAIT: Duration = Duration::from_secs(8);

struct CancellationFixture {
    gateway: Harness,
    wire: PathBuf,
    started: PathBuf,
    release: PathBuf,
}

impl CancellationFixture {
    async fn new(timeout: u64, policy: SharedRecovery) -> Self {
        let gateway = Harness::shared_with(1, |config| {
            config.call_timeout_seconds = timeout;
            config.queue_timeout_seconds = 1;
            config.shared_recovery = policy;
            config
                .backend
                .env
                .insert("MOCK_IGNORE_CANCELLATION".into(), "1".into());
            for (key, name) in [
                ("MOCK_RAW_WIRE_FILE", "wire"),
                ("MOCK_ECHO_JOURNAL", "echo-events"),
                ("MOCK_ECHO_GATE_DIR", "echo-gates"),
            ] {
                let path = config.state_dir.with_file_name(name);
                config
                    .backend
                    .env
                    .insert(key.into(), path.to_str().unwrap().into());
            }
        })
        .await;
        let wire = gateway.config.with_file_name("wire");
        let started = gateway.config.with_file_name("echo-events");
        let gates = gateway.config.with_file_name("echo-gates");
        std::fs::create_dir(&gates).unwrap();
        Self {
            gateway,
            wire,
            started,
            release: gates.join("0"),
        }
    }

    async fn start_call(&self, client: &Session) -> (reqwest::Response, Value) {
        let response = client
            .post(json!({
                "jsonrpc":"2.0","id":CANCEL_ID,"method":"tools/call",
                "params":{"name":"echo_delay","arguments":{"nonce":0,"hold":true}}
            }))
            .await;
        let start = observe(&self.started, |entry| entry["event"] == "start").await;
        (response, start["id"].clone())
    }

    async fn assert_forwarded(&self, upstream_id: &Value) {
        let notification = observe(&self.wire, |entry| {
            entry["method"] == "notifications/cancelled"
        })
        .await;
        assert_ne!(upstream_id, CANCEL_ID);
        assert_eq!(&notification["params"]["requestId"], upstream_id);
    }
}

async fn observe(path: &Path, matches: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(entry) = records(path).into_iter().find(&matches) {
                return entry;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("expected a backend fixture event")
}

async fn cancel(client: &Session) {
    let notification = client
        .post(json!({"jsonrpc":"2.0","method":"notifications/cancelled",
            "params":{"requestId":CANCEL_ID}}))
        .await;
    assert_eq!(notification.status(), reqwest::StatusCode::ACCEPTED);
}

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
    wait_for_state(h, "dormant").await
}

async fn wait_for_state(h: &Harness, state: &str) -> Value {
    for _ in 0..200 {
        let health = h.health().await.unwrap();
        if health["runtime"]["shared_state"] == state {
            return health;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("Gateway did not reach {state}: {:?}", h.health().await);
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
async fn shared_cancellation_drains_without_recovery() {
    let mut f = CancellationFixture::new(15, SharedRecovery::Manual).await;
    let (a, b) = tokio::join!(f.gateway.session(), f.gateway.session());
    let first = mock_value(&a.call("state", json!({"value":"retained"})).await);
    let (response, upstream_id) = f.start_call(&a).await;
    cancel(&a).await;
    assert_cancelled_stream(&a, response).await;
    assert_call_drains(&f, &b, &upstream_id, &first).await;
    f.gateway.stop();
}

#[tokio::test]
async fn shared_session_delete_drains_without_recovery() {
    let mut f = CancellationFixture::new(15, SharedRecovery::Manual).await;
    let (a, b) = tokio::join!(f.gateway.session(), f.gateway.session());
    let first = mock_value(&a.call("state", json!({"value":"retained"})).await);
    let (response, upstream_id) = f.start_call(&a).await;
    a.close().await;
    assert_cancelled_stream(&a, response).await;
    assert_call_drains(&f, &b, &upstream_id, &first).await;
    f.gateway.stop();
}

#[tokio::test]
async fn shared_http_disconnect_drains_without_recovery() {
    let mut f = CancellationFixture::new(15, SharedRecovery::Manual).await;
    let (a, b) = tokio::join!(f.gateway.session(), f.gateway.session());
    let first = mock_value(&a.call("state", json!({"value":"retained"})).await);
    let (response, upstream_id) = f.start_call(&a).await;
    drop(response);
    assert_call_drains(&f, &b, &upstream_id, &first).await;
    f.gateway.stop();
}

async fn assert_cancelled_stream(client: &Session, response: reqwest::Response) {
    let result = tokio::time::timeout(WAIT, client.read_message(response, Some(CANCEL_ID)))
        .await
        .expect("cancelled client should stop waiting before the backend replies");
    assert_eq!(result, json!({"transportEnded":true}));
}

async fn assert_call_drains(
    f: &CancellationFixture,
    other: &Session,
    upstream_id: &Value,
    first: &Value,
) {
    f.assert_forwarded(upstream_id).await;
    // The frontend is finished while the backend is still holding the gate.
    let queued = other
        .call("state", json!({"value":"must-not-execute"}))
        .await;
    assert!(content(&queued).contains("queue wait timed out"));
    let health = f.gateway.health().await.unwrap();
    assert_eq!(health["runtime"]["shared_state"], "running");
    assert_eq!(health["workers"], 1);
    assert!(health["runtime"]["recovery"]["last_failure"].is_null());

    // Reuse the cancelled client's downstream ID on a different session.
    let response = other
        .post(json!({
            "jsonrpc":"2.0","id":CANCEL_ID,"method":"tools/call","params":{"name":"state"}
        }))
        .await;
    std::fs::write(&f.release, []).unwrap();
    let response = other.read_message(response, Some(CANCEL_ID)).await;
    assert_eq!(response["id"], CANCEL_ID);
    let result = mock_value(&response);
    assert_eq!(result["pid"], first["pid"]);
    assert_eq!(result["value"], "retained");
    let runtime = f.gateway.health().await.unwrap()["runtime"].clone();
    assert_eq!(runtime["shared_state"], "running");
    assert_eq!(runtime["recovery"]["generation"], 0);
    assert!(runtime["recovery"]["last_failure"].is_null());
}

#[tokio::test]
async fn cancelled_call_without_reply_fails_at_deadline() {
    for (policy, expected) in [
        (SharedRecovery::Manual, "failed"),
        (SharedRecovery::Restart, "dormant"),
    ] {
        let mut f = CancellationFixture::new(5, policy).await;
        let (a, b) = tokio::join!(f.gateway.session(), f.gateway.session());
        let first = mock_value(&a.call("state", json!({})).await);
        let (response, upstream_id) = f.start_call(&a).await;
        cancel(&a).await;
        assert_cancelled_stream(&a, response).await;
        f.assert_forwarded(&upstream_id).await;
        assert_eq!(
            f.gateway.health().await.unwrap()["runtime"]["shared_state"],
            "running"
        );
        // Leave the mock call gated. Only its original deadline should fail the worker.
        let health = wait_for_state(&f.gateway, expected).await;
        assert_eq!(health["workers"], 0);
        assert_eq!(
            health["runtime"]["recovery"]["last_failure"]["reason"],
            "timeout"
        );
        let next = b.call("state", json!({})).await;
        if policy == SharedRecovery::Manual {
            assert!(content(&next).contains("restart the gateway"));
        } else {
            assert_ne!(mock_value(&next)["pid"], first["pid"]);
        }
        assert_eq!(
            records(&f.started)
                .iter()
                .filter(|entry| entry["event"] == "start")
                .count(),
            1,
            "the cancelled call was never replayed"
        );
        f.gateway.stop();
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

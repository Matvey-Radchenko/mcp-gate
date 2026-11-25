#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use mcp_gate::config::Config;
use reqwest::{Method, StatusCode};
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};
use support::*;

async fn core_session_limits(idle: u64) -> Harness {
    let mut h = Harness::session_limits(1, idle, 1).await;
    let mut backend = Config::load(&h.config).unwrap().backend;
    backend.env.insert("MOCK_CORE_CATALOG".into(), "1".into());
    h.replace_backend(backend).await;
    h
}

#[tokio::test]
async fn discovery_and_ping_do_not_keep_session_backend_alive() {
    // The reaper checks leases once per second and, by design, skips a session
    // with an HTTP request in flight. Back-to-back discovery could cover every
    // tick on a slow runner, so each burst is followed by a quiet gap longer
    // than the tick. Bursts still recur well inside the idle window, so a
    // session that discovery kept alive could never expire here.
    let idle = Duration::from_secs(3);
    let quiet = Duration::from_millis(1500);
    let mut h = core_session_limits(idle.as_secs()).await;
    let a = h.session().await;
    a.call("state", json!({})).await;
    let mut last_success = tokio::time::Instant::now();
    let deadline = last_success + idle * 4;
    let closed_at = 'discovery: loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "Discovery and ping extended the session idle deadline"
        );
        tokio::time::sleep(quiet).await;
        for method in ["tools/list", "resources/list", "prompts/list", "ping"] {
            let response = a
                .post(json!({"jsonrpc":"2.0","id":100,"method":method,"params":{}}))
                .await;
            if response.status() == StatusCode::NOT_FOUND {
                break 'discovery tokio::time::Instant::now();
            }
            assert!(response.status().is_success());
            let message = a.read_message(response, Some(100)).await;
            assert!(message["result"].is_object(), "{method}: {message}");
            last_success = tokio::time::Instant::now();
        }
    };
    // Had any discovery request counted as activity, the session would still
    // be open for a full idle window after the last successful one.
    assert!(
        closed_at < last_success + idle,
        "Discovery and ping extended the session idle deadline"
    );
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
async fn every_executable_operation_extends_session_idle_from_completion() {
    let mut h = core_session_limits(3).await;
    let a = h.session().await;
    a.call("state", json!({"value":"kept"})).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let resource = a
        .request("resources/read", json!({"uri":"fixture://first"}))
        .await;
    assert!(resource["result"].is_object());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let prompt = a.request("prompts/get", json!({"name":"echo"})).await;
    assert!(prompt["result"].is_object());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        mock_value(&a.call("state", json!({})).await)["value"],
        "kept"
    );
    h.workers(1).await;
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
async fn disabled_session_idle_preserves_state_and_capacity_eviction() {
    let mut h = Harness::session_limits(1, 0, 1).await;
    let a = h.session().await;
    a.call("state", json!({"value":"old"})).await;
    tokio::time::sleep(Duration::from_millis(2200)).await;
    h.workers(1).await;
    a.request("ping", json!({})).await;
    let b = h.session().await;
    assert_eq!(
        mock_value(&b.call("state", json!({"value":"new"})).await)["value"],
        "new"
    );
    assert!(content(&a.call("state", json!({})).await).contains("Reconnect"));
    h.workers(1).await;
    b.close().await;
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
async fn open_get_stream_keeps_session_backend_alive_beyond_idle_timeout() {
    let mut h = Harness::session_limits(1, 1, 1).await;
    let a = h.session().await;
    let stream = a.req(Method::GET).send().await.unwrap();
    assert!(stream.status().is_success());
    a.call("state", json!({"value":"recording"})).await;
    tokio::time::sleep(Duration::from_millis(2200)).await;
    h.workers(1).await;
    assert_eq!(
        mock_value(&a.call("state", json!({})).await)["value"],
        "recording"
    );
    drop(stream);
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
async fn session_active_and_queued_calls_survive_idle_deadline() {
    let mut h = Harness::session_limits(1, 1, 1).await;
    let a = h.session().await;
    let caller = a.clone();
    let running = tokio::spawn(async move {
        caller
            .request(
                "tools/call",
                json!({"name":"pause", "arguments":{"delay_ms":2500},
            "_meta":{"progressToken":"test-progress"}}),
            )
            .await
    });
    for _ in 0..100 {
        if a.progress.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(a.progress.load(Ordering::SeqCst) > 0);
    let queued = tokio::spawn(async move { a.call("state", json!({"value":"queued"})).await });
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert!(!running.is_finished());
    assert!(!queued.is_finished());
    h.workers(1).await;
    assert_eq!(content(&running.await.unwrap()), "done");
    assert_eq!(mock_value(&queued.await.unwrap())["value"], "queued");
    h.workers(0).await;
    h.stop();
}

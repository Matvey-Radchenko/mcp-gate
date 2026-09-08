#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};
use support::*;

#[tokio::test]
async fn explicit_rootless_shared_policy_never_passes_client_roots_upstream() {
    let mut h = Harness::generic("shared", 2, 10, 5).await;
    h.ignore_shared_roots().await;
    let a = Session::new(&h.base).await;
    let b = Session::new(&h.base).await;
    let (av, bv) = tokio::join!(a.call("state", json!({})), b.call("state", json!({})));
    assert_eq!(mock_value(&av)["pid"], mock_value(&bv)["pid"]);
    assert_eq!(
        mock_value(&a.call("roots", json!({})).await)["roots"],
        json!([])
    );
    assert_eq!(
        mock_value(&b.call("roots", json!({})).await)["roots"],
        json!([])
    );
    h.workers(1).await;
    h.stop();
}

#[tokio::test]
async fn tool_list_notifications_revalidate_and_reject_changed_schemas() {
    for ownership in ["shared", "session"] {
        let mut h = Harness::generic(ownership, 2, 10, 5).await;
        let a = h.session().await;
        a.call("state", json!({"notify_catalog":"unchanged"})).await;
        let value = a.call("state", json!({"value":"still-valid"})).await;
        assert_eq!(mock_value(&value)["value"], "still-valid");
        a.call("state", json!({"notify_catalog":"changed"})).await;
        let response = a.call("state", json!({"value":"must-not-write"})).await;
        assert!(content(&response).contains("catalog changed"));
        h.workers(0).await;
        h.stop();
    }
}

#[tokio::test]
async fn failed_initialization_reaps_owned_process_group() {
    use std::sync::{Arc, atomic::AtomicUsize};
    let mut h = Harness::generic("shared", 2, 10, 5).await;
    h.stop();
    let mut config = mcp_gate::config::Config::load(&h.config).unwrap();
    let pids = config.state_dir.join("hung-processes");
    config.backend.env.insert(
        "MOCK_INIT_HANG_PID_FILE".into(),
        pids.to_string_lossy().into_owned(),
    );
    config.startup_timeout_seconds = 1;
    let live = Arc::new(AtomicUsize::new(0));
    let result = mcp_gate::worker::Worker::start(&config, None, None, live.clone()).await;
    assert!(result.is_err());
    assert_eq!(live.load(Ordering::SeqCst), 0);
    let pids: Vec<u32> = std::fs::read_to_string(pids)
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    for _ in 0..100 {
        if pids.iter().all(|p| !alive(*p)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Initialization failure left owned processes running: {pids:?}");
}

async fn pause(session: &Session, delay_ms: u64) -> tokio::task::JoinHandle<serde_json::Value> {
    let client = session.clone();
    let task = tokio::spawn(async move {
        client
            .request(
                "tools/call",
                json!({"name":"pause", "arguments":{"delay_ms":delay_ms},
            "_meta":{"progressToken":"test-progress"}}),
            )
            .await
    });
    for _ in 0..100 {
        if session.progress.load(Ordering::SeqCst) > 0 {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Backend did not start pause tool")
}

#[tokio::test]
async fn cold_start_singleton_response_routing_and_independent_client_close() {
    let mut h = Harness::generic("shared", 4, 10, 5).await;
    let (a, b) = tokio::join!(h.session(), h.session());
    a.request("tools/list", json!({})).await;
    h.workers(0).await;
    // Both clients use the same request numbers, but must receive their own result.
    let (av, bv) = tokio::join!(
        a.call("state", json!({"value":"A"})),
        b.call("state", json!({"value":"B"}))
    );
    assert_eq!(mock_value(&av)["value"], "A");
    assert_eq!(mock_value(&bv)["value"], "B");
    let pid = mock_value(&av)["pid"].as_u64().unwrap() as u32;
    assert_eq!(mock_value(&bv)["pid"], pid);
    h.workers(1).await;
    a.close().await;
    b.close().await;
    // No frontend owners remain; the gateway must still own the shared worker.
    tokio::time::sleep(Duration::from_secs(3)).await;
    h.workers(1).await;
    let c = h.session().await;
    assert_eq!(mock_value(&c.call("state", json!({})).await)["pid"], pid);
    h.stop();
    assert!(!alive(pid));
}

#[tokio::test]
async fn same_executable_generic_session_profile_keeps_independent_state() {
    let mut h = Harness::generic("session", 4, 10, 5).await;
    let (a, b) = tokio::join!(h.session(), h.session());
    let (av, bv) = tokio::join!(
        a.call("state", json!({"value":"A"})),
        b.call("state", json!({"value":"B"}))
    );
    assert_ne!(mock_value(&av)["pid"], mock_value(&bv)["pid"]);
    assert_eq!(mock_value(&a.call("state", json!({})).await)["value"], "A");
    a.close().await;
    h.workers(1).await;
    b.close().await;
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
async fn bounded_queue_rejects_overload_without_stopping_active_call() {
    let mut h = Harness::generic("shared", 0, 10, 5).await;
    let a = h.session().await;
    let b = h.session().await;
    let running = pause(&a, 500).await;
    assert!(
        content(&b.call("state", json!({"value":"must-not-write"})).await)
            .contains("queue is full")
    );
    assert_eq!(content(&running.await.unwrap()), "done");
    assert_eq!(b.progress.load(Ordering::SeqCst), 0);
    assert_eq!(
        mock_value(&b.call("state", json!({})).await)["value"],
        serde_json::Value::Null
    );
    h.stop();
}

#[tokio::test]
async fn queued_timeout_never_dispatches_but_long_active_call_finishes() {
    let mut h = Harness::generic("shared", 2, 10, 1).await;
    let a = h.session().await;
    let b = h.session().await;
    let running = pause(&a, 1600).await;
    assert!(
        content(&b.call("state", json!({"value":"must-not-write"})).await)
            .contains("queue wait timed out")
    );
    assert_eq!(content(&running.await.unwrap()), "done");
    assert_eq!(
        mock_value(&b.call("state", json!({})).await)["value"],
        serde_json::Value::Null
    );
    h.stop();
}

#[tokio::test]
async fn queued_cancellation_preserves_shared_worker_and_does_not_dispatch() {
    let mut h = Harness::generic("shared", 2, 10, 5).await;
    let a = h.session().await;
    let b = h.session().await;
    let running = pause(&a, 800).await;
    let id = b.next.load(Ordering::SeqCst);
    let queued_client = b.clone();
    let queued = tokio::spawn(async move {
        queued_client
            .call("state", json!({"value":"must-not-write"}))
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    b.post(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}}))
        .await;
    tokio::time::timeout(Duration::from_secs(3), queued)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(content(&running.await.unwrap()), "done");
    assert_eq!(
        mock_value(&b.call("state", json!({})).await)["value"],
        serde_json::Value::Null
    );
    h.stop();
}

#[tokio::test]
async fn active_cancellation_fails_shared_state_instead_of_overlapping_calls() {
    let mut h = Harness::generic("shared", 2, 10, 5).await;
    let a = h.session().await;
    let id = a.next.load(Ordering::SeqCst);
    let running = pause(&a, 10000).await;
    a.post(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}}))
        .await;
    tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .unwrap()
        .unwrap();
    h.workers(0).await;
    let b = h.session().await;
    assert!(content(&b.call("state", json!({})).await).contains("restart the gateway"));
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
async fn active_timeout_fails_shared_state_and_new_clients_cannot_respawn_it() {
    let mut h = Harness::generic("shared", 2, 1, 5).await;
    let a = h.session().await;
    let response = a.call("pause", json!({"delay_ms":10000})).await;
    assert!(content(&response).contains("NOT retried"));
    h.workers(0).await;
    let b = h.session().await;
    assert!(content(&b.call("state", json!({})).await).contains("restart the gateway"));
    h.stop();
}

#[tokio::test]
async fn roots_are_rejected_and_backend_crash_is_sticky_across_clients() {
    let mut h = Harness::generic("shared", 2, 10, 5).await;
    let rooted = Session::new(&h.base).await;
    assert!(
        content(&rooted.call("state", json!({})).await).contains("does not accept client roots")
    );
    h.workers(0).await;
    let a = h.session().await;
    a.call("crash", json!({})).await;
    let b = h.session().await;
    assert!(content(&b.call("state", json!({})).await).contains("restart the gateway"));
    h.workers(0).await;
    h.stop();
}

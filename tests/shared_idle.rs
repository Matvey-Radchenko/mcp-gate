#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};
use support::*;

async fn idle_gateway() -> Harness {
    idle_gateway_after(1).await
}

async fn idle_gateway_after(seconds: u64) -> Harness {
    let mut harness = Harness::generic("shared", 4, 15, 10).await;
    harness.stop();
    let mut config = mcp_gate::config::Config::load(&harness.config).unwrap();
    config.idle_seconds = Some(seconds);
    harness.restart_with_config(config).await;
    harness
}

#[tokio::test]
async fn idle_shutdown_frees_the_process_and_restarts_once_in_existing_sessions() {
    let mut harness = idle_gateway().await;
    let a = harness.session().await;
    let b = harness.session().await;
    let stream = a.req(reqwest::Method::GET).send().await.unwrap();
    assert!(stream.status().is_success());
    let first = mock_value(&a.call("state", json!({"value":"old"})).await);
    let old_pid = first["pid"].as_u64().unwrap() as u32;
    harness.workers(1).await;
    harness.workers(0).await;
    assert!(!alive(old_pid));
    assert_eq!(
        harness.health().await.unwrap()["runtime"]["shared_state"],
        "dormant"
    );
    assert!(
        harness.health().await.unwrap()["session_details"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["streams"] == 1)
    );
    a.request("tools/list", json!({})).await;
    harness.workers(0).await;
    let (left, right) = tokio::join!(
        a.call("state", json!({"value":"A"})),
        b.call("state", json!({"value":"B"}))
    );
    assert_eq!(mock_value(&left)["value"], "A");
    assert_eq!(mock_value(&right)["value"], "B");
    assert_eq!(mock_value(&left)["pid"], mock_value(&right)["pid"]);
    assert_ne!(mock_value(&left)["pid"], old_pid);
    drop(stream);
    harness.stop();
}

#[tokio::test]
async fn active_and_queued_calls_survive_idle_deadline() {
    let mut harness = idle_gateway().await;
    let a = harness.session().await;
    let b = harness.session().await;
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
    let queued = tokio::spawn(async move { b.call("state", json!({"value":"queued"})).await });
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert!(!running.is_finished());
    assert!(!queued.is_finished());
    harness.workers(1).await;
    assert_eq!(content(&running.await.unwrap()), "done");
    assert_eq!(mock_value(&queued.await.unwrap())["value"], "queued");
    harness.workers(0).await;
    harness.stop();
}

#[tokio::test]
async fn idle_shutdown_does_not_clear_manual_failure() {
    let mut harness = idle_gateway().await;
    let a = harness.session().await;
    a.call("crash", json!({})).await;
    harness.workers(0).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let value = a.call("state", json!({"value":"must-not-run"})).await;
    assert!(content(&value).contains("restart the gateway"));
    harness.workers(0).await;
    harness.stop();
}

#[tokio::test]
async fn external_crash_between_calls_is_not_hidden_by_idle_shutdown() {
    // With a 1 s idle limit, a slow runner can let idle shutdown stop the
    // backend before the external kill lands; taskkill then finds no process.
    // The kill must happen while the backend is still running, and an idle
    // check must still run after the crash, so wait past the limit below.
    let mut harness = idle_gateway_after(3).await;
    let a = harness.session().await;
    let pid = mock_value(&a.call("state", json!({})).await)["pid"]
        .as_u64()
        .unwrap();
    #[cfg(windows)]
    let killed = std::process::Command::new("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/F"])
        .output()
        .unwrap();
    #[cfg(unix)]
    let killed = std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .output()
        .unwrap();
    assert!(
        killed.status.success(),
        "External kill of backend {pid} failed: {}",
        String::from_utf8_lossy(&killed.stderr)
    );
    tokio::time::sleep(Duration::from_secs(4)).await;
    let value = a.call("state", json!({"value":"must-not-run"})).await;
    assert!(content(&value).contains("Backend exited"));
    harness.workers(0).await;
    let value = a.call("state", json!({})).await;
    assert!(content(&value).contains("restart the gateway"));
    harness.workers(0).await;
    harness.stop();
}

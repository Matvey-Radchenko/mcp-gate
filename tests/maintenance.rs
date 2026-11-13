#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Fixtures are shared across independent integration suites"
)]
mod support;
use reqwest::{Client, Method, StatusCode};
use serde_json::json;
async fn maintenance(h: &support::Harness, method: Method) -> StatusCode {
    Client::new()
        .request(method, format!("{}/admin/maintenance", h.base))
        .bearer_auth(support::TOKEN)
        .send()
        .await
        .unwrap()
        .status()
}
#[tokio::test]
async fn maintenance_excludes_sessions_and_new_initialization_atomically() {
    let mut h = support::Harness::generic("session", 4, 10, 5).await;
    assert_eq!(maintenance(&h, Method::POST).await, StatusCode::OK);
    assert_eq!(maintenance(&h, Method::POST).await, StatusCode::OK);
    let init=Client::new().post(format!("{}/mcp",h.base)).bearer_auth(support::TOKEN)
        .header("Accept","application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).send().await.unwrap();
    assert_eq!(init.status(), StatusCode::SERVICE_UNAVAILABLE);
    h.workers(0).await;
    assert_eq!(maintenance(&h, Method::DELETE).await, StatusCode::OK);
    let session = h.session().await;
    assert_eq!(maintenance(&h, Method::POST).await, StatusCode::CONFLICT);
    session.close().await;
    assert_eq!(maintenance(&h, Method::POST).await, StatusCode::OK);
    assert_eq!(
        Client::new()
            .post(format!("{}/admin/maintenance", h.base))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    h.stop();
}

#[tokio::test]
async fn normally_exiting_backend_does_not_leave_its_child_alive() {
    let mut h = support::Harness::generic("session", 4, 10, 5).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("child-pid");
    let mut backend = mcp_gate::config::Config::load(&h.config).unwrap().backend;
    backend.env.insert(
        "MOCK_CHILD_PID_FILE".into(),
        file.to_string_lossy().into_owned(),
    );
    h.replace_backend(backend).await;
    // Discovery itself also has to clean up the child created before initialize.
    let discovery_pid: u32 = std::fs::read_to_string(&file).unwrap().parse().unwrap();
    for _ in 0..100 {
        if !support::alive(discovery_pid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!support::alive(discovery_pid));
    let session = h.session().await;
    session.call("state", json!({"value":"owned-child"})).await;
    let pid: u32 = std::fs::read_to_string(file).unwrap().parse().unwrap();
    assert!(support::alive(pid));
    session.close().await;
    h.workers(0).await;
    #[cfg(windows)]
    assert!(
        !support::alive(pid),
        "Windows must not report an idle worker while its job has live descendants"
    );
    for _ in 0..100 {
        if !support::alive(pid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!support::alive(pid));
    h.stop();
}

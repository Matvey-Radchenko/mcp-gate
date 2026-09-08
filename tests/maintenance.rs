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

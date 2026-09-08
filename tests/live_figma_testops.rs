//! Explicitly authorized read-only live probes. Do not log API payloads.
#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Native Codex fixtures are shared with offline suites"
)]
mod support;
use mcp_gate::config::Config;
use serde_json::{Value, json};
use std::path::PathBuf;
use support::native_codex::NativeCodex;

fn successful_read(value: &Value) {
    assert_ne!(
        value.get("isError"),
        Some(&json!(true)),
        "Live MCP error; payload withheld"
    );
    assert!(value["content"].as_array().is_some_and(|v| !v.is_empty()));
}
async fn workers(config: &Config) -> Value {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{}/health", config.listen))
        .bearer_auth(config.token().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["workers"]
        .clone()
}

#[tokio::test]
#[ignore = "Requires LIVE_TESTOPS_CONFIG, TESTOPS_CASE_ID, CODEX_BINARY and explicit live-read authorization"]
async fn testops_real_case_two_native_clients() {
    let path = PathBuf::from(std::env::var("LIVE_TESTOPS_CONFIG").unwrap());
    let id = std::env::var("TESTOPS_CASE_ID").unwrap();
    let config = Config::load(&path).unwrap();
    let before = workers(&config).await;
    let (mut a, mut b) = tokio::join!(
        NativeCodex::for_config(&path),
        NativeCodex::for_config(&path)
    );
    assert_eq!(tokio::join!(a.discover(), b.discover()), (3, 3));
    assert_eq!(workers(&config).await, before);
    let (av, bv) = tokio::join!(
        a.call("get_testcase", json!({"id":id})),
        b.call("get_testcase_steps", json!({"id":id}))
    );
    successful_read(&av);
    successful_read(&bv);
    a.close().await;
    successful_read(&b.call("get_testcase_full", json!({"id":id})).await);
    b.close().await;
    assert_eq!(workers(&config).await, 1);
}

#[tokio::test]
#[ignore = "Requires LIVE_FIGMA_CONFIG, FIGMA_FILE_KEY, FIGMA_NODE_ID, CODEX_BINARY and explicit read/download authorization"]
async fn figma_real_frame_read_and_cached_image() {
    let path = PathBuf::from(std::env::var("LIVE_FIGMA_CONFIG").unwrap());
    let key = std::env::var("FIGMA_FILE_KEY").unwrap();
    let node = std::env::var("FIGMA_NODE_ID").unwrap();
    let config = Config::load(&path).unwrap();
    let before = workers(&config).await;
    let mut a = NativeCodex::for_config(&path).await;
    assert_eq!(a.discover().await, 2);
    assert_eq!(workers(&config).await, before);
    successful_read(
        &a.call("get_figma_data", json!({"fileKey":key,"nodeId":node}))
            .await,
    );
    let result=a.call("download_figma_images",json!({"fileKey":key,"localPath":"migration-smoke","pngScale":0.25,"nodes":[{"nodeId":node,"fileName":"frame.png"}]})).await;
    successful_read(&result);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Downloaded 1 images"),
        "Expected one exported image; payload withheld"
    );
    let directory = text.split('`').nth(1).expect("Download directory missing");
    let image = PathBuf::from(directory).join("frame.png");
    assert!(image.starts_with(&config.tool_policy.scoped_directories[0].root));
    let bytes = std::fs::read(&image).unwrap();
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    eprintln!(
        "Verified cached PNG: {} ({} bytes)",
        image.display(),
        bytes.len()
    );
    a.close().await;
    assert_eq!(workers(&config).await, 1);
}

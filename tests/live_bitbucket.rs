//! Opt-in live corporate API probe. Only discovery and three read-only calls.
#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Native Codex fixtures are shared with offline suites"
)]
mod support;
use mcp_gate::config::Config;
use serde_json::{Value, json};
use support::native_codex::NativeCodex;

async fn health(config: &Config) -> Value {
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
        .json()
        .await
        .unwrap()
}

fn successful_read(value: &Value) {
    // Never print API responses (including failures) into test logs.
    assert_ne!(
        value.get("isError"),
        Some(&json!(true)),
        "Live read returned an MCP tool error; payload withheld"
    );
    assert!(
        value["content"].as_array().is_some_and(|v| !v.is_empty()),
        "Empty live response"
    );
}

#[tokio::test]
#[ignore = "Needs explicit VPN/live-read authorization, LIVE_BITBUCKET_CONFIG and CODEX_BINARY; starts no model"]
async fn two_native_clients_read_live_api() {
    let path = std::path::PathBuf::from(
        std::env::var_os("LIVE_BITBUCKET_CONFIG").expect("Set pilot config"),
    );
    let config = Config::load(&path).unwrap();
    let before = health(&config).await;
    let (mut a, mut b) = tokio::join!(
        NativeCodex::for_config(&path),
        NativeCodex::for_config(&path)
    );
    let (ac, bc) = tokio::join!(a.discover(), b.discover());
    assert_eq!((ac, bc), (25, 25));
    assert_eq!(
        health(&config).await["workers"],
        before["workers"],
        "Discovery must not spawn a worker"
    );
    let (av, bv) = tokio::join!(
        a.call("list_projects", json!({"limit":1})),
        b.call("search_repositories", json!({"query":"terminal","limit":1}))
    );
    successful_read(&av);
    successful_read(&bv);
    assert_eq!(health(&config).await["workers"], 1);
    a.close().await;
    successful_read(&b.call("list_projects", json!({"limit":1})).await);
    b.close().await;
    assert_eq!(health(&config).await["workers"], 1);
    eprintln!(
        "Live Bitbucket: 25 tools, two native clients, read-only calls, one shared worker, independent close passed. No API payloads logged."
    );
}

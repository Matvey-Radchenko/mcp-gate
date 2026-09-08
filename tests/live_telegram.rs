//! Explicitly authorized Telegram reads; no message contents or credentials in logs.
#![cfg(feature = "test-backend")]
#[allow(dead_code, reason = "Shared native-client fixture")]
mod support;
use mcp_gate::config::{Config, Ownership};
use serde_json::{Value, json};
use std::path::PathBuf;
use support::native_codex::NativeCodex;

fn success(v: &Value) {
    assert_ne!(v["isError"], true, "Telegram read failed; payload withheld");
    assert!(v["content"].as_array().is_some_and(|c| !c.is_empty()));
}
fn reaction_data(v: &Value) -> Value {
    success(v);
    if let Some(data) = v.get("structuredContent") {
        return data.clone();
    }
    serde_json::from_str(v["content"][0]["text"].as_str().unwrap()).unwrap()
}
async fn workers(c: &Config) -> Value {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{}/health", c.listen))
        .bearer_auth(c.token().unwrap())
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
#[ignore = "Requires LIVE_TELEGRAM_CONFIG, TELEGRAM_SMOKE_CHAT_ID, TELEGRAM_SMOKE_MESSAGE_ID, CODEX_BINARY and live-read authorization"]
async fn two_native_clients_share_history_and_broadcast_counts() {
    let path = PathBuf::from(std::env::var("LIVE_TELEGRAM_CONFIG").unwrap());
    let chat = std::env::var("TELEGRAM_SMOKE_CHAT_ID").unwrap();
    let message: i64 = std::env::var("TELEGRAM_SMOKE_MESSAGE_ID")
        .unwrap()
        .parse()
        .unwrap();
    let c = Config::load(&path).unwrap();
    assert!(c.ownership == Ownership::Shared);
    let before = workers(&c).await;
    let (mut a, mut b) = tokio::join!(
        NativeCodex::for_config(&path),
        NativeCodex::for_config(&path)
    );
    assert_eq!(tokio::join!(a.discover(), b.discover()), (63, 63));
    assert_eq!(workers(&c).await, before);
    let args = json!({"account":"default","chat_id":chat,"message_id":message,"limit":100});
    let (history, reactions) = tokio::join!(
        a.call(
            "get_history",
            json!({"account":"default","chat_id":chat,"limit":1})
        ),
        b.call("get_message_reactions", args.clone())
    );
    success(&history);
    let data = reaction_data(&reactions);
    // The channel must have non-empty aggregate counts, not just a successful RPC.
    let counts = data["data"]["reactionCounts"]
        .as_array()
        .expect("Aggregate counts missing");
    assert!(
        !counts.is_empty(),
        "Expected reactions on the selected channel post"
    );
    assert!(
        counts
            .iter()
            .all(|v| v["totalCount"].as_u64().is_some_and(|n| n > 0))
    );
    assert_eq!(data["data"]["canGetAddedReactions"], false);
    assert_eq!(workers(&c).await, 1);
    a.close().await;
    success(&b.call("get_message_reactions", args).await);
    b.close().await;
    assert_eq!(
        workers(&c).await,
        1,
        "Shared owner survives client disconnect"
    );
    eprintln!(
        "Verified history and {} aggregate reaction kinds; channel payload withheld",
        counts.len()
    );
}

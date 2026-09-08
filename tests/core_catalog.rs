//! Resources and prompts are core MCP operations, not per-server adapters.
#![cfg(feature = "test-backend")]
#[allow(dead_code, reason = "Fixtures are shared with lifecycle suites")]
mod support;
use mcp_gate::config::Config;
use serde_json::{Value, json};
use support::{Harness, mock_value};

async fn core(ownership: &str, timeout: u64) -> Harness {
    let mut h = Harness::generic(ownership, 4, timeout, 5).await;
    let mut backend = Config::load(&h.config).unwrap().backend;
    backend.env.insert("MOCK_CORE_CATALOG".into(), "1".into());
    h.replace_backend(backend).await;
    h
}

#[tokio::test]
async fn lazy_full_catalog_and_shared_execution() {
    let mut h = core("shared", 10).await;
    let (a, b) = tokio::join!(h.session(), h.session());
    assert_eq!(
        a.request("resources/list", json!({})).await["result"]["resources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        a.request("resources/templates/list", json!({})).await["result"]["resourceTemplates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        b.request("prompts/list", json!({})).await["result"]["prompts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        a.request("resources/list", json!({"cursor":"invalid"}))
            .await
            .get("error")
            .is_some()
    );
    assert!(
        a.request("prompts/get", json!({"name":"absent"}))
            .await
            .get("error")
            .is_some()
    );
    h.workers(0).await;
    let (resource, prompt) = tokio::join!(
        a.request("resources/read", json!({"uri":"fixture://first"})),
        b.request(
            "prompts/get",
            json!({"name":"echo","arguments":{"text":"client-b"}})
        )
    );
    let resource: Value =
        serde_json::from_str(resource["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    let prompt: Value = serde_json::from_str(
        prompt["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(resource["pid"], prompt["pid"]);
    assert_eq!(prompt["arguments"]["text"], "client-b");
    assert_eq!(
        mock_value(&a.call("state", json!({})).await)["pid"],
        resource["pid"]
    );
    h.workers(1).await;
    assert_eq!(
        a.request("resources/read", json!({"uri":"fixture://error"}))
            .await["error"]["code"],
        -32602
    );
    a.close().await;
    assert!(
        b.request("prompts/get", json!({"name":"echo"}))
            .await
            .get("result")
            .is_some()
    );
    b.close().await;
    h.workers(1).await;
    h.stop();
}

#[tokio::test]
async fn resource_and_prompt_notifications_invalidate_pin() {
    for method in ["resources/read", "prompts/get"] {
        let mut h = core("shared", 10).await;
        let a = h.session().await;
        let params = if method == "resources/read" {
            json!({"uri":"fixture://change"})
        } else {
            json!({"name":"echo","arguments":{"change":"yes"}})
        };
        assert!(a.request(method, params).await.get("result").is_some());
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let next = a
            .request("resources/read", json!({"uri":"fixture://first"}))
            .await;
        assert!(
            next["error"]["message"]
                .as_str()
                .unwrap()
                .contains("catalog changed")
        );
        h.workers(0).await;
        h.stop();
    }
}

#[tokio::test]
async fn resource_timeout_poison_shared_and_session_reads_are_isolated() {
    let mut h = core("shared", 1).await;
    let a = h.session().await;
    assert!(
        a.request("resources/read", json!({"uri":"fixture://pause"}))
            .await
            .get("error")
            .is_some()
    );
    assert!(
        a.request("prompts/get", json!({"name":"echo"}))
            .await
            .get("error")
            .is_some()
    );
    h.workers(0).await;
    h.stop();
    let mut h = core("session", 10).await;
    let (a, b) = tokio::join!(h.session(), h.session());
    let read = |s: support::Session| async move {
        s.request("resources/read", json!({"uri":"fixture://first"}))
            .await
    };
    let (av, bv) = tokio::join!(read(a.clone()), read(b.clone()));
    assert_ne!(
        av["result"]["contents"][0]["text"],
        bv["result"]["contents"][0]["text"]
    );
    h.workers(2).await;
    a.close().await;
    h.workers(1).await;
    b.close().await;
    h.workers(0).await;
    h.stop();
}

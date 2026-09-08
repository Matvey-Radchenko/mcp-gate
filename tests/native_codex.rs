#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Integration fixtures are shared across separate suites"
)]
mod support;
use serde_json::{Value, json};
use support::{Harness, native_codex::NativeCodex};

fn payload(response: &Value) -> Value {
    assert_ne!(response["isError"], true, "Native tool failed: {response}");
    serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "Requires CODEX_BINARY; isolated Codex homes, ephemeral threads, no model/API calls"]
async fn native_codex_shared_and_session_calls() {
    for ownership in ["shared", "session"] {
        let mut h = Harness::generic(ownership, 4, 20, 10).await;
        if ownership == "shared" {
            h.ignore_shared_roots().await;
        }
        let (mut a, mut b) = tokio::join!(NativeCodex::start(&h), NativeCodex::start(&h));
        let (ac, bc) = tokio::join!(a.discover(), b.discover());
        assert_eq!((ac, bc), (4, 4));
        h.workers(0).await;
        let (av, bv) = tokio::join!(
            a.call("state", json!({"value":"A"})),
            b.call("state", json!({"value":"B"}))
        );
        let av = payload(&av);
        let bv = payload(&bv);
        assert_eq!(av["value"], "A");
        assert_eq!(bv["value"], "B");
        if ownership == "shared" {
            assert_eq!(av["pid"], bv["pid"]);
            h.workers(1).await;
        } else {
            assert_ne!(av["pid"], bv["pid"]);
            h.workers(2).await;
        }
        a.close().await;
        h.workers(1).await;
        let after = payload(&b.call("state", json!({})).await);
        assert_eq!(after["pid"], bv["pid"]);
        b.close().await;
        h.workers(if ownership == "shared" { 1 } else { 0 }).await;
        eprintln!(
            "Native Codex: {ownership}, discovery lazy, calls routed, worker count correct, independent close passed"
        );
        h.stop();
    }
}

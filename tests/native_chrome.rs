//! One real Codex app-server, two ephemeral threads, two isolated real browsers.
//! Optional STAGED_CHROME_CONFIG must be a dedicated empty pilot, not a live service.
#![cfg(feature = "test-backend")]
#[allow(dead_code, reason = "Integration fixtures are shared across suites")]
mod support;
use mcp_gate::config::{Config, Ownership};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use support::{
    Harness,
    native_codex::{NativeCodex, discovery_sessions},
    process_tree::{descendants, track},
};

struct Probe {
    config: Config,
    client: reqwest::Client,
}
impl Probe {
    async fn health(&self) -> Value {
        self.client
            .get(format!("http://{}/health", self.config.listen))
            .bearer_auth(self.config.token().unwrap())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn workers(&self, expected: u64) {
        let limit = self.config.disconnect_grace_seconds + 20;
        tokio::time::timeout(Duration::from_secs(limit), async {
            while self.health().await["workers"] != expected {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("Worker cleanup/startup deadline exceeded");
    }
    async fn delete(&self, session: &str) {
        self.client
            .delete(format!("http://{}/mcp", self.config.listen))
            .bearer_auth(self.config.token().unwrap())
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", "2025-11-25")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
}
fn text(result: &Value) -> &str {
    assert_ne!(
        result["isError"], true,
        "Local fixture tool failed: {result}"
    );
    result["content"][0]["text"].as_str().expect("Tool text")
}
async fn browser_isolation(c: &mut NativeCodex, a: &str, b: &str, url: &str) {
    for (thread, name) in [(a, "A"), (b, "B")] {
        text(
            &c.call_in(
                thread,
                "navigate_page",
                json!({"pageId":1,"type":"url","url":format!("{url}?task={name}")}),
            )
            .await,
        );
    }
    let written = c.call_in(a,"evaluate_script",json!({"pageId":1,"function":"() => { window.name='native-A'; localStorage.setItem('gateway','A'); document.cookie='gateway=A;path=/'; return 'stored-A'; }"})).await;
    assert!(text(&written).contains("stored-A"));
    let isolated = c.call_in(b,"evaluate_script",json!({"pageId":1,"function":"() => localStorage.getItem('gateway') === null && !document.cookie.includes('gateway=A') && window.name !== 'native-A'"})).await;
    assert!(text(&isolated).contains("true"));
    text(
        &c.call_in(
            b,
            "evaluate_script",
            json!({"pageId":1,"function":"() => { window.name='native-B'; return window.name; }"}),
        )
        .await,
    );
    let before = c.call_in(b, "list_pages", json!({})).await;
    text(
        &c.call_in(a, "new_page", json!({"url":format!("{url}?extra=A")}))
            .await,
    );
    text(&c.call_in(a, "select_page", json!({"pageId":2})).await);
    let after = c.call_in(b, "list_pages", json!({})).await;
    assert_eq!(
        text(&before),
        text(&after),
        "A must not alter B's tabs/selection"
    );
    assert!(!text(&after).contains("extra=A"));
}

#[tokio::test]
#[ignore = "Requires CODEX_BINARY, DEVTOOLS_NODE and Chrome; only isolated browsers/local pages"]
async fn two_threads_real_browsers_and_lifecycle() {
    let staged = std::env::var_os("STAGED_CHROME_CONFIG").map(PathBuf::from);
    let mut harness = if staged.is_none() {
        Some(Harness::new(true, 2).await)
    } else {
        None
    };
    let config_path = staged
        .as_deref()
        .unwrap_or_else(|| &harness.as_ref().unwrap().config);
    let config = Config::load(config_path).unwrap();
    assert!(config.ownership == Ownership::Session);
    let probe = Probe {
        config,
        client: reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap(),
    };
    let initial = probe.health().await;
    assert_eq!(
        initial["sessions"], 0,
        "Refusing a gateway with pre-existing clients"
    );
    assert_eq!(
        initial["workers"], 0,
        "Refusing a gateway with pre-existing browsers"
    );
    let gateway_pid = initial["pid"].as_u64().unwrap() as u32;
    assert!(descendants(gateway_pid).is_empty());
    let mut c = NativeCodex::for_config(config_path).await;
    let a = c.thread.clone();
    assert_eq!(c.discover().await, 29);
    let first_ids = discovery_sessions(config_path, 1).await;
    let cwd = tempfile::tempdir().unwrap();
    let second = c.request("thread/start",json!({"cwd":cwd.path(),"ephemeral":true,"approvalPolicy":"never","sandbox":"read-only"})).await;
    assert_eq!(second["thread"]["ephemeral"], true);
    let b = second["thread"]["id"].as_str().unwrap().to_owned();
    assert_eq!(c.discover_in(&b).await, 29);
    let both_ids = discovery_sessions(config_path, 2).await;
    assert!(
        first_ids.is_subset(&both_ids),
        "First thread session changed"
    );
    probe.workers(0).await;
    assert!(
        descendants(gateway_pid).is_empty(),
        "Discovery must not launch Node/Chrome"
    );
    text(&c.call_in(&a, "list_pages", json!({})).await);
    probe.workers(1).await;
    let a_pids = descendants(gateway_pid);
    let a_processes = track(&a_pids);
    text(&c.call_in(&b, "list_pages", json!({})).await);
    probe.workers(2).await;
    let all_pids = descendants(gateway_pid);
    assert!(a_pids.len() >= 3 && all_pids.len() > a_pids.len());
    let mut all_processes = track(&all_pids);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let pages = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::get(|| async {
                    axum::response::Html("<title>Native session test</title><h1>Local fixture</h1>")
                }),
            ),
        )
        .await
        .unwrap();
    });
    browser_isolation(&mut c, &a, &b, &url).await;
    // Include renderers created by navigation and the extra tab, not only startup PIDs.
    all_processes.extend(track(&descendants(gateway_pid)));
    tokio::time::sleep(Duration::from_secs(3)).await;
    let preserved = c.call_in(&a,"evaluate_script",json!({"pageId":1,"function":"() => window.name === 'native-A' && localStorage.getItem('gateway') === 'A' && document.cookie.includes('gateway=A')"})).await;
    assert!(
        text(&preserved).contains("true"),
        "An ordinary pause must preserve state"
    );
    let unsub = c.request("thread/unsubscribe", json!({"threadId":a})).await;
    assert_eq!(unsub["status"], "unsubscribed");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        probe.health().await["workers"],
        2,
        "Unsubscribe alone is not session termination"
    );
    eprintln!(
        "Native unsubscribe retains MCP session/browser while app-server remains alive (short observation, not a 30-minute expiry test)."
    );
    probe.delete(first_ids.first().unwrap()).await;
    probe.workers(1).await;
    assert!(
        a_processes.iter().all(|p| !p.running()),
        "A's original browser tree must be gone while B survives"
    );
    let surviving = c
        .call_in(
            &b,
            "evaluate_script",
            json!({"pageId":1,"function":"() => window.name === 'native-B'"}),
        )
        .await;
    assert!(text(&surviving).contains("true"));
    c.request("thread/unsubscribe", json!({"threadId":b})).await;
    c.close().await;
    probe.workers(0).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        while all_processes.iter().any(|p| p.running()) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("Owned backend/browser process survived cleanup");
    assert_eq!(probe.health().await["sessions"], 0);
    assert!(descendants(gateway_pid).is_empty());
    pages.abort();
    if let Some(h) = harness.as_mut() {
        h.stop();
    }
    eprintln!(
        "Passed: one native Codex / two threads, lazy discovery, two isolated browser trees, tabs/storage/cookies, pause preservation, explicit session close, client exit and full child cleanup."
    );
}

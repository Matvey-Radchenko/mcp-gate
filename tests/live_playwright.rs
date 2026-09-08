//! Authorized local-only browser acceptance against an empty staged gateway.
#![cfg(feature = "test-backend")]
#[allow(dead_code, reason = "Shared native-client fixture")]
mod support;
use mcp_gate::config::{Config, Ownership};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};
use support::native_codex::NativeCodex;

fn descendants(root: u32) -> BTreeSet<u32> {
    let output = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .unwrap();
    assert!(output.status.success());
    let rows: Vec<(u32, u32)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
        })
        .collect();
    let mut found = BTreeSet::from([root]);
    loop {
        let next: Vec<_> = rows
            .iter()
            .filter(|(pid, parent)| found.contains(parent) && !found.contains(pid))
            .map(|(pid, _)| *pid)
            .collect();
        if next.is_empty() {
            break;
        }
        found.extend(next);
    }
    found.remove(&root);
    found
}

async fn health(c: &Config) -> Value {
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
        .json()
        .await
        .unwrap()
}
fn ids(h: &Value) -> BTreeSet<String> {
    h["session_details"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect()
}
fn text(v: &Value) -> String {
    assert_ne!(v["isError"], true, "Local fixture failed: {v}");
    v["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
async fn delete(c: &Config, id: &str, expected: u64) {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .delete(format!("http://{}/mcp", c.listen))
        .bearer_auth(c.token().unwrap())
        .header("mcp-session-id", id)
        .header("mcp-protocol-version", "2025-11-25")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while health(c).await["workers"] != expected {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("Worker cleanup");
}
fn screenshots(c: &Config, name: &str) -> BTreeSet<PathBuf> {
    std::fs::read_dir(c.state_dir.join("worker-data"))
        .unwrap()
        .map(|e| {
            e.unwrap()
                .path()
                .join("PLAYWRIGHT_MCP_OUTPUT_DIR")
                .join(name)
        })
        .filter(|p| p.is_file())
        .collect()
}

#[tokio::test]
#[ignore = "Requires STAGED_PLAYWRIGHT_CONFIG, CODEX_BINARY, Chrome and explicit local-browser authorization"]
async fn native_clients_isolate_browsers_artifacts_and_cleanup() {
    let path = PathBuf::from(std::env::var("STAGED_PLAYWRIGHT_CONFIG").unwrap());
    let c = Config::load(&path).unwrap();
    assert!(c.ownership == Ownership::Session);
    assert_eq!(
        health(&c).await["sessions"],
        0,
        "Use an empty staged gateway"
    );
    assert_eq!(health(&c).await["workers"], 0);
    let gateway_pid = health(&c).await["pid"].as_u64().unwrap() as u32;
    assert!(descendants(gateway_pid).is_empty());
    let mut a = NativeCodex::for_config(&path).await;
    assert_eq!(a.discover().await, 24);
    let aid = ids(&health(&c).await).pop_first().unwrap();
    let mut b = NativeCodex::for_config(&path).await;
    assert_eq!(b.discover().await, 24);
    let bid = ids(&health(&c).await)
        .into_iter()
        .find(|id| *id != aid)
        .unwrap();
    assert_eq!(health(&c).await["workers"], 0, "Discovery must stay lazy");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let pages = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::get(|| async {
                    axum::response::Html("<title>Gateway fixture</title><h1>Local test</h1>")
                }),
            ),
        )
        .await
        .unwrap();
    });
    text(&a.call("browser_navigate", json!({"url":url})).await);
    assert_eq!(health(&c).await["workers"], 1);
    let a_pids = descendants(gateway_pid);
    assert!(a_pids.len() >= 3, "Expected Node and real browser children");
    text(&b.call("browser_navigate", json!({"url":url})).await);
    assert_eq!(health(&c).await["workers"], 2);
    text(&a.call("browser_evaluate", json!({"function":"() => { window.name='A'; localStorage.setItem('fixture','A'); document.cookie='fixture=A;path=/'; document.body.style.background='red'; return 'ok'; }"})).await);
    assert!(text(&b.call("browser_evaluate", json!({"function":"() => window.name !== 'A' && localStorage.getItem('fixture') === null && !document.cookie.includes('fixture=A')"})).await).contains("true"));
    let before = text(&b.call("browser_tabs", json!({"action":"list"})).await);
    text(
        &a.call(
            "browser_tabs",
            json!({"action":"new","url":format!("{url}?extra=A")}),
        )
        .await,
    );
    assert_eq!(
        before,
        text(&b.call("browser_tabs", json!({"action":"list"})).await)
    );
    text(
        &a.call("browser_tabs", json!({"action":"select","index":0}))
            .await,
    );
    assert!(text(&a.call("browser_evaluate", json!({"function":"() => window.name === 'A' && localStorage.getItem('fixture') === 'A'"})).await).contains("true"));
    let name = format!("fixture-{}.png", uuid::Uuid::new_v4());
    text(
        &a.call(
            "browser_take_screenshot",
            json!({"filename":name,"scale":"css"}),
        )
        .await,
    );
    text(
        &b.call(
            "browser_take_screenshot",
            json!({"filename":name,"scale":"css"}),
        )
        .await,
    );
    let images = screenshots(&c, &name);
    assert_eq!(
        images.len(),
        2,
        "Same filename must land in separate worker directories"
    );
    let bytes: Vec<_> = images.iter().map(|p| std::fs::read(p).unwrap()).collect();
    assert!(bytes.iter().all(|b| b.starts_with(b"\x89PNG\r\n\x1a\n")));
    assert_ne!(
        bytes[0], bytes[1],
        "Independent pages must produce different images"
    );
    let all_pids = descendants(gateway_pid);
    delete(&c, &aid, 1).await;
    assert!(
        a_pids.iter().all(|p| !support::alive(*p)),
        "A's browser process tree must be reaped"
    );
    assert!(
        text(
            &b.call(
                "browser_evaluate",
                json!({"function":"() => localStorage.getItem('fixture') === null"})
            )
            .await
        )
        .contains("true")
    );
    delete(&c, &bid, 0).await;
    assert!(
        all_pids.iter().all(|p| !support::alive(*p)),
        "Browser cleanup must not leave orphans"
    );
    assert!(descendants(gateway_pid).is_empty());
    assert!(
        images.iter().all(|p| p.exists()),
        "Artifacts must survive backend cleanup"
    );
    a.close().await;
    b.close().await;
    pages.abort();
}

//! Authorized local-only browser acceptance against an empty staged gateway.
#![cfg(feature = "test-backend")]
#[allow(dead_code, reason = "Shared native-client fixture")]
mod support;
use mcp_gate::config::{Config, Ownership};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};
use support::{
    native_codex::{NativeCodex, discovery_sessions},
    process_tree::{descendants, track},
};

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
#[ignore = "Requires PLAYWRIGHT_ENTRYPOINT/DEVTOOLS_NODE or an empty STAGED_PLAYWRIGHT_CONFIG, CODEX_BINARY and installed Chrome"]
async fn native_clients_isolate_browsers_artifacts_and_cleanup() {
    let mut owned = None;
    let path = if let Ok(path) = std::env::var("STAGED_PLAYWRIGHT_CONFIG") {
        PathBuf::from(path)
    } else {
        let mut h = support::Harness::generic("session", 4, 60, 30).await;
        let mut backend = Config::load(&h.config).unwrap().backend;
        backend.command = std::env::var("DEVTOOLS_NODE").unwrap().into();
        backend.entrypoint = Some(std::env::var("PLAYWRIGHT_ENTRYPOINT").unwrap().into());
        backend.version = "1.63.0-alpha-2026-08-31".into();
        backend.args = vec![
            "--isolated".into(),
            "--browser".into(),
            "chrome".into(),
            "--headless".into(),
        ];
        if cfg!(windows) {
            // Native hosted Windows runners can exceed Playwright's 5-second
            // screenshot default. This explicit fixture option stays below the
            // gateway call deadline; setup never changes a user's timeout.
            backend
                .args
                .extend(["--timeout-action".into(), "30000".into()]);
        }
        backend.working_directory = Some(h.config.parent().unwrap().to_path_buf());
        backend.directory_env = vec!["PLAYWRIGHT_MCP_OUTPUT_DIR".into()];
        backend.working_directory_env = Some("PLAYWRIGHT_MCP_OUTPUT_DIR".into());
        h.replace_backend(backend).await;
        let path = h.config.clone();
        owned = Some(h);
        path
    };
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
    let aid = discovery_sessions(&path, 1).await.pop_first().unwrap();
    let mut b = NativeCodex::for_config(&path).await;
    assert_eq!(b.discover().await, 24);
    let both_ids = discovery_sessions(&path, 2).await;
    assert!(both_ids.contains(&aid), "First client session changed");
    let bid = both_ids.into_iter().find(|id| *id != aid).unwrap();
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
    let a_processes = track(&a_pids);
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
    let all_processes = track(&all_pids);
    delete(&c, &aid, 1).await;
    assert!(
        a_processes.iter().all(|p| !p.running()),
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
        all_processes.iter().all(|p| !p.running()),
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
    if let Some(h) = &mut owned {
        h.stop();
    }
}

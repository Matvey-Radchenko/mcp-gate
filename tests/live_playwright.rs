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
/// The tab list alone. Playwright appends asynchronous page events, such as
/// a new console entry, to whichever response happens to follow them; they
/// are not part of the tab state being compared.
fn tab_list(v: &Value) -> String {
    let text = text(v);
    let result = text
        .split_once("### Result")
        .unwrap_or_else(|| panic!("Tab list without a result: {text}"))
        .1;
    result
        .split_once("\n### ")
        .map_or(result, |(list, _)| list)
        .trim()
        .to_owned()
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
fn still_running(processes: &[support::process_tree::TrackedProcess]) -> Vec<u32> {
    processes
        .iter()
        .filter(|process| process.running())
        .map(|process| process.pid())
        .collect()
}
async fn quiet(processes: &[support::process_tree::TrackedProcess]) -> bool {
    // The gateway worker can exit before Windows reaps Chrome. Exit code 259
    // still means running. native_chrome uses this same bound; five seconds
    // was still inside that exit after the worker had already stopped.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while processes.iter().any(|process| process.running()) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    true
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

async fn playwright_harness() -> support::Harness {
    let mut h = support::Harness::generic("session", 4, 60, 30).await;
    let mut backend = Config::load(&h.config).unwrap().backend;
    backend.command = std::env::var("DEVTOOLS_NODE").unwrap().into();
    backend.entrypoint = Some(std::env::var("PLAYWRIGHT_ENTRYPOINT").unwrap().into());
    backend.version = "1.63.0-alpha-2026-08-31".into();
    backend.args = vec!["--isolated".into(), "--headless".into()];
    if let Some(browser) = support::browser_executable() {
        backend.args.extend([
            "--browser".into(),
            "chromium".into(),
            "--executable-path".into(),
            browser,
        ]);
    } else {
        backend.args.extend(["--browser".into(), "chrome".into()]);
    }
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
    h
}

#[tokio::test]
#[ignore = "Requires CODEX_BINARY and an empty STAGED_PLAYWRIGHT_CONFIG or a local Playwright fixture with Chrome or MCP_GATE_TEST_BROWSER"]
async fn native_clients_isolate_browsers_artifacts_and_cleanup() {
    let staged = std::env::var_os("STAGED_PLAYWRIGHT_CONFIG").map(PathBuf::from);
    let mut owned = if staged.is_none() {
        Some(playwright_harness().await)
    } else {
        None
    };
    let path = staged
        .as_deref()
        .unwrap_or_else(|| &owned.as_ref().unwrap().config);
    let c = Config::load(path).unwrap();
    assert!(c.ownership == Ownership::Session);
    assert_eq!(
        health(&c).await["sessions"],
        0,
        "Use an empty staged gateway"
    );
    assert_eq!(health(&c).await["workers"], 0);
    let gateway_pid = health(&c).await["pid"].as_u64().unwrap() as u32;
    assert!(descendants(gateway_pid).is_empty());
    let mut a = NativeCodex::for_config(path).await;
    assert_eq!(a.discover().await, 24);
    let aid = discovery_sessions(path, 1).await.pop_first().unwrap();
    let mut b = NativeCodex::for_config(path).await;
    assert_eq!(b.discover().await, 24);
    let both_ids = discovery_sessions(path, 2).await;
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
    // Capture while each page is still its browser's only, foreground tab.
    // Headless Chrome on Windows can stop producing frames for a page that
    // was backgrounded by a new tab, even after it is selected again, so a
    // screenshot then waits for a frame until its action timeout.
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
    let before = tab_list(&b.call("browser_tabs", json!({"action":"list"})).await);
    text(
        &a.call(
            "browser_tabs",
            json!({"action":"new","url":format!("{url}?extra=A")}),
        )
        .await,
    );
    assert_eq!(
        before,
        tab_list(&b.call("browser_tabs", json!({"action":"list"})).await)
    );
    text(
        &a.call("browser_tabs", json!({"action":"select","index":0}))
            .await,
    );
    assert!(text(&a.call("browser_evaluate", json!({"function":"() => window.name === 'A' && localStorage.getItem('fixture') === 'A'"})).await).contains("true"));
    let all_pids = descendants(gateway_pid);
    let all_processes = track(&all_pids);
    delete(&c, &aid, 1).await;
    assert!(
        quiet(&a_processes).await,
        "A's browser process tree must be reaped; still running: {:?}",
        still_running(&a_processes)
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
        quiet(&all_processes).await,
        "Browser cleanup must not leave orphans; still running: {:?}",
        still_running(&all_processes)
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

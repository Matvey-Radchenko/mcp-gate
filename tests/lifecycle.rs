#![cfg(feature = "test-backend")]
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::Ordering,
    time::Duration,
};
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use support::*;

#[tokio::test]
#[ignore = "Requires CODEX_BINARY; uses temporary CLI overrides, creates no Codex tasks"]
async fn codex_native_discovery_and_helper() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut h = Harness::new(false, 1).await;
    let binary =
        std::env::var("CODEX_BINARY").expect("Set CODEX_BINARY to the installed Codex executable");
    let mut args = Vec::new();
    let config_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".codex"));
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(config_home.join("config.toml")).unwrap()).unwrap();
    if let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) {
        for name in servers.keys() {
            args.extend(["-c".to_owned(), format!("mcp_servers.{name}.enabled=false")]);
        }
    }
    if let Some(plugins) = config.get("plugins").and_then(toml::Value::as_table) {
        for name in plugins.keys() {
            args.extend(["-c".to_owned(), format!("plugins.{}.enabled=false", name)]);
        }
    }
    let installed_config = std::env::var_os("GATEWAY_CONFIG").map(PathBuf::from);
    let probe_config = installed_config.as_ref().unwrap_or(&h.config);
    let probe = mcp_gate::config::Config::load(probe_config).unwrap();
    let expected_tools = mcp_gate::catalog::Catalog::load(&probe.catalog_file, &probe.backend)
        .unwrap()
        .tools
        .len();
    let helper_binary = installed_config
        .as_ref()
        .map(|p| p.parent().unwrap().join("bin/mcp-gate"))
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_mcp-gate")));
    let helper = format!(
        "'{}' headers --config '{}'",
        helper_binary.display(),
        probe_config.display()
    );
    args.extend([
        "-c".into(),
        format!(
            "mcp_servers.gateway-probe={{url={},http_headers_helper={},startup_timeout_sec=20}}",
            serde_json::to_string(&format!("http://{}/mcp", probe.listen)).unwrap(),
            serde_json::to_string(&helper).unwrap()
        ),
    ]);
    let checked = Command::new(&binary)
        .args(&args)
        .args(["mcp", "list", "--json"])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "Probe configuration rejected: {}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let checked: Value = serde_json::from_slice(&checked.stdout).unwrap();
    let enabled: Vec<_> = checked
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["enabled"] == true)
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        enabled,
        vec!["gateway-probe"],
        "Refusing to launch unrelated MCP servers"
    );
    let mut codex = tokio::process::Command::new(&binary)
        .args(&args)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = codex.stdin.take().unwrap();
    let mut lines = BufReader::new(codex.stdout.take().unwrap()).lines();
    input.write_all(format!("{}\n",json!({"id":0,"method":"initialize","params":{"clientInfo":{"name":"gateway-test","version":"1"},"capabilities":{"experimentalApi":true}}})).as_bytes()).await.unwrap();
    let result=tokio::time::timeout(Duration::from_secs(60),async {
        while let Some(line)=lines.next_line().await.unwrap() {
            let value:Value=serde_json::from_str(&line).unwrap();
            if value["id"]==0 {
                input.write_all(format!("{}\n{}\n",json!({"method":"initialized"}),json!({"id":1,"method":"mcpServerStatus/list","params":{"detail":"toolsAndAuthOnly"}})).as_bytes()).await.unwrap();
            }
            if value["id"]==1 { return value; }
        }
        panic!("Codex exited before MCP status")
    }).await.unwrap();
    // Expose only the relevant diagnostics; never log the general inventory.
    let data = result["result"]["data"]
        .as_array()
        .unwrap_or_else(|| panic!("Status RPC failed: {result}"));
    let probe = data.iter().find(|v| v["name"] == "gateway-probe").unwrap();
    assert_eq!(
        probe["tools"]
            .as_object()
            .map(|t| t.len())
            .or_else(|| probe["tools"].as_array().map(|t| t.len())),
        Some(expected_tools),
        "Probe status: {probe}"
    );
    h.workers(0).await;
    drop(input);
    if tokio::time::timeout(Duration::from_secs(5), codex.wait())
        .await
        .is_err()
    {
        codex.kill().await.unwrap();
    }
    h.stop();
}

#[tokio::test]
async fn auth_lazy_isolation_capacity_and_delete() {
    let mut h = Harness::new(false, 2).await;
    let client = Client::new();
    assert_eq!(
        client
            .get(format!("{}/health", h.base))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for (key, value) in [("Origin", "http://evil.test"), ("Host", "evil.test")] {
        assert_eq!(
            client
                .get(format!("{}/health", h.base))
                .bearer_auth(TOKEN)
                .header(key, value)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let (a, b, c) = tokio::join!(h.session(), h.session(), h.session());
    assert_ne!(a.id, b.id);
    for session in [&a, &b, &c] {
        assert_eq!(
            session.request("tools/list", json!({})).await["result"]["tools"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }
    h.workers(0).await;
    let (av, bv) = tokio::join!(
        a.call("state", json!({"value":"A"})),
        b.call("state", json!({"value":"B"}))
    );
    let apid = mock_value(&av)["pid"].as_u64().unwrap() as u32;
    let bpid = mock_value(&bv)["pid"].as_u64().unwrap() as u32;
    assert_ne!(apid, bpid);
    assert_eq!(mock_value(&a.call("state", json!({})).await)["value"], "A");
    assert!(content(&c.call("state", json!({})).await).contains("slots"));
    a.close().await;
    h.workers(1).await;
    assert!(!alive(apid));
    assert_eq!(mock_value(&b.call("state", json!({})).await)["value"], "B");
    assert!(c.call("state", json!({})).await["result"]["isError"] != true);
    b.close().await;
    c.close().await;
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
async fn disconnect_grace_reconnect_and_shutdown() {
    let mut h = Harness::new(false, 2).await;
    let a = h.session().await;
    let pid = mock_value(&a.call("state", json!({"value":"preserved"})).await)["pid"]
        .as_u64()
        .unwrap() as u32;
    let stream = a.req(reqwest::Method::GET).send().await.unwrap();
    drop(stream);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let stream = a.req(reqwest::Method::GET).send().await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(alive(pid));
    assert_eq!(
        mock_value(&a.call("state", json!({})).await)["value"],
        "preserved"
    );
    drop(stream);
    h.workers(0).await;
    assert!(!alive(pid));
    assert_eq!(
        a.req(reqwest::Method::GET).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let b = h.session().await;
    let pid = mock_value(&b.call("state", json!({})).await)["pid"]
        .as_u64()
        .unwrap() as u32;
    h.stop();
    assert!(!alive(pid));
}

#[tokio::test]
async fn roots_cancellation_crash_and_no_replay() {
    let mut h = Harness::new(false, 2).await;
    let a = h.session().await;
    assert_eq!(
        mock_value(&a.call("roots", json!({})).await)["roots"][0]["uri"],
        "file:///tmp/gateway-test"
    );
    let pending = a.clone();
    let call_id = a.next.load(Ordering::SeqCst);
    let task = tokio::spawn(async move {
        pending
            .request(
                "tools/call",
                json!({"name":"pause", "arguments":{}, "_meta":{"progressToken":"test-progress"}}),
            )
            .await
    });
    for _ in 0..100 {
        if a.progress.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(a.progress.load(Ordering::SeqCst), 1);
    a.post(json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":call_id, "reason":"test"}})).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap();
    for _ in 0..100 {
        if h.cancel_file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(h.cancel_file.exists(), "cancellation must reach backend");
    a.call("crash", json!({})).await;
    assert!(content(&a.call("state", json!({})).await).contains("Reconnect"));
    assert!(content(&a.call("state", json!({})).await).contains("NOT retried"));
    a.close().await;
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
async fn rejects_duplicate_daemon_and_catalog_drift() {
    let mut h = Harness::new(false, 1).await;
    let output = Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
        .args(["serve", "--config"])
        .arg(&h.config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Another gateway"));
    let a = h.session().await;
    assert_eq!(
        a.req(reqwest::Method::POST)
            .header("mcp-protocol-version", "2026-07-28")
            .json(&json!({"jsonrpc":"2.0","id":42,"method":"ping"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    a.close().await;
    h.stop();
    let mut config = mcp_gate::config::Config::load(&h.config).unwrap();
    config.backend.args.retain(|a| a != "--headless");
    std::fs::write(&h.config, toml::to_string(&config).unwrap()).unwrap();
    let check = Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
        .args(["check", "--config"])
        .arg(&h.config)
        .output()
        .unwrap();
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("Backend changed"));
}

#[tokio::test]
async fn dropped_http_response_does_not_destroy_an_active_call() {
    let mut h = Harness::new(false, 1).await;
    let a = h.session().await;
    a.call("state", json!({})).await;
    let get = a.req(reqwest::Method::GET).send().await.unwrap();
    let response = a.post(json!({"jsonrpc":"2.0", "id":900, "method":"tools/call", "params":{"name":"pause","arguments":{}}})).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(response);
    drop(get);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        h.health().await.unwrap()["workers"],
        1,
        "An active MCP handler must outlive a dropped HTTP body"
    );
    a.close().await;
    h.workers(0).await;
    h.stop();
}

fn descendants(root: u32) -> Vec<u32> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .unwrap();
    let rows: Vec<(u32, u32)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect();
    let mut result = vec![root];
    loop {
        let new: Vec<_> = rows
            .iter()
            .filter(|(pid, ppid)| result.contains(ppid) && !result.contains(pid))
            .map(|(pid, _)| *pid)
            .collect();
        if new.is_empty() {
            break;
        }
        result.extend(new);
    }
    result.remove(0);
    result
}

#[tokio::test]
#[ignore = "Requires installed Chrome and DEVTOOLS_NODE; operates only on local test pages"]
async fn real_chrome_isolation_and_process_cleanup() {
    let mut h = Harness::new(true, 2).await;
    let (a, b) = tokio::join!(h.session(), h.session());
    let tools = a.request("tools/list", json!({})).await;
    assert!(tools["result"]["tools"].as_array().unwrap().len() > 20);
    h.workers(0).await;
    let (av, bv) = tokio::join!(
        a.call("list_pages", json!({})),
        b.call("list_pages", json!({}))
    );
    assert!(av["result"]["isError"] != true, "{av}");
    assert!(bv["result"]["isError"] != true, "{bv}");
    h.workers(2).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let pages = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::get(|| async {
                    axum::response::Html("<title>Gateway isolation test</title><h1>Local test</h1>")
                }),
            ),
        )
        .await
        .unwrap();
    });
    for session in [&a, &b] {
        let result = session
            .call("navigate_page", json!({"pageId":1,"type":"url","url":url}))
            .await;
        assert!(result["result"]["isError"] != true, "{result}");
    }
    let av=a.call("evaluate_script",json!({"pageId":1,"function":"() => { globalThis.gatewayMarker='A'; localStorage.setItem('gateway','A'); document.cookie='gateway=A;path=/'; return gatewayMarker }"})).await;
    assert!(content(&av).contains('A'), "{av}");
    let bv = b
        .call(
            "evaluate_script",
            json!({"pageId":1,"function":"() => ({marker:typeof globalThis.gatewayMarker,storage:localStorage.getItem('gateway'),cookies:document.cookie})"}),
        )
        .await;
    assert!(content(&bv).contains("undefined"), "{bv}");
    assert!(content(&bv).contains("null"), "{bv}");
    assert!(!content(&bv).contains("gateway=A"), "{bv}");
    let (ta, tb) = tokio::join!(
        a.call(
            "performance_start_trace",
            json!({"pageId":1,"reload":false,"autoStop":false})
        ),
        b.call(
            "performance_start_trace",
            json!({"pageId":1,"reload":false,"autoStop":false})
        )
    );
    for value in [&ta, &tb] {
        assert!(content(value).contains("being recorded"), "{value}");
    }
    let stopped = a.call("performance_stop_trace", json!({"pageId":1})).await;
    assert!(stopped["result"]["isError"] != true, "{stopped}");
    let still_running = b
        .call(
            "performance_start_trace",
            json!({"pageId":1,"reload":false,"autoStop":false}),
        )
        .await;
    assert!(
        content(&still_running).contains("already running"),
        "{still_running}"
    );
    let stopped = b.call("performance_stop_trace", json!({"pageId":1})).await;
    assert!(stopped["result"]["isError"] != true, "{stopped}");
    let pids = descendants(h.child.id());
    assert!(
        pids.len() >= 4,
        "Expected own workers and Chrome descendants: {pids:?}"
    );
    a.close().await;
    h.workers(1).await;
    assert!(b.call("list_pages", json!({})).await["result"]["isError"] != true);
    b.close().await;
    h.workers(0).await;
    for _ in 0..100 {
        if pids.iter().all(|p| !alive(*p)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        pids.iter().all(|p| !alive(*p)),
        "No test backend/Chrome child may survive DELETE"
    );
    pages.abort();
    h.stop();
}

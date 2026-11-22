#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Fixtures are shared across independent integration suites"
)]
mod support;

use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use mcp_gate::{manage::store, platform};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use support::{Harness, TOKEN};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    net::TcpListener,
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::mpsc,
    time::{sleep, timeout},
};

const WAIT: Duration = Duration::from_secs(20);

fn command(name: &str, root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-gate"));
    command
        .args(["connect", name])
        .env("MCP_GATE_TEST_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

struct Proxy {
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
}

impl Proxy {
    fn spawn(mut command: Command) -> Self {
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            input,
            output,
        }
    }

    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        timeout(
            WAIT,
            self.input.as_mut().unwrap().write_all(line.as_bytes()),
        )
        .await
        .unwrap()
        .unwrap();
    }

    async fn read(&mut self) -> Value {
        let line = timeout(WAIT, self.output.next_line())
            .await
            .expect("Timed out reading bridge stdout")
            .unwrap()
            .expect("Bridge stdout closed");
        assert!(
            !line.contains(TOKEN),
            "Bridge stdout leaked the fixture token"
        );
        serde_json::from_str(&line).expect("Bridge stdout must contain only JSON-RPC lines")
    }

    async fn wait(mut self) -> String {
        // Keep any remaining stdin handle until the process exits for the signal test.
        let output = timeout(WAIT, self.child.wait_with_output())
            .await
            .expect("Bridge failed to exit")
            .unwrap();
        assert!(
            output.status.success(),
            "Bridge failed with {}",
            output.status
        );
        assert!(
            timeout(WAIT, self.output.next_line())
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            !stderr.contains(TOKEN),
            "Bridge stderr leaked the fixture token"
        );
        stderr
    }

    async fn close(mut self) -> String {
        drop(self.input.take());
        self.wait().await
    }
}

fn initialize(roots: bool) -> Value {
    json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
        "protocolVersion":"2025-06-18",
        "capabilities": if roots {json!({"roots":{"listChanged":true}})} else {json!({})},
        "clientInfo":{"name":"bridge-test", "version":"1"}
    }})
}

fn initialized() -> Value {
    json!({"jsonrpc":"2.0", "method":"notifications/initialized"})
}

async fn open_session(harness: &Harness) -> Value {
    timeout(WAIT, async {
        loop {
            let health = harness.health().await.unwrap();
            if health["session_details"][0]["streams"] == 1 {
                return health["session_details"][0]["id"].clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Bridge did not open a GET stream")
}

#[tokio::test]
async fn stdio_proxies_gateway_calls_concurrently_and_closes_the_session() {
    let mut harness = Harness::generic("session", 4, 30, 10).await;
    let dir = harness.config.parent().unwrap();
    let mut cmd = command("fixture", &dir.join("unused-registry"));
    cmd.args(["--config", "config.toml"]).current_dir(dir);
    let mut proxy = Proxy::spawn(cmd);

    // Pipeline the handshake and first request to check both ordering barriers.
    proxy.send(initialize(true)).await;
    proxy.send(initialized()).await;
    proxy
        .send(json!({"jsonrpc":"2.0", "method":"tools/list", "id":2}))
        .await;
    let init = proxy.read().await;
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        proxy.read().await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let session_id = open_session(&harness).await;

    proxy
        .send(
            json!({"jsonrpc":"2.0", "id":"state", "method":"tools/call", "params":{
                "name":"state", "arguments":{"value":"through stdio"}
            }}),
        )
        .await;
    let roots = proxy.read().await;
    assert_eq!(roots["method"], "roots/list");
    // The first tool call cannot finish until we reply to roots/list. Discovery
    // must already be able to complete on its own HTTP request.
    proxy
        .send(json!({"jsonrpc":"2.0", "id":3, "method":"tools/list"}))
        .await;
    assert_eq!(proxy.read().await["id"], 3);
    proxy
        .send(json!({"jsonrpc":"2.0", "id":roots["id"], "result":{"roots":[]}}))
        .await;
    let result = proxy.read().await;
    assert_eq!(result["id"], "state");
    assert_eq!(support::mock_value(&result)["value"], "through stdio");

    for id in [4, 5] {
        proxy
            .send(json!({"jsonrpc":"2.0", "id":id, "method":"tools/list"}))
            .await;
    }
    let mut ids = [
        proxy.read().await["id"].as_u64().unwrap(),
        proxy.read().await["id"].as_u64().unwrap(),
    ];
    ids.sort_unstable();
    assert_eq!(ids, [4, 5]);

    assert!(proxy.close().await.is_empty());
    assert_eq!(harness.health().await.unwrap()["sessions"], 0);
    let status = reqwest::Client::builder()
        .no_proxy()
        .timeout(WAIT)
        .build()
        .unwrap()
        .post(format!("{}/mcp", harness.base))
        .bearer_auth(TOKEN)
        .header("mcp-session-id", session_id.as_str().unwrap())
        .header(header::ACCEPT, "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0", "id":6, "method":"tools/list"}))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::NOT_FOUND);
    harness.workers(0).await;
    harness.stop();
}

fn config(root: &Path, listen: SocketAddr) -> PathBuf {
    let token = root.join("token");
    std::fs::write(&token, TOKEN).unwrap();
    platform::private_permissions(&token).unwrap();
    let config = root.join("config.toml");
    let settings = json!({"format_version":2, "listen":listen, "token_file":token,
        "catalog_file":root.join("catalog.json"), "state_dir":root.join("state"),
        "backend":{"profile":"stdio", "command":env!("CARGO_BIN_EXE_mock-backend"), "version":"fixture"}});
    std::fs::write(&config, toml::to_string(&settings).unwrap()).unwrap();
    config
}

fn register(root: &Path) {
    let registry = store::Registry {
        format_version: 1,
        gateways: vec![store::Record {
            removing: false,
            id: "fixture-id".into(),
            release: root.to_path_buf(),
            label: "local.mcp-gate.fixture".into(),
            binary_hash: String::new(),
            binary: PathBuf::from(env!("CARGO_BIN_EXE_mcp-gate")),
            bindings: vec![],
        }],
    };
    store::save(root, &registry).unwrap();
}

async fn http_fixture(
    State(events): State<mpsc::UnboundedSender<Method>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    assert!(headers[header::AUTHORIZATION] == format!("Bearer {TOKEN}"));
    let message: Value = if parts.method == Method::POST {
        assert_eq!(
            headers[header::ACCEPT],
            "application/json, text/event-stream"
        );
        assert_eq!(headers[header::CONTENT_TYPE], "application/json");
        serde_json::from_slice(&to_bytes(body, 4096).await.unwrap()).unwrap()
    } else {
        Value::Null
    };
    if message["method"] == "initialize" {
        assert!(!headers.contains_key("mcp-session-id"));
        assert!(!headers.contains_key("mcp-protocol-version"));
        return (
            [("mcp-session-id", "fixture-session")],
            axum::Json(json!({
                "jsonrpc":"2.0", "id":message["id"], "result":{"protocolVersion":"2025-03-26"}
            })),
        )
            .into_response();
    }
    assert_eq!(headers["mcp-session-id"], "fixture-session");
    assert_eq!(headers["mcp-protocol-version"], "2025-03-26");
    events.send(parts.method.clone()).unwrap();
    match parts.method {
        Method::GET => {
            assert_eq!(headers[header::ACCEPT], "text/event-stream");
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                "data: {\"jsonrpc\":\"2.0\",\"id\":\"server\",\"method\":\"ping\"}\n\n",
            )
                .into_response()
        }
        Method::DELETE => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        _ if message["method"] == "notifications/initialized" || message["id"] == "server" => {
            if message["id"] == "server" {
                assert_eq!(message["result"], json!({}));
            }
            StatusCode::ACCEPTED.into_response()
        }
        _ => (StatusCode::NOT_FOUND, TOKEN).into_response(),
    }
}

#[tokio::test]
async fn managed_bridge_negotiates_json_and_relays_get_messages_and_http_errors() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    config(dir.path(), listener.local_addr().unwrap());
    register(dir.path());
    let (send, mut received) = mpsc::unbounded_channel();
    let router = Router::new()
        .route("/mcp", any(http_fixture))
        .with_state(send);
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut proxy = Proxy::spawn(command("fixture", dir.path()));

    proxy.send(initialize(false)).await;
    assert_eq!(
        proxy.read().await["result"]["protocolVersion"],
        "2025-03-26"
    );
    proxy.send(initialized()).await;
    assert_eq!(proxy.read().await["id"], "server");
    proxy
        .send(json!({"jsonrpc":"2.0", "id":"server", "result":{}}))
        .await;
    proxy
        .send(json!({"jsonrpc":"2.0", "id":"expires", "method":"tools/call"}))
        .await;
    let response = proxy.read().await;
    assert_eq!(response["id"], "expires");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("session expired")
    );

    // Wait for the independent client-response POST before initiating shutdown.
    for method in [Method::POST, Method::GET, Method::POST, Method::POST] {
        assert_eq!(
            timeout(WAIT, received.recv()).await.unwrap().unwrap(),
            method
        );
    }
    #[cfg(unix)]
    let stderr = {
        // SAFETY: the child is still running and owned by this fixture.
        let sent = unsafe { libc::kill(proxy.child.id().unwrap() as i32, libc::SIGTERM) };
        assert_eq!(sent, 0);
        proxy.wait().await
    };
    #[cfg(windows)]
    let stderr = proxy.close().await;
    assert!(stderr.contains("session expired"));
    assert!(!stderr.contains("Cannot close gateway session"));
    assert_eq!(
        timeout(WAIT, received.recv()).await.unwrap().unwrap(),
        Method::DELETE
    );
    assert!(
        received.try_recv().is_err(),
        "Bridge retried an HTTP operation"
    );
    server.abort();
}

#[tokio::test]
async fn reports_unknown_gateway_without_writing_to_stdout() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "127.0.0.1:1234".parse().unwrap());
    register(dir.path());
    let output = timeout(WAIT, command("missing", dir.path()).output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("No managed gateway matches missing"));
    assert!(stderr.contains("Known gateways: fixture"));
    assert!(stderr.contains("mcp-gate status"));
    assert!(!stderr.contains(TOKEN));
}

#[tokio::test]
async fn first_connection_failure_returns_a_json_rpc_error_and_start_hint() {
    let dir = tempfile::tempdir().unwrap();
    // A listener cannot own port zero. Using it avoids a race with another fixture.
    let config = config(dir.path(), "127.0.0.1:0".parse().unwrap());
    let mut cmd = command("fixture", dir.path());
    cmd.arg("--config").arg(config);
    let mut proxy = Proxy::spawn(cmd);
    proxy.send(initialize(false)).await;
    let reply = proxy.read().await;
    assert_eq!(reply["id"], 1);
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("mcp-gate serve")
    );
    assert!(proxy.close().await.contains("Cannot reach the gateway"));
}

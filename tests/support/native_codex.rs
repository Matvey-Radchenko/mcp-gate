//! Real app-bundled Codex, isolated home/cwd and ephemeral threads. Never starts
//! a model turn or copies account credentials. Only the fixture MCP is enabled.
use super::Harness;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, process::Stdio, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
};
#[cfg(windows)]
#[path = "../../src/platform/windows_job.rs"]
mod windows_job;

pub(crate) struct NativeCodex {
    _home: TempDir,
    child: Child,
    #[cfg(windows)]
    job: windows_job::Job,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
    next: u64,
    request_timeout: Duration,
    ready_threads: BTreeSet<String>,
    last_event: Option<String>,
    pub thread: String,
}
impl NativeCodex {
    pub async fn start(h: &Harness) -> Self {
        Self::for_config(&h.config).await
    }
    /// Explicit live probes can reuse the isolated client without a mock gateway.
    pub async fn for_config(config_path: &std::path::Path) -> Self {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let config = mcp_gate::config::Config::load(config_path).unwrap();
        // A first tool call includes lazy backend/browser startup. Do not have
        // this fixture cancel it before the gateway's own configured deadlines.
        let tool_timeout = config
            .startup_timeout_seconds
            .saturating_add(config.queue_timeout_seconds)
            .saturating_add(config.call_timeout_seconds);
        let helper = mcp_gate::manage::runtime::headers_helper(
            std::path::Path::new(env!("CARGO_BIN_EXE_mcp-gate")),
            config_path,
        );
        let text = format!(
            r#"
model_provider = "offline-fixture"
[model_providers.offline-fixture]
name = "Offline MCP test - never called"
base_url = "http://127.0.0.1:9/v1"
wire_api = "responses"
requires_openai_auth = false
[mcp_servers.gateway-probe]
url = {url:?}
http_headers_helper = {helper:?}
startup_timeout_sec = 20
tool_timeout_sec = {tool_timeout}
"#,
            url = format!("http://{}/mcp", config.listen)
        );
        std::fs::write(home.path().join("config.toml"), text).unwrap();
        let binary =
            PathBuf::from(std::env::var_os("CODEX_BINARY").expect("Set app-bundled Codex"));
        // Concurrent client startup must not block the async runtime while the
        // other client's RPC deadline is already running.
        let checked = tokio::time::timeout(
            Duration::from_secs(60),
            tokio::process::Command::new(&binary)
                .env("CODEX_HOME", home.path())
                .current_dir(&cwd)
                .kill_on_drop(true)
                .args(["mcp", "list", "--json"])
                .output(),
        )
        .await
        .expect("Native isolated config inventory timed out")
        .unwrap();
        assert!(checked.status.success(), "Isolated configuration rejected");
        let inventory: Value = serde_json::from_slice(&checked.stdout).unwrap();
        let enabled: Vec<_> = inventory
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["enabled"] == true)
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            enabled,
            vec!["gateway-probe"],
            "Refusing unrelated MCP startup"
        );
        let mut command = tokio::process::Command::new(&binary);
        command
            .env("CODEX_HOME", home.path())
            .current_dir(&cwd)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // The Windows npm command is a .cmd -> Node -> Codex process tree.
        // Killing only its shell would leave the actual test client running.
        #[cfg(windows)]
        command.creation_flags(0x00000004 | 0x08000000);
        let mut child = command.spawn().unwrap();
        #[cfg(windows)]
        let job = windows_job::Job::assign(&child).unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut client = Self {
            _home: home,
            child,
            #[cfg(windows)]
            job,
            input,
            output,
            next: 1,
            request_timeout: Duration::from_secs(tool_timeout.saturating_add(10)),
            ready_threads: BTreeSet::new(),
            last_event: None,
            thread: String::new(),
        };
        client
            .request(
                "initialize",
                json!({"clientInfo":{"name":"gateway-native-test","version":"1"},
            "capabilities":{"experimentalApi":true}}),
            )
            .await;
        client.send(json!({"method":"initialized"})).await;
        let started = client
            .request(
                "thread/start",
                json!({"cwd":cwd,"ephemeral":true,
            "approvalPolicy":"never","sandbox":"read-only"}),
            )
            .await;
        assert_eq!(
            started["thread"]["ephemeral"], true,
            "Fixture must not persist a task"
        );
        client.thread = started["thread"]["id"].as_str().unwrap().to_owned();
        client
    }

    async fn send(&mut self, value: Value) {
        self.input
            .as_mut()
            .unwrap()
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({"id":id,"method":method,"params":params}))
            .await;
        tokio::time::timeout(self.request_timeout, async {
            loop {
                let value = self.next_message().await;
                if value["id"] == id && value.get("method").is_none() {
                    assert!(
                        value.get("error").is_none(),
                        "Test RPC {method}: {}",
                        value["error"]
                    );
                    return value["result"].clone();
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "Native RPC {method} timed out; process={:?}, last event={:?}, ready threads={}",
                self.child.try_wait(),
                self.last_event,
                self.ready_threads.len()
            )
        })
    }
    async fn next_message(&mut self) -> Value {
        let line = self
            .output
            .next_line()
            .await
            .unwrap()
            .expect("Native Codex exited while waiting for a response or startup event");
        let value: Value = serde_json::from_str(&line).unwrap();
        if let Some(method) = value["method"].as_str() {
            // Lifecycle metadata only: never emit payloads or helper output.
            self.last_event = Some(method.to_owned());
        }
        // No approval or model interactions are expected in this fixture.
        assert!(
            value.get("id").is_none() || value.get("method").is_none(),
            "Unexpected native client request: {}",
            value["method"]
        );
        if value["method"] == "mcpServer/startupStatus/updated"
            && value["params"]["name"] == "gateway-probe"
            && let Some(thread) = value["params"]["threadId"].as_str()
        {
            match value["params"]["status"].as_str().unwrap() {
                "ready" => {
                    self.ready_threads.insert(thread.into());
                }
                "starting" => {
                    self.ready_threads.remove(thread);
                }
                status => panic!("Fixture MCP startup ended with status {status}"),
            }
        }
        value
    }
    async fn ready(&mut self, thread: &str) {
        // thread/start and inventory alone do not establish that the thread's
        // transport is ready. Wait for its documented startup event before
        // querying runtime inventory or counting gateway sessions.
        tokio::time::timeout(self.request_timeout, async {
            while !self.ready_threads.contains(thread) {
                self.next_message().await;
            }
        })
        .await
        .expect("Native thread MCP startup did not complete");
    }
    pub async fn discover(&mut self) -> usize {
        self.discover_in(&self.thread.clone()).await
    }
    pub async fn discover_in(&mut self, thread: &str) -> usize {
        self.ready(thread).await;
        let result = self
            .request(
                "mcpServerStatus/list",
                json!({"threadId":thread,
            "detail":"toolsAndAuthOnly"}),
            )
            .await;
        let data = result["data"].as_array().unwrap();
        let server = data.iter().find(|v| v["name"] == "gateway-probe").unwrap();
        assert_eq!(server["runtimeStatus"], "connected");
        server["tools"]
            .as_object()
            .map(|v| v.len())
            .or_else(|| server["tools"].as_array().map(|v| v.len()))
            .unwrap()
    }
    pub async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.call_in(&self.thread.clone(), tool, arguments).await
    }
    pub async fn call_in(&mut self, thread: &str, tool: &str, arguments: Value) -> Value {
        self.request(
            "mcpServer/tool/call",
            json!({"threadId":thread,
            "server":"gateway-probe","tool":tool,"arguments":arguments}),
        )
        .await
    }
    pub async fn close(&mut self) {
        self.request("thread/unsubscribe", json!({"threadId":self.thread}))
            .await;
        self.input.take();
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            #[cfg(windows)]
            self.job.terminate().unwrap();
            #[cfg(not(windows))]
            self.child.kill().await.unwrap();
            tokio::time::timeout(Duration::from_secs(10), self.child.wait())
                .await
                .expect("Native client did not exit after termination")
                .unwrap();
        }
        #[cfg(windows)]
        {
            self.job.terminate().unwrap();
            self.job.wait_empty().await.unwrap();
        }
    }
}

pub(crate) async fn discovery_sessions(
    config_path: &std::path::Path,
    expected: usize,
) -> BTreeSet<String> {
    // Codex 0.153.4 mcpServerStatus/list builds a separate, threadless connection
    // set even with threadId. The returned snapshot precedes transport teardown
    // and potentially our disconnect grace. Count only after those probes close;
    // persistent extra sessions still fail, and discovery must never start workers.
    let config = mcp_gate::config::Config::load(config_path).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(config.disconnect_grace_seconds.saturating_add(10));
    loop {
        let health: Value = client
            .get(format!("http://{}/health", config.listen))
            .bearer_auth(config.token().unwrap())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            health["workers"], 0,
            "Discovery started a backend: {health}"
        );
        let ids: BTreeSet<_> = health["session_details"]
            .as_array()
            .unwrap()
            .iter()
            .map(|session| session["id"].as_str().unwrap().to_owned())
            .collect();
        if ids.len() == expected && health["sessions"] == expected {
            return ids;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Expected {expected} persistent sessions after discovery cleanup: {health}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

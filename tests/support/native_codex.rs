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

pub(crate) struct NativeCodex {
    _home: TempDir,
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
    next: u64,
    request_timeout: Duration,
    ready_threads: BTreeSet<String>,
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
        let mut check = std::process::Command::new(&binary);
        check.env("CODEX_HOME", home.path()).current_dir(&cwd);
        let checked = check.args(["mcp", "list", "--json"]).output().unwrap();
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
        let mut child = tokio::process::Command::new(&binary)
            .env("CODEX_HOME", home.path())
            .current_dir(&cwd)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut client = Self {
            _home: home,
            child,
            input,
            output,
            next: 1,
            request_timeout: Duration::from_secs(tool_timeout.saturating_add(10)),
            ready_threads: BTreeSet::new(),
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
        .expect("Native test RPC timed out")
    }
    async fn next_message(&mut self) -> Value {
        let line = self
            .output
            .next_line()
            .await
            .unwrap()
            .expect("Native Codex exited while waiting for a response or startup event");
        let value: Value = serde_json::from_str(&line).unwrap();
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
            self.child.kill().await.unwrap();
            self.child.wait().await.unwrap();
        }
    }
}

use futures::StreamExt;
use reqwest::Client;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;
pub(crate) mod native_codex;
pub(crate) const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

pub(crate) struct Harness {
    _dir: TempDir,
    pub(crate) config: PathBuf,
    pub(crate) base: String,
    pub(crate) child: Child,
    pub(crate) cancel_file: PathBuf,
    client_roots: bool,
}
impl Harness {
    pub(crate) async fn set_policy(&mut self, policy: mcp_gate::policy::ToolPolicy) {
        self.stop();
        let mut config = mcp_gate::config::Config::load(&self.config).unwrap();
        config.tool_policy = policy;
        self.restart_with_config(config).await;
    }
    pub(crate) async fn replace_backend(&mut self, backend: mcp_gate::backend::Backend) {
        self.stop();
        let mut config = mcp_gate::config::Config::load(&self.config).unwrap();
        config.backend = backend;
        self.restart_with_config(config).await;
    }
    pub(crate) async fn ignore_shared_roots(&mut self) {
        self.stop();
        let mut config = mcp_gate::config::Config::load(&self.config).unwrap();
        config.shared_client_roots = mcp_gate::config::SharedClientRoots::Ignore;
        self.restart_with_config(config).await;
    }
    async fn restart_with_config(&mut self, config: mcp_gate::config::Config) {
        std::fs::write(&self.config, toml::to_string(&config).unwrap()).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
            .args(["catalog", "--config"])
            .arg(&self.config)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        self.child = Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
            .args(["serve", "--config"])
            .arg(&self.config)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if self.health().await.is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("Replacement test gateway did not start");
    }
    pub(crate) async fn new(real: bool, max_workers: usize) -> Self {
        Self::configured(real, max_workers, None).await
    }
    pub(crate) async fn generic(
        ownership: &str,
        pending: usize,
        call_timeout: u64,
        queue_timeout: u64,
    ) -> Self {
        Self::configured(
            false,
            if ownership == "shared" { 1 } else { 4 },
            Some((ownership, pending, call_timeout, queue_timeout)),
        )
        .await
    }
    async fn configured(
        real: bool,
        max_workers: usize,
        generic: Option<(&str, usize, u64, u64)>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let token = dir.path().join("token");
        std::fs::write(&token, TOKEN).unwrap();
        mcp_gate::platform::private_permissions(&token).unwrap();
        let config = dir.path().join("config.toml");
        let cancel_file = dir.path().join("cancelled");
        let mock = env!("CARGO_BIN_EXE_mock-backend");
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let node = std::env::var("DEVTOOLS_NODE").unwrap_or_else(|_| "/usr/local/bin/node".into());
        let entry = root
            .join("runtime/node_modules/chrome-devtools-mcp/build/src/bin/chrome-devtools-mcp.js");
        let text = format!(
            r#"
listen = "127.0.0.1:{port}"
token_file = {token:?}
catalog_file = {catalog:?}
state_dir = {state:?}
max_workers = {max_workers}
max_sessions = 8
disconnect_grace_seconds = 2
startup_timeout_seconds = 30
call_timeout_seconds = 60
[backend]
command = {command:?}
entrypoint = {entrypoint:?}
version = {version:?}
args = ["--isolated", "--headless", "--no-usage-statistics", "--no-performance-crux"]
[backend.env]
MOCK_CANCEL_FILE = {cancel_file:?}
"#,
            token = token,
            catalog = dir.path().join("catalog.json"),
            state = dir.path().join("state"),
            command = if real { node.as_str() } else { mock },
            entrypoint = if real {
                entry.as_path()
            } else {
                std::path::Path::new(mock)
            },
            version = if real { "1.8.0" } else { "mock-1" }
        );
        let text = if let Some((ownership, pending, call_timeout, queue_timeout)) = generic {
            let mut value: toml::Value = toml::from_str(&text).unwrap();
            let table = value.as_table_mut().unwrap();
            table.insert("format_version".into(), 2.into());
            table.insert("ownership".into(), ownership.into());
            table.insert("max_pending_calls".into(), (pending as i64).into());
            table.insert("call_timeout_seconds".into(), (call_timeout as i64).into());
            table.insert(
                "queue_timeout_seconds".into(),
                (queue_timeout as i64).into(),
            );
            value["backend"]
                .as_table_mut()
                .unwrap()
                .insert("profile".into(), "stdio".into());
            value["backend"]
                .as_table_mut()
                .unwrap()
                .remove("entrypoint");
            value["backend"]["args"] = toml::Value::Array(vec![]);
            toml::to_string(&value).unwrap()
        } else {
            text
        };
        std::fs::write(&config, text).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
            .args(["catalog", "--config"])
            .arg(&config)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "catalog: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let child = Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
            .args(["serve", "--config"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let harness = Self {
            _dir: dir,
            config,
            base: format!("http://127.0.0.1:{port}"),
            child,
            cancel_file,
            client_roots: generic.is_none(),
        };
        for _ in 0..100 {
            if harness.health().await.is_some() {
                return harness;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("Gateway failed to start")
    }
    pub(crate) async fn health(&self) -> Option<Value> {
        Client::new()
            .get(format!("{}/health", self.base))
            .bearer_auth(TOKEN)
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()
    }
    pub(crate) async fn workers(&self, expected: u64) {
        for _ in 0..160 {
            if self.health().await.unwrap()["workers"] == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("Expected {expected} workers, got {:?}", self.health().await)
    }
    pub(crate) async fn session(&self) -> Session {
        Session::with_roots(&self.base, self.client_roots).await
    }
    pub(crate) fn stop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: the harness owns this unreaped child; kill takes only scalars.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
            assert!(self.child.wait().unwrap().success());
        }
        #[cfg(windows)]
        {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
        }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[derive(Clone)]
pub(crate) struct Session {
    pub(crate) client: Client,
    pub(crate) url: String,
    pub(crate) id: String,
    pub(crate) next: Arc<AtomicU64>,
    pub(crate) progress: Arc<AtomicU64>,
}
impl Session {
    pub(crate) async fn new(base: &str) -> Self {
        Self::with_roots(base, true).await
    }
    pub(crate) async fn with_roots(base: &str, roots: bool) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .unwrap();
        let response = client.post(format!("{base}/mcp")).bearer_auth(TOKEN).header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
                "protocolVersion":"2025-11-25", "capabilities": if roots { json!({"roots":{"listChanged":true}}) } else { json!({}) }, "clientInfo":{"name":"identical-client", "version":"1"}
            }})).send().await.unwrap();
        assert!(response.status().is_success(), "{}", response.status());
        let id = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .into();
        let session = Self {
            client,
            url: format!("{base}/mcp"),
            id,
            next: Arc::new(AtomicU64::new(1)),
            progress: Arc::new(AtomicU64::new(0)),
        };
        session.parse(response).await;
        session
            .post(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}))
            .await;
        session
    }
    pub(crate) fn req(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        self.client
            .request(method, &self.url)
            .bearer_auth(TOKEN)
            .header("Accept", "application/json, text/event-stream")
            .header("mcp-session-id", &self.id)
            .header("mcp-protocol-version", "2025-11-25")
    }
    pub(crate) async fn post(&self, value: Value) -> reqwest::Response {
        self.req(reqwest::Method::POST)
            .json(&value)
            .send()
            .await
            .unwrap()
    }
    pub(crate) async fn request(&self, method: &str, params: Value) -> Value {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let response = self
            .post(json!({"jsonrpc":"2.0", "id":id,"method":method,"params":params}))
            .await;
        assert!(response.status().is_success(), "HTTP {}", response.status());
        self.parse(response).await
    }
    pub(crate) async fn parse(&self, response: reqwest::Response) -> Value {
        if response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json")
        {
            return response.json().await.unwrap();
        }
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        while let Some(chunk) = stream.next().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            while let Some(end) = buffer.find('\n') {
                let line = buffer[..end].trim_end().to_owned();
                buffer.drain(..=end);
                if let Some(data) = line.strip_prefix("data:") {
                    let Ok(value) = serde_json::from_str::<Value>(data.trim()) else {
                        continue;
                    };
                    if value["method"] == "roots/list" {
                        self.post(json!({"jsonrpc":"2.0", "id":value["id"], "result":{"roots":[{"uri":"file:///tmp/gateway-test", "name":"test"}]}})).await;
                    } else if value["method"] == "notifications/progress" {
                        assert_eq!(value["params"]["progressToken"], "test-progress");
                        self.progress.fetch_add(1, Ordering::SeqCst);
                    } else if value.get("result").is_some() || value.get("error").is_some() {
                        return value;
                    }
                }
            }
        }
        json!({"transportEnded":true}) // A cancelled MCP request may end without a result.
    }
    pub(crate) async fn call(&self, name: &str, args: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":args}))
            .await
    }
    pub(crate) async fn close(&self) {
        assert!(
            self.req(reqwest::Method::DELETE)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
}
pub(crate) fn content(value: &Value) -> &str {
    value["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("Missing tool text: {value}"))
}
pub(crate) fn mock_value(value: &Value) -> Value {
    serde_json::from_str(content(value)).unwrap()
}
#[cfg(unix)]
pub(crate) fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only probes the OS PID; kill dereferences no Rust memory.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub(crate) fn alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // SAFETY: OS process query takes a scalar PID; code is a valid output pointer.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0;
        let running = GetExitCodeProcess(handle, &mut code) != 0 && code == 259;
        CloseHandle(handle);
        running
    }
}

#![cfg(feature = "test-backend")]
#[path = "support/offline_model.rs"]
mod offline_model;
#[allow(
    dead_code,
    reason = "Fixtures are shared between independent native-client tests"
)]
mod support;
use serde_json::json;
use std::{fs, path::Path, process::Stdio, time::Duration};
use tokio::process::Command;
fn isolated(binary: &str, root: &Path) -> Command {
    for directory in ["claude", "config", "data", "cache", "state", "opencode"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    let mut command = Command::new(
        std::env::var_os(binary).expect("Native client binary must be explicitly selected"),
    );
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("OPENCODE_CONFIG_DIR", root.join("opencode"))
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
        .env("OPENCODE_DISABLE_MODELS_FETCH", "1")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in [
        "SystemRoot",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "COMSPEC",
        "PATHEXT",
        "HOME",
        "TMPDIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
}
#[tokio::test]
#[ignore = "Requires CLAUDE_BINARY; uses a local model fixture and no paid API or personal settings"]
async fn claude_calls_mock_tool_via_gateway_using_private_headers_helper() {
    let mut h = support::Harness::generic("session", 4, 10, 5).await;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("called");
    let mut backend = mcp_gate::config::Config::load(&h.config).unwrap().backend;
    backend.env.insert(
        "MOCK_CLIENT_CALL_FILE".into(),
        marker.to_string_lossy().into_owned(),
    );
    h.replace_backend(backend).await;
    let config = dir.path().join("mcp.json");
    let binary = Path::new(env!("CARGO_BIN_EXE_mcp-gate"));
    fs::write(
        &config,
        json!({"mcpServers":{"gateway-probe":{"type":"http","url":format!("{}/mcp",h.base),
        "headersHelper":mcp_gate::manage::runtime::headers_helper(binary, &h.config)}}})
        .to_string(),
    )
    .unwrap();
    let (base, model) = offline_model::start().await;
    let debug = dir.path().join("claude-debug.log");
    let result = tokio::time::timeout(
        Duration::from_secs(90),
        isolated("CLAUDE_BINARY", dir.path())
            .env("ANTHROPIC_BASE_URL", base)
            .env("ANTHROPIC_API_KEY", "offline-fixture-only")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .arg("--debug-file")
            .arg(&debug)
            .args(["--bare", "--strict-mcp-config", "--mcp-config"])
            .arg(&config)
            .args([
                "--model",
                "sonnet",
                "--allowedTools",
                "mcp__gateway-probe__state",
                "--max-turns",
                "3",
                "-p",
                "Use the fixture state tool once, then report completion.",
            ])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    model.abort();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        marker.is_file(),
        "Real Claude did not execute the fixture tool: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        fixture_mcp_diagnostics(&debug)
    );
    assert_eq!(fs::read_to_string(marker).unwrap(), "offline-model-fixture");
    h.stop();
}
#[tokio::test]
#[ignore = "Requires OPENCODE_BINARY; isolated configs and local mock MCP only"]
async fn opencode_connects_using_private_file_header_without_starting_backend() {
    let mut h = support::Harness::generic("session", 4, 10, 5).await;
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("authorization");
    mcp_gate::manage::store::atomic(&auth, format!("Bearer {}", support::TOKEN).as_bytes())
        .unwrap();
    let config = dir.path().join("opencode.json");
    fs::write(&config,json!({"mcp":{"gateway-probe":{"type":"remote","url":format!("{}/mcp",h.base),
        "headers":{"Authorization":format!("{{file:{}}}",auth.display())},"oauth":false}},"permission":{"*":"deny"}}).to_string()).unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        isolated("OPENCODE_BINARY", dir.path())
            .env("OPENCODE_CONFIG", &config)
            .args(["mcp", "list"])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("gateway-probe") && text.contains("connected"),
        "{text}"
    );
    h.workers(0).await;
    h.stop();
}

#[tokio::test]
#[ignore = "Requires OPENCODE_BINARY; a local Anthropic model fixture calls a local MCP tool without paid APIs"]
async fn opencode_calls_mock_tool_through_the_gateway() {
    let mut h = support::Harness::generic("session", 4, 10, 5).await;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("called");
    let mut backend = mcp_gate::config::Config::load(&h.config).unwrap().backend;
    backend.env.insert(
        "MOCK_CLIENT_CALL_FILE".into(),
        marker.to_string_lossy().into_owned(),
    );
    h.replace_backend(backend).await;
    let auth = dir.path().join("authorization");
    mcp_gate::manage::store::atomic(&auth, format!("Bearer {}", support::TOKEN).as_bytes())
        .unwrap();
    let (base, model) = offline_model::start().await;
    let config = dir.path().join("opencode.json");
    fs::write(&config,json!({
        "model":"anthropic/claude-sonnet-4-5","small_model":"anthropic/claude-sonnet-4-5",
        "enabled_providers":["anthropic"],"provider":{"anthropic":{"options":{"baseURL":format!("{base}/v1"),"apiKey":"offline-fixture-only"}}},
        "mcp":{"fixture":{"type":"remote","url":format!("{}/mcp",h.base),"oauth":false,"headers":{"Authorization":format!("{{file:{}}}",auth.display())}}},
        "permission":{"*":"deny","fixture_*":"allow"}
    }).to_string()).unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(90),
        isolated("OPENCODE_BINARY", dir.path())
            .env("OPENCODE_CONFIG", &config)
            .env("ANTHROPIC_API_KEY", "offline-fixture-only")
            .args([
                "run",
                "--format",
                "json",
                "Use the fixture state tool once, then report completion.",
            ])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    model.abort();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        marker.is_file(),
        "Real OpenCode did not call the fixture: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read_to_string(marker).unwrap(), "offline-model-fixture");
    h.stop();
}

#[tokio::test]
#[ignore = "Requires CLAUDE_BINARY; verifies actual local/project precedence in disposable settings"]
async fn claude_local_scope_uses_the_installer_project_key_and_overrides_shared_config() {
    let dir = tempfile::Builder::new()
        .prefix("Claude project Юникод ")
        .tempdir()
        .unwrap();
    let root = mcp_gate::platform::project_path(dir.path()).unwrap();
    let shared = root.join(".mcp.json");
    let bytes = json!({"mcpServers":{"fixture":{"type":"http","url":"http://127.0.0.1:2/shared"}}})
        .to_string();
    fs::write(&shared, &bytes).unwrap();
    let add = tokio::time::timeout(
        Duration::from_secs(30),
        isolated("CLAUDE_BINARY", &root)
            .args([
                "mcp",
                "add",
                "--scope",
                "local",
                "--transport",
                "http",
                "fixture",
                "http://127.0.0.1:1/local",
            ])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("claude/.claude.json")).unwrap()).unwrap();
    assert_eq!(
        settings["projects"][mcp_gate::clients::claude_project_key(&root).unwrap()]["mcpServers"]["fixture"]
            ["url"],
        "http://127.0.0.1:1/local",
        "Actual fixture project keys: {:?}",
        settings["projects"]
            .as_object()
            .map(|entries| entries.keys().collect::<Vec<_>>())
    );
    let get = tokio::time::timeout(
        Duration::from_secs(30),
        isolated("CLAUDE_BINARY", &root)
            .args(["mcp", "get", "fixture"])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        get.status.success(),
        "{}",
        String::from_utf8_lossy(&get.stderr)
    );
    assert!(String::from_utf8_lossy(&get.stdout).contains("http://127.0.0.1:1/local"));
    assert_eq!(fs::read_to_string(shared).unwrap(), bytes);
}

fn fixture_mcp_diagnostics(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains("MCP server") || line.contains("headersHelper"))
        .take(30)
        .collect::<Vec<_>>()
        .join("\n")
        .replace(support::TOKEN, "<hidden>")
        .replace("offline-fixture-only", "<hidden>")
}

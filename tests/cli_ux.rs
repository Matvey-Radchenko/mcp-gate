#![cfg(feature = "test-backend")]
use mcp_gate::clients::{self, Client};
use serde_json::{Value, json};
use std::{
    fs,
    process::{Command, Output},
};

struct Fixture {
    directory: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        for path in [".codex", ".config/opencode", "claude"] {
            fs::create_dir_all(directory.path().join(path)).unwrap();
        }
        Self { directory }
    }
    fn command(&self, args: &[&str]) -> Command {
        let root = self.directory.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-gate"));
        command
            .args(args)
            .current_dir(root)
            .env("MCP_GATE_TEST_HOME", root)
            .env("MCP_GATE_TEST_ROOT", root.join("state"))
            .env("CODEX_HOME", root.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", root.join("claude"))
            .env("XDG_CONFIG_HOME", root.join(".config"))
            .env_remove("OPENCODE_CONFIG");
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        let output = self.command(args).output().unwrap();
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("never-print-this-token"));
        }
        assert!(
            !self.directory.path().join("state").exists(),
            "Passive CLI must not create managed state"
        );
        output
    }
    fn codex(&self, value: Value) {
        fs::write(
            self.directory.path().join(".codex/config.toml"),
            toml::to_string(&value).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn default_empty_commands_are_readable_and_do_not_require_flags() {
    let fixture = Fixture::new();
    for command in ["status", "setup", "remove"] {
        let output = fixture.run(&[command]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(!text.trim_start().starts_with('{'));
        assert!(!text.contains("[y/N]"));
        assert!(text.contains(if command == "remove" {
            "Nothing to remove"
        } else if command == "setup" {
            "No setup changes"
        } else {
            "No MCP connections found"
        }));
    }
}

#[test]
fn home_is_not_a_project_and_same_file_aliases_are_not_duplicated() {
    let fixture = Fixture::new();
    fixture.codex(json!({"mcp_servers":{"example":{"command":"npx","args":["package"],"env":{"TOKEN":"never-print-this-token"}}}}));
    let root = fixture.directory.path();
    for selected in [false, true] {
        let found = clients::discover(root, root, &[Client::Codex], selected).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].binding.project.is_none());
        assert!(!found[0].issue.as_ref().unwrap().contains("Project file"));
    }
    #[cfg(unix)]
    {
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        std::os::unix::fs::symlink(root.join(".codex"), project.join(".codex")).unwrap();
        assert_eq!(
            clients::discover(root, &project, &[Client::Codex], true)
                .unwrap()
                .len(),
            1
        );
    }
    let output = fixture.run(&["status", "--json"]);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["connections"].as_array().unwrap().len(), 1);
    assert_eq!(value["connections"][0]["kind"], "direct");
}

#[test]
fn http_is_not_claimed_as_a_gateway_and_helpers_are_not_executed() {
    let fixture = Fixture::new();
    let marker = fixture.directory.path().join("helper-was-run");
    fixture.codex(json!({"mcp_servers":{"remote":{"url":"http://127.0.0.1:49871/mcp","http_headers_helper":format!("touch {}", marker.display()),"http_headers":{"Authorization":"never-print-this-token"}}}}));
    for args in [
        vec!["status", "--json"],
        vec!["setup", "--dry-run", "--json"],
        vec!["remove", "--json"],
    ] {
        let output = fixture.run(&args);
        assert!(output.status.success());
        let v: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(v["format_version"], 1);
        assert!(!marker.exists());
        let connections = if args[0] == "status" {
            &v["connections"]
        } else if args[0] == "setup" {
            &v["plan"]["connections"]
        } else {
            &v["discovered_connections"]
        };
        assert_eq!(connections[0]["kind"], "http");
        assert_eq!(connections[0]["state"], "not checked");
    }
}

#[test]
fn broken_client_source_does_not_hide_other_connections_or_leak_parser_values() {
    let fixture = Fixture::new();
    fs::write(
        fixture.directory.path().join(".codex/config.toml"),
        "never-print-this-token = [\n",
    )
    .unwrap();
    fs::write(
        fixture
            .directory
            .path()
            .join(".config/opencode/opencode.jsonc"),
        r#"{"mcp":{"ok":{"type":"remote","url":"https://example.invalid/mcp"}}}"#,
    )
    .unwrap();
    let output = fixture.run(&["status", "--json"]);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["source_issues"].as_array().unwrap().len(), 1);
    assert_eq!(value["connections"][0]["name"], "ok");
}

#[test]
fn dry_run_is_secret_free_and_noninteractive_setup_never_silently_applies() {
    let fixture = Fixture::new();
    fixture.codex(json!({"mcp_servers":{"fixture":{"command":env!("CARGO_BIN_EXE_mock-backend"),"cwd":fixture.directory.path(),"args":[],"env":{"TOKEN":"never-print-this-token"},"disabled_tools":["danger"]}}}));
    let original = fs::read(fixture.directory.path().join(".codex/config.toml")).unwrap();
    let output = fixture.run(&["setup", "--dry-run", "--diff", "--json"]);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["plan"]["servers"][0]["mode"], "session");
    assert!(value["plan"]["servers"][0]["diff"]["before"]["disabled_tools"].is_array());
    for args in [
        vec!["setup"],
        vec!["setup", "--json"],
        vec!["setup", "--yes", "--json"],
        vec!["setup", "--all", "--yes", "--json"],
    ] {
        let output = fixture.run(&args);
        assert!(!output.status.success());
        if args.contains(&"--json") {
            let v: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(v["phase"], "failed");
        }
    }
    assert_eq!(
        fs::read(fixture.directory.path().join(".codex/config.toml")).unwrap(),
        original
    );
}

#[test]
#[cfg(target_os = "macos")]
fn legacy_health_is_observed_without_adopting_or_starting_the_backend() {
    for (runtime, expected) in [
        (json!({}), "reachable; backend unknown"),
        (json!({"shared_state":"dormant"}), "ready; backend idle"),
        (
            json!({"shared_state":"failed"}),
            "blocked; backend unavailable",
        ),
        (
            json!({"shared_state":"recovering"}),
            "recovering; backend unavailable",
        ),
    ] {
        legacy_health_fixture(runtime, expected);
    }
}

#[cfg(target_os = "macos")]
fn legacy_health_fixture(runtime: Value, expected: &str) {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let root = fixture.directory.path();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok((mut stream, _)) = listener.accept() {
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                let length = stream.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..length]).starts_with("GET /health "));
                let body = json!({"service":"mcp-session-gateway","workers":0,"sessions":0,"runtime":runtime,"extra":"never-print-this-token"}).to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                break;
            }
            assert!(
                Instant::now() < deadline,
                "No legacy health request received"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let token = root.join("legacy-token");
    fs::write(&token, "a".repeat(64)).unwrap();
    mcp_gate::platform::private_permissions(&token).unwrap();
    let config = root.join("legacy.toml");
    fs::write(&config, toml::to_string(&json!({"format_version":2,"listen":address.to_string(),"token_file":token,"catalog_file":root.join("unused-catalog.json"),"state_dir":root.join("unused-state"),
        "backend":{"profile":"stdio","command":env!("CARGO_BIN_EXE_mock-backend"),"version":"fixture","working_directory":root}})).unwrap()).unwrap();
    let agents = root.join("Library/LaunchAgents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(
        agents.join("fixture.plist"),
        json!({"ProgramArguments":["/fixture/mcp-session-gateway","serve","--config",config]})
            .to_string(),
    )
    .unwrap();
    fixture.codex(json!({"mcp_servers":{"existing":{"url":format!("http://{address}/mcp")}}}));
    let output = fixture.run(&["status", "--json"]);
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["connections"][0]["kind"], "legacy");
    assert_eq!(result["connections"][0]["state"], expected);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("never-print-this-token"));
    assert_eq!(result["gateways"], json!([]));
    server.join().unwrap();
    let output = fixture.run(&["remove"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Found 1 legacy gateway connections"));
    assert!(!root.join("unused-state").exists());
}

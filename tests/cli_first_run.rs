#![cfg(feature = "test-backend")]
use serde_json::{Value, json};
use std::{fs, process::Command};

struct Fixture {
    directory: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        for path in [".codex", "claude", ".config/opencode"] {
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
    fn run(&self, args: &[&str]) -> std::process::Output {
        let output = self.command(args).output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(!text.contains("never-print-this-token"), "{text}");
        assert!(!err.contains("never-print-this-token"), "{err}");
        output
    }
    fn codex(&self, servers: Value) {
        fs::write(
            self.directory.path().join(".codex/config.toml"),
            toml::to_string(&json!({"mcp_servers": servers})).unwrap(),
        )
        .unwrap();
    }
    fn claude(&self, servers: Value) {
        fs::write(
            self.directory.path().join("claude/.claude.json"),
            json!({"mcpServers": servers}).to_string(),
        )
        .unwrap();
    }
}

fn backend() -> String {
    env!("CARGO_BIN_EXE_mock-backend").to_string()
}

#[test]
fn dry_run_keeps_connections_that_need_a_directory() {
    let fixture = Fixture::new();
    let command = backend();
    fixture.codex(json!({"filesystem": {"command": command, "args": ["/tmp"]}}));
    fixture.claude(json!({"time": {"command": command, "args": []}}));
    let output = fixture.run(&["setup", "--dry-run"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(
        "needs a working directory — the interactive setup will ask, or pass --cwd <dir>"
    ));
    assert!(text.contains("2 personal connections need an explicit directory."));
    assert!(text.contains("No gateway would be registered until a working directory is chosen."));
    assert!(!text.contains("No setup changes"));
    assert!(!text.contains("0 personal"));
    assert!(!text.contains("setup --project"));
    assert!(!text.contains("gateways start now"));
    let json = fixture.run(&["setup", "--dry-run", "--json"]);
    let value: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["plan"]["servers"].as_array().unwrap().len(), 2);
    assert!(
        value["plan"]["servers"][0]["directory_hint"]
            .as_str()
            .unwrap()
            .contains("--cwd")
    );
}

#[test]
fn one_missing_directory_is_singular_and_details_are_not_repeated() {
    let fixture = Fixture::new();
    fixture.codex(json!({"only": {"command": backend(), "args": []}}));
    let output = fixture.run(&["setup", "--dry-run", "--details"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("1 personal connection needs an explicit directory."));
    assert!(!text.contains("setup --details"));
    assert!(!text.contains("0 personal"));
}

#[test]
fn cwd_flag_fills_only_connections_that_do_not_declare_one() {
    let fixture = Fixture::new();
    let kept = fixture.directory.path().join("kept");
    fs::create_dir(&kept).unwrap();
    let command = backend();
    fixture.codex(json!({
        "kept": {"command": command, "args": ["kept"], "cwd": kept},
        "open": {"command": command, "args": ["open"]}
    }));
    let missing = fixture.run(&["setup", "--dry-run", "--cwd", "/no/such/mcp-gate-dir"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("existing absolute directory"));
    let directory = fixture.directory.path().to_string_lossy().into_owned();
    let output = fixture.run(&["setup", "--dry-run", "--cwd", &directory, "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let servers = value["plan"]["servers"].as_array().unwrap();
    let kept_plan = servers.iter().find(|s| s["name"] == "kept").unwrap();
    let open_plan = servers.iter().find(|s| s["name"] == "open").unwrap();
    assert!(
        kept_plan["working_directory"]
            .as_str()
            .unwrap()
            .contains("kept")
    );
    assert!(
        open_plan["working_directory"].as_str().unwrap().contains(
            fixture
                .directory
                .path()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
        )
    );
    assert!(open_plan.get("directory_hint").is_none());
    assert!(
        open_plan["service_label"]
            .as_str()
            .unwrap()
            .starts_with("local.mcp-gate.open")
    );
}

#[test]
fn dry_run_wording_is_conditional_and_labels_come_from_the_server_name() {
    let fixture = Fixture::new();
    let cwd = fixture.directory.path().to_string_lossy().into_owned();
    fixture.codex(json!({"Playwright": {"command": backend(), "args": [], "cwd": cwd}}));
    let output = fixture.run(&["setup", "--dry-run"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("User autostart would be registered and gateways would start after setup")
    );
    assert!(text.contains("Discovery would run the original command"));
    assert!(text.contains("Service label: local.mcp-gate.playwright"));
    assert!(!text.contains("gateways start now"));
    assert!(!text.contains("is registered"));
}

#[test]
fn identical_definitions_share_a_gateway_and_collisions_get_a_suffix() {
    let fixture = Fixture::new();
    let cwd = fixture.directory.path().to_string_lossy().into_owned();
    let command = backend();
    fixture.codex(json!({
        "filesystem": {"command": command, "args": ["/tmp"], "cwd": cwd},
        "mail": {"command": command, "args": ["one"], "cwd": cwd}
    }));
    fixture.claude(json!({
        "filesystem": {"command": command, "args": ["/tmp"], "cwd": cwd},
        "mail": {"command": command, "args": ["two"], "cwd": cwd}
    }));
    let output = fixture.run(&["setup", "--dry-run", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let servers = value["plan"]["servers"].as_array().unwrap();
    let label = |name: &str, client: &str| {
        servers
            .iter()
            .find(|server| server["name"] == name && server["client"] == client)
            .unwrap()["service_label"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let shared = label("filesystem", "codex");
    assert_eq!(shared, label("filesystem", "claude-code"));
    assert!(shared.starts_with("local.mcp-gate.filesystem"));
    assert!(
        servers
            .iter()
            .any(|server| { server["name"] == "filesystem" && server["shares_gateway"] == true })
    );
    let mail = [label("mail", "codex"), label("mail", "claude-code")];
    assert!(
        mail.iter()
            .any(|item| item.starts_with("local.mcp-gate.mail"))
    );
    assert!(mail.iter().any(|item| item.ends_with("-2")));
    assert_ne!(mail[0], mail[1]);
}

#[test]
fn help_describes_flags_and_json_works_before_the_subcommand() {
    let fixture = Fixture::new();
    let setup = String::from_utf8(
        fixture
            .command(&["setup", "--help"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    for flag in [
        "--dry-run",
        "--json",
        "--yes",
        "--client",
        "--server",
        "--project",
        "--cwd",
        "--diff",
        "--details",
    ] {
        assert!(
            setup.contains(flag),
            "{flag} missing from setup help:\n{setup}"
        );
    }
    assert!(setup.contains("change nothing"));
    assert!(setup.contains("Requires both --client and --server"));
    assert!(!setup.contains("--all"), "{setup}");
    let remove = String::from_utf8(
        fixture
            .command(&["remove", "--help"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(remove.contains("--all"));
    assert!(remove.contains("remove --all --yes"));
    let status = String::from_utf8(
        fixture
            .command(&["status", "--help"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(status.contains("--probe"));
    assert!(status.contains("--details"));
    let adopt = String::from_utf8(
        fixture
            .command(&["adopt", "--help"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(adopt.contains("--dry-run"));
    assert!(adopt.contains("--json"));
    let output = fixture.run(&["--json", "status"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["format_version"], 1);
}

#[test]
fn no_subcommand_explains_what_to_run_first() {
    let fixture = Fixture::new();
    let output = fixture.run(&[]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("mcp-gate setup"));
    assert!(text.contains("mcp-gate status"));
    assert!(!text.contains("error:"));
    let json = fixture.run(&["--json"]);
    assert!(json.status.success());
    let value: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(value["phase"], "help");
}

#[test]
fn remove_yes_without_targets_suggests_all() {
    let fixture = Fixture::new();
    let output = fixture.run(&["remove", "--yes"]);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("remove --all --yes"), "{err}");
    assert!(
        err.contains("--yes requires both --client and --server"),
        "{err}"
    );
}

#[test]
fn status_explains_direct_connections_and_rejects_unknown_filters() {
    let fixture = Fixture::new();
    let cwd = fixture.directory.path().to_string_lossy().into_owned();
    fixture.codex(json!({"filesystem": {"command": backend(), "args": ["/tmp"], "cwd": cwd}}));
    let output = fixture.run(&["status"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("health not checked"), "{text}");
    assert!(!text.contains("Application switch"));
    let detailed = fixture.run(&["status", "--details"]);
    let detailed = String::from_utf8(detailed.stdout).unwrap();
    assert!(detailed.contains("backend health is not checked"));
    assert!(!detailed.contains("status --details"));
    let missing = fixture.run(&["status", "--json", "--server", "does-not-exist"]);
    assert!(!missing.status.success());
    let value: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(value["phase"], "failed");
    assert!(value["error"].as_str().unwrap().contains("does-not-exist"));
    let client = fixture.run(&["status", "--client", "opencode"]);
    assert!(!client.status.success());
    assert!(String::from_utf8_lossy(&client.stderr).contains("client opencode"));
}

#[test]
fn diff_shows_command_arguments_directory_and_url_but_hides_secrets() {
    let fixture = Fixture::new();
    let cwd = fixture.directory.path().to_string_lossy().into_owned();
    fixture.codex(json!({
        "fixture": {
            "command": backend(),
            "cwd": cwd,
            "args": ["--flag", "keep", "--token=never-print-this-token"],
            "env": {"TOKEN": "never-print-this-token"}
        }
    }));
    let output = fixture.run(&["setup", "--dry-run", "--diff", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let diff = &value["plan"]["servers"][0]["diff"];
    assert_eq!(diff["before"]["command"], backend());
    assert_eq!(diff["before"]["args"][1], "keep");
    assert_eq!(diff["before"]["args"][2], "--token=<hidden>");
    assert!(
        diff["before"]["cwd"].as_str().unwrap().contains(
            fixture
                .directory
                .path()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
        )
    );
    assert_eq!(diff["before"]["env"], "<hidden>");
    assert!(
        diff["after"]["url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    let rendered = String::from_utf8(output.stdout).unwrap();
    assert!(!rendered.contains("never-print-this-token"));
}

#[test]
fn project_hint_appears_only_when_claude_has_a_project_server() {
    let fixture = Fixture::new();
    fixture.codex(json!({"remote": {"url": "http://127.0.0.1:9/mcp"}}));
    let plain = String::from_utf8(fixture.run(&["setup", "--dry-run"]).stdout).unwrap();
    assert!(!plain.contains("setup --project"), "{plain}");
    fs::write(
        fixture.directory.path().join("claude/.claude.json"),
        json!({"projects": {"/tmp/project": {"mcpServers": {"local": {"command": "true"}}}}})
            .to_string(),
    )
    .unwrap();
    let hinted = String::from_utf8(fixture.run(&["setup", "--dry-run"]).stdout).unwrap();
    assert!(
        hinted.contains("use setup --project <directory>"),
        "{hinted}"
    );
}

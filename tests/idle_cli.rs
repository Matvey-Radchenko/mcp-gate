#![cfg(feature = "test-backend")]
use mcp_gate::{
    clients::{Binding, Client},
    config::Config,
    manage::store::{self, Record, Registry},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    record: Record,
}
impl Fixture {
    fn new(ownership: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("managed");
        store::prepare(&root).unwrap();
        let release = root.join("releases/fixture");
        fs::create_dir(&release).unwrap();
        let record = Record {
            removing: false,
            id: "fixture-id".into(),
            release,
            label: "local.mcp-gate.fixture".into(),
            binary_hash: "fixture".into(),
            // A nonexistent service binary makes accidental service execution fail.
            binary: root.join("service-must-not-run"),
            bindings: vec![Binding {
                client: Client::CODEX,
                name: "api".into(),
                source: root.join("client-must-not-read"),
                target: root.join("client-must-not-write"),
                source_path: vec![],
                path: vec![],
                toml: true,
                project: None,
                before: json!({}),
                direct: json!({}),
                after: json!({"token":"fixture-value-must-stay-private"}),
            }],
        };
        let config: Config = serde_json::from_value(json!({
            "format_version":2,"ownership":ownership,"listen":"127.0.0.1:12345",
            "max_workers":1,"state_dir":root.join("runtime"),
            "token_file":root.join("client-token-must-not-read"),
            "catalog_file":root.join("catalog-must-not-read.json"),
            "backend":{"profile":"stdio","command":env!("CARGO_BIN_EXE_mock-backend"),
                "args":[],"version":"mock-1",
                "env_files":{"API_TOKEN":root.join("backend-token-must-not-read")}}
        }))
        .unwrap();
        let text = format!(
            "# Retain this operator comment.\n{}",
            toml::to_string(&config).unwrap()
        );
        store::atomic(&record.config(), text.as_bytes()).unwrap();
        store::save(
            &root,
            &Registry {
                format_version: 1,
                gateways: vec![record.clone()],
            },
        )
        .unwrap();
        Self {
            _directory: directory,
            root,
            record,
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
            .args(args)
            .env("MCP_GATE_TEST_ROOT", &self.root)
            .env("MCP_GATE_TEST_HOME", &self.root)
            .output()
            .unwrap();
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("fixture-value-must-stay-private"));
        }
        for file in [
            "client-token-must-not-read",
            "backend-token-must-not-read",
            "client-must-not-write",
            "service-must-not-run",
        ] {
            assert!(!self.root.join(file).exists());
        }
        output
    }
    fn run_json(&self, args: &[&str]) -> Output {
        self.run(&[&["--json"], args].concat())
    }
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.run_json(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(self.record.config()).unwrap()
    }
}

#[test]
fn show_uses_ownership_defaults_without_reading_credentials_or_creating_a_lock() {
    for (ownership, seconds) in [("session", 900), ("shared", 0)] {
        let fixture = Fixture::new(ownership);
        let before = fixture.bytes();
        for name in ["fixture-id", "local.mcp-gate.fixture", "fixture", "api"] {
            let value = fixture.ok(&["idle", name]);
            assert_eq!(value["phase"], "current");
            assert_eq!(value["idle_seconds"], seconds);
            assert_eq!(value["uses_ownership_default"], true);
            assert_eq!(value["restart_required"], false);
            assert!(value.get("restart_command").is_none());
        }
        assert_eq!(fixture.bytes(), before);
        assert!(!fixture.root.join("setup.lock").exists());
    }
}

#[test]
fn duration_previews_and_applies_preserve_config_and_never_restart_services() {
    let fixture = Fixture::new("shared");
    let registry = fs::read(fixture.root.join("registry.json")).unwrap();
    for (duration, seconds) in [
        ("900", 900),
        ("1h", 3600),
        ("15m", 900),
        ("off", 0),
        ("90s", 90),
        ("0", 0),
    ] {
        let before = fixture.bytes();
        let preview = fixture.ok(&["idle", "fixture", duration, "--dry-run"]);
        assert_eq!(preview["phase"], "preview");
        assert_eq!(preview["idle_seconds"], seconds);
        assert!(
            preview["restart_command"]
                .as_str()
                .unwrap()
                .contains("fixture")
        );
        assert_eq!(fixture.bytes(), before);
        let applied = fixture.ok(&["idle", "fixture", duration, "--yes"]);
        assert_eq!(applied["idle_seconds"], seconds);
        assert_eq!(applied["restart_required"], true);
        let config = Config::load(&fixture.record.config()).unwrap();
        assert_eq!(config.idle_seconds, Some(seconds as u64));
        assert!(
            String::from_utf8(fixture.bytes())
                .unwrap()
                .starts_with("# Retain this operator comment.")
        );
        assert_eq!(
            fs::read(fixture.root.join("registry.json")).unwrap(),
            registry
        );
    }
}

#[test]
fn saved_idle_setting_does_not_suggest_a_restart() {
    let fixture = Fixture::new("shared");
    let config = String::from_utf8(fixture.bytes()).unwrap();
    let before = format!("idle_seconds = +900 # Retain the spelling.\n{config}");
    store::atomic(&fixture.record.config(), before.as_bytes()).unwrap();
    let message = "Nothing to change: fixture-id already has idle set to 900s.";
    for option in ["--dry-run", "--yes"] {
        let report = fixture.ok(&["idle", "fixture", "15m", option]);
        assert_eq!(report["phase"], "unchanged");
        assert_eq!(report["message"], message);
        assert_eq!(report["restart_required"], false);
        assert!(report.get("restart_command").is_none());
    }
    let output = fixture.run(&["idle", "fixture", "900", "--dry-run"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().last(), Some(message));
    assert!(!text.contains("restart"));
    assert_eq!(fixture.bytes(), before.as_bytes());
    assert!(!fixture.root.join("setup.lock").exists());
}

#[test]
fn saving_requires_explicit_confirmation_and_rejects_invalid_durations() {
    let fixture = Fixture::new("shared");
    let before = fixture.bytes();
    let refused = fixture.run_json(&["idle", "fixture", "15m"]);
    assert!(!refused.status.success());
    let value: Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert!(value["error"].as_str().unwrap().contains("--yes"));
    for invalid in ["-1", "1.5m", "1d", "1h30m", "m", "18446744073709551615h"] {
        let output = fixture.run_json(&["idle", "fixture", invalid, "--yes"]);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["phase"],
            "failed"
        );
    }
    assert_eq!(fixture.bytes(), before);
    assert!(!fixture.run(&["idle", "missing"]).status.success());
}

#[test]
fn enabling_shared_docker_idle_is_rejected_but_off_is_supported() {
    let fixture = Fixture::new("shared");
    let mut config = Config::load(&fixture.record.config()).unwrap();
    config.backend.docker = true;
    config.backend.args = vec!["run".into(), "-i".into(), "fixture-image".into()];
    store::atomic(
        &fixture.record.config(),
        toml::to_string(&config).unwrap().as_bytes(),
    )
    .unwrap();
    let before = fixture.bytes();
    let output = fixture.run_json(&["idle", "fixture", "15m", "--yes"]);
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["error"].as_str().unwrap().contains("shared Docker"));
    assert_eq!(fixture.bytes(), before);
    assert_eq!(
        fixture.ok(&["idle", "fixture", "off", "--yes"])["idle_seconds"],
        0
    );
}

#[test]
fn session_eviction_validation_applies_only_when_idle_is_enabled() {
    let fixture = Fixture::new("session");
    let before = fixture.bytes();
    let output = fixture.run_json(&["idle", "fixture", "90s", "--yes"]);
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("session_idle_evict_seconds")
    );
    assert_eq!(fixture.bytes(), before);
    fixture.ok(&["idle", "fixture", "off", "--yes"]);
    fixture.ok(&["idle", "fixture", "15m", "--yes"]);
}

#[test]
fn idle_edits_preserve_comments_and_backend_settings_for_all_input_keys() {
    for key in [
        "idle_seconds",
        "session_idle_seconds",
        "shared_idle_seconds",
    ] {
        let fixture = Fixture::new("shared");
        let before = String::from_utf8(fixture.bytes()).unwrap();
        let text =
            format!("# Preserve this idle note.\n{key} = 900 # Retain this suffix.\n{before}");
        store::atomic(&fixture.record.config(), text.as_bytes()).unwrap();
        fixture.ok(&["idle", "fixture", "1h", "--yes"]);
        let after = String::from_utf8(fixture.bytes()).unwrap();
        if key != "idle_seconds" {
            assert!(!after.contains(key));
        }
        assert!(after.contains("# Preserve this idle note."));
        assert!(after.contains("# Retain this suffix."));
        let backend = |text: &str| toml::from_str::<toml::Value>(text).unwrap()["backend"].clone();
        assert_eq!(backend(&after), backend(&before));
        assert_eq!(
            Config::load(&fixture.record.config()).unwrap().idle_seconds,
            Some(3600)
        );
    }
}

#[test]
fn concurrent_setup_lock_prevents_an_idle_write() {
    let fixture = Fixture::new("shared");
    let before = fixture.bytes();
    let lock = fs::File::create(fixture.root.join("setup.lock")).unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let output = fixture.run_json(&["idle", "fixture", "15m", "--yes"]);
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("operation is running")
    );
    assert_eq!(fixture.bytes(), before);
}

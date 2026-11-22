#![cfg(feature = "test-backend")]
use mcp_gate::{
    backend::Profile,
    config::{Config, Ownership, OwnershipSource, SharedClientRoots, SharedRecovery},
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
    config: Config,
}

impl Fixture {
    fn new() -> Self {
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
            binary_hash: mcp_gate::catalog::digest(env!("CARGO_BIN_EXE_mcp-gate").as_ref())
                .unwrap(),
            binary: root.join("service-must-not-run"),
            bindings: vec![],
        };
        let config = serde_json::from_value(json!({
            "format_version": 2, "ownership": "session", "listen": "127.0.0.1:12345",
            "max_workers": 4, "state_dir": root.join("runtime"),
            "token_file": root.join("client-token-must-not-read"),
            "catalog_file": root.join("catalog.json"),
            "backend": {
                "profile": "stdio", "command": root.join("backend-must-not-run"),
                "version": "fixture", "args": ["--token=private-fixture-argument"],
                "env": {"API_TOKEN": "private-fixture-env"},
                "env_files": {"PASSWORD": root.join("backend-token-must-not-read")}
            }
        }))
        .unwrap();
        store::save(
            &root,
            &Registry {
                format_version: 1,
                gateways: vec![record.clone()],
            },
        )
        .unwrap();
        let fixture = Self {
            _directory: directory,
            root,
            record,
            config,
        };
        fixture.write_config();
        fixture.write_catalog("time", "get_current_time");
        fixture
    }

    fn write_config(&self) {
        let text = toml::to_string(&self.config)
            .unwrap()
            .replace(
                "\nownership = ",
                "\n# Keep this ownership note.\nownership = ",
            )
            .replace(
                "max_workers = 4",
                "max_workers   =   4 # Keep this capacity note.",
            );
        store::atomic(&self.record.config(), text.as_bytes()).unwrap();
    }

    fn write_catalog(&self, name: &str, tool: &str) {
        let catalog = json!({
            "format_version": 3, "backend_version": self.config.backend.version,
            "entrypoint_sha256": "fixture", "args": self.config.backend.args,
            "server_info": {
                "protocolVersion": "2025-11-25",
                "serverInfo": {"name": name, "version": self.config.backend.version},
                "capabilities": {"tools": {}}
            },
            "tools": [{"name": tool, "inputSchema": {"type": "object"},
                "annotations": {"readOnlyHint": true}}]
        });
        store::atomic(
            &self.config.catalog_file,
            &serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        let registry = fs::read(self.root.join("registry.json")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
            .args(args)
            .env("MCP_GATE_TEST_ROOT", &self.root)
            .env("MCP_GATE_TEST_HOME", &self.root)
            .output()
            .unwrap();
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("private-fixture-"));
        }
        assert_eq!(fs::read(self.root.join("registry.json")).unwrap(), registry);
        assert!(!self.config.state_dir.exists());
        output
    }

    fn json(&self, args: &[&str], phase: &str) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let output = self.run(&all);
        assert_eq!(output.status.success(), phase != "failed", "{output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["phase"], phase);
        value
    }

    fn text(&self) -> String {
        fs::read_to_string(self.record.config()).unwrap()
    }
}

#[test]
fn share_dry_run_reports_advice_and_edits_without_writing() {
    let fixture = Fixture::new();
    let before = fixture.text();
    let report = fixture.json(&["share", "fixture", "--dry-run"], "preview");
    assert_eq!(report["advice"]["recommendation"], "shared_candidate");
    assert_eq!(
        report["changes"][0],
        json!({"field":"ownership","previous":"session","new":"shared"})
    );
    assert_eq!(report["restart_required"], true);
    assert!(
        report["restart_command"]
            .as_str()
            .unwrap()
            .contains("fixture")
    );
    assert_eq!(fixture.text(), before);
    assert!(!fixture.root.join("setup.lock").exists());
}

#[test]
fn share_yes_preserves_comments_and_sets_operator_mode_and_one_worker() {
    let fixture = Fixture::new();
    let before = fixture.text();
    fixture.json(&["share", "fixture-id", "--yes"], "complete");
    let after = fixture.text();
    assert_eq!(
        after.replace("ownership_source = \"operator\"\n", ""),
        before
            .replace("ownership = \"session\"", "ownership = \"shared\"")
            .replace("max_workers   =   4", "max_workers   =   1")
    );
    let config = Config::load(&fixture.record.config()).unwrap();
    assert!(config.ownership == Ownership::Shared);
    assert_eq!(config.ownership_source, Some(OwnershipSource::Operator));
    assert_eq!(config.max_workers, 1);
    assert!(config.shared_client_roots == SharedClientRoots::Reject);
}

#[test]
fn share_refuses_chrome_profiles_legacy_formats_and_docker_idle() {
    let mut fixture = Fixture::new();
    fixture.config.backend.profile = Profile::ChromeDevtools;
    fixture.config.backend.entrypoint = Some(fixture.root.join("entrypoint"));
    fixture.config.backend.args = vec!["--isolated".into()];
    for (version, error) in [(2, "stdio profile"), (1, "format_version = 2")] {
        fixture.config.format_version = version;
        fixture.write_config();
        let before = fixture.text();
        let report = fixture.json(&["share", "fixture", "--yes"], "failed");
        assert!(report["error"].as_str().unwrap().contains(error));
        assert_eq!(report["advice"]["recommendation"], "shared_candidate");
        assert_eq!(fixture.text(), before);
    }
    fixture.config.format_version = 2;
    fixture.config.backend.profile = Profile::Stdio;
    fixture.config.backend.docker = true;
    fixture.config.backend.args = vec!["run".into(), "-i".into(), "image".into()];
    fixture.config.idle_seconds = Some(900);
    fixture.write_config();
    let before = fixture.text();
    let report = fixture.json(&["share", "fixture", "--yes"], "failed");
    assert!(report["error"].as_str().unwrap().contains("shared Docker"));
    assert_eq!(fixture.text(), before);
    assert!(!fixture.root.join("setup.lock").exists());
}

#[test]
fn isolate_restores_workers_and_removes_both_shared_only_keys() {
    let mut fixture = Fixture::new();
    fixture.config.ownership = Ownership::Shared;
    fixture.config.max_workers = 1;
    fixture.config.shared_client_roots = SharedClientRoots::Ignore;
    fixture.config.shared_recovery = SharedRecovery::Restart;
    fixture.write_config();
    let report = fixture.json(&["isolate", "local.mcp-gate.fixture", "--yes"], "complete");
    assert_eq!(report["advice"]["recommendation"], "shared_candidate");
    for (field, previous, new) in [
        ("ownership", json!("shared"), json!("session")),
        ("max_workers", json!(1), json!(4)),
        ("shared_client_roots", json!("ignore"), Value::Null),
        ("shared_recovery", json!("restart"), Value::Null),
    ] {
        assert!(
            report["changes"]
                .as_array()
                .unwrap()
                .contains(&json!({"field":field,"previous":previous,"new":new}))
        );
    }
    let config = Config::load(&fixture.record.config()).unwrap();
    assert!(config.ownership == Ownership::Session);
    assert_eq!(config.ownership_source, Some(OwnershipSource::Operator));
    assert_eq!(config.max_workers, 4);
    assert!(!fixture.text().contains("shared_client_roots"));
    assert!(!fixture.text().contains("shared_recovery"));
}

#[test]
fn requested_mode_is_unchanged_even_in_dry_run() {
    for (mode, command, workers) in [
        (Ownership::Session, "isolate", 4),
        (Ownership::Shared, "share", 1),
    ] {
        let mut fixture = Fixture::new();
        fixture.config.ownership = mode;
        fixture.config.max_workers = workers;
        fixture.write_config();
        let before = fixture.text();
        for args in [
            vec![command, "fixture"],
            vec![command, "fixture", "--dry-run"],
        ] {
            let report = fixture.json(&args, "unchanged");
            assert_eq!(report["changes"], json!([]));
        }
        assert_eq!(fixture.text(), before);
        assert!(!fixture.root.join("setup.lock").exists());
    }
}

#[test]
fn share_warns_before_saving_and_requires_yes_when_piped() {
    let fixture = Fixture::new();
    fixture.write_catalog("chrome-devtools", "select_page");
    let before = fixture.text();
    let refused = fixture.json(&["share", "fixture"], "failed");
    assert!(refused["error"].as_str().unwrap().contains("--yes"));
    assert_eq!(refused["advice"]["recommendation"], "session");
    assert!(
        refused["warning"]
            .as_str()
            .unwrap()
            .contains("session ownership")
    );
    assert_eq!(fixture.text(), before);
    assert!(!fixture.root.join("setup.lock").exists());

    let text = fixture.run(&["share", "fixture", "--dry-run"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("readOnlyHint"));
    assert!(text.find("Warning").unwrap() < text.find("ownership: \"session\"").unwrap());
    let applied = fixture.json(&["share", "fixture", "--yes"], "complete");
    assert_eq!(applied["warning"], refused["warning"]);
}

#[test]
fn missing_and_malformed_catalogs_give_unknown_advice() {
    let fixture = Fixture::new();
    fs::remove_file(&fixture.config.catalog_file).unwrap();
    for contents in [None, Some(b"invalid json".as_slice())] {
        if let Some(bytes) = contents {
            fs::write(&fixture.config.catalog_file, bytes).unwrap();
        }
        let report = fixture.json(&["share", "fixture", "--dry-run"], "preview");
        assert_eq!(report["advice"]["recommendation"], "session");
        assert_eq!(report["advice"]["signals"][0]["kind"], "no_catalog");
        assert!(report["warning"].is_string());
    }
}

#[test]
fn concurrent_setup_lock_prevents_a_mode_write() {
    let fixture = Fixture::new();
    let before = fixture.text();
    let lock = fs::File::create(fixture.root.join("setup.lock")).unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let report = fixture.json(&["share", "fixture", "--yes"], "failed");
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("operation is running")
    );
    assert_eq!(fixture.text(), before);
}

#[test]
fn share_refuses_gateways_running_a_different_binary() {
    let mut fixture = Fixture::new();
    fixture.record.binary_hash = "older-release".into();
    store::save(
        &fixture.root,
        &Registry {
            format_version: 1,
            gateways: vec![fixture.record.clone()],
        },
    )
    .unwrap();
    let before = fixture.text();
    let report = fixture.json(&["share", "fixture-id", "--yes"], "failed");
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("Run mcp-gate setup")
    );
    assert_eq!(fixture.text(), before);
    // Previews stay available so the operator can review advice first.
    fixture.json(&["share", "fixture-id", "--dry-run"], "preview");
}

#![cfg(target_os = "linux")]

use mcp_gate::manage::store;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("codex")).unwrap();
        Self(directory)
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn settings(&self) -> PathBuf {
        self.root().join("codex/config.toml")
    }

    fn command(&self, args: &[&str]) -> Command {
        let root = self.root();
        let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-gate"));
        command
            .args(args)
            .arg("--json")
            .current_dir(root)
            .env_clear()
            .env("HOME", root)
            .env("PATH", "/usr/bin:/bin")
            .env("CODEX_HOME", root.join("codex"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_RUNTIME_DIR", root.join("no-user-bus"))
            .env("FIXTURE", root);
        command
    }

    fn write_settings(&self, entry: &Value) -> Vec<u8> {
        let bytes = toml::to_string(&json!({"mcp_servers":{"fixture":entry}}))
            .unwrap()
            .into_bytes();
        fs::write(self.settings(), &bytes).unwrap();
        bytes
    }

    fn mock_systemctl(&self, script: &str) -> String {
        let bin = self.root().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let path = bin.join("systemctl");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        format!("{}:/usr/bin:/bin", bin.display())
    }
}

fn output_json(output: Output, success: bool) -> Value {
    assert_eq!(
        output.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

const SETUP: &[&str] = &["setup", "--client", "codex", "--server", "fixture", "--yes"];

#[test]
fn no_user_manager_leaves_client_settings_and_state_untouched() {
    let fixture = Fixture::new();
    let original = fixture.write_settings(&json!({
        "command":env!("CARGO_BIN_EXE_mcp-gate"), "cwd":fixture.root()
    }));
    let preview = output_json(
        fixture.command(SETUP).arg("--dry-run").output().unwrap(),
        true,
    );
    assert_eq!(preview["phase"], "preview");
    let result = output_json(fixture.command(SETUP).output().unwrap(), false);
    let error = result["error"].as_str().unwrap();
    assert!(error.contains("Automatic setup needs a reachable systemd --user manager"));
    assert!(error.contains("Client configs were not changed"));
    assert!(error.contains("Prepare a private state directory and a config"));
    assert!(error.contains(r#"mcp-gate serve --config "/absolute/path/config.toml""#));
    assert_eq!(fs::read(fixture.settings()).unwrap(), original);
    assert!(!fixture.root().join("data").exists());
    assert!(!fixture.root().join("config").exists());

    let status = output_json(fixture.command(&["status"]).output().unwrap(), true);
    assert_eq!(status["connections"][0]["name"], "fixture");
    assert_eq!(status["services"], json!([]));
    assert!(status["service_manager_issue"].is_string());
}

#[test]
fn no_user_manager_reports_unknown_state_and_existing_config_path() {
    let fixture = Fixture::new();
    let entry = json!({"url":"http://127.0.0.1:5/mcp"});
    let original = fixture.write_settings(&entry);
    let root = fixture.root().join("data/mcp-gate");
    store::prepare(&root).unwrap();
    let release = root.join("release with space");
    let record: store::Record = serde_json::from_value(json!({
        "id":"fixture","label":"local.mcp-gate.fixture","release":release,
        "binary":env!("CARGO_BIN_EXE_mcp-gate"),"binary_hash":"fixture",
        "bindings":[{
            "client":"codex","name":"fixture","source":fixture.settings(),
            "source_path":["mcp_servers","fixture"],"target":fixture.settings(),
            "path":["mcp_servers","fixture"],"toml":true,"before":{},
            "direct":{}, "after":entry
        }]
    }))
    .unwrap();
    let config = record.config();
    store::save(
        &root,
        &store::Registry {
            format_version: 1,
            gateways: vec![record],
        },
    )
    .unwrap();
    let status = output_json(fixture.command(&["status"]).output().unwrap(), true);
    let gateway = &status["gateways"][0];
    assert_eq!(
        gateway["service"],
        json!({"registered":null,"enabled":null,"active":null})
    );
    assert!(gateway["issues"].as_array().unwrap().iter().any(|issue| {
        issue
            .as_str()
            .unwrap()
            .contains("Cannot query systemd --user service state")
    }));
    let result = output_json(fixture.command(SETUP).output().unwrap(), false);
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains(&format!("mcp-gate serve --config \"{}\"", config.display()))
    );
    assert_eq!(fs::read(fixture.settings()).unwrap(), original);
    assert!(!root.join("setup.lock").exists());
}

#[test]
fn linux_state_root_uses_only_absolute_xdg_data_home() {
    let fixture = Fixture::new();
    let xdg = fixture.root().join("xdg-data");
    let fallback = fixture.root().join(".local/share");
    for (setting, base) in [
        (Some(xdg.as_os_str()), &xdg),
        (None, &fallback),
        (Some(std::ffi::OsStr::new("")), &fallback),
        (Some(std::ffi::OsStr::new("relative-data")), &fallback),
    ] {
        let root = base.join("mcp-gate");
        store::prepare(&root).unwrap();
        store::atomic(&root.join("registry.json"), b"invalid fixture registry").unwrap();
        let mut command = fixture.command(&["status"]);
        if let Some(setting) = setting {
            command.env("XDG_DATA_HOME", setting);
        } else {
            command.env_remove("XDG_DATA_HOME");
        }
        let result = output_json(command.output().unwrap(), false);
        assert_eq!(result["error"], "Invalid managed registry");
        fs::remove_file(root.join("registry.json")).unwrap();
    }
}

#[test]
fn discovery_uses_effective_units_and_adopt_records_only_literal_identity() {
    let fixture = Fixture::new();
    let path = fixture.mock_systemctl(
        r#"#!/bin/sh
shift 2
case "$1" in
  list-unit-files) printf '%s\n' 'local.mcp-gate.fixture.service enabled' 'local.mcp-gate.wrapper.service disabled' ;;
  list-units) printf '%s\n' 'local.mcp-gate.legacy.service loaded active running' ;;
  show)
    if [ "$2" = --property=Version ]; then printf '%s\n' '257'; exit 0; fi
    unit="$5"
    printf '%s\n' "Id=$unit" LoadState=loaded NeedDaemonReload=no
    printf 'FragmentPath=%s/%s\n' "$FIXTURE" "$unit"
    case "$unit" in
      local.mcp-gate.fixture.service)
        printf '%s\n' ActiveState=inactive UnitFileState=enabled ;;
      local.mcp-gate.legacy.service)
        printf '%s\n' ActiveState=active UnitFileState=disabled
        printf 'DropInPaths="%s/override directory/legacy.conf"\n' "$FIXTURE" ;;
      *) printf '%s\n' ActiveState=inactive UnitFileState=disabled ;;
    esac ;;
  *) exit 1 ;;
esac
"#,
    );
    let binary = env!("CARGO_BIN_EXE_mcp-gate");
    let config = fixture.root().join("config.toml");
    fs::write(
        &config,
        toml::to_string(&json!({
            "format_version":2,"listen":"127.0.0.1:5",
            "token_file":fixture.root().join("absent-token"),
            "catalog_file":fixture.root().join("catalog.json"),
            "state_dir":fixture.root().join("state"),
            "backend":{"profile":"stdio","command":binary,"version":"fixture"}
        }))
        .unwrap(),
    )
    .unwrap();
    let legacy_config = fixture.root().join("legacy.toml");
    let config_text = fs::read_to_string(&config).unwrap();
    fs::write(
        &legacy_config,
        config_text.replace("127.0.0.1:5", "127.0.0.1:7"),
    )
    .unwrap();
    fixture.write_settings(&json!({"url":"http://localhost:7/mcp"}));
    let root = fixture.root();
    fs::write(
        root.join("local.mcp-gate.fixture.service"),
        format!(
            "[Service]\nExecStart=\"{binary}\" serve --config \"{}\"\n",
            config.display()
        ),
    )
    .unwrap();
    for name in ["legacy", "wrapper"] {
        fs::write(
            root.join(format!("local.mcp-gate.{name}.service")),
            format!(
                "[Service]\nExecStart=/bin/sh -c '{binary} serve --config {}'\n",
                config.display()
            ),
        )
        .unwrap();
    }
    fs::create_dir(root.join("override directory")).unwrap();
    fs::write(root.join("override directory/legacy.conf"),
        format!("[Service]\nExecStart=\nExecStart=/bin/mcp-session-gateway serve --config \\\n \"{}\"\n", legacy_config.display())
    ).unwrap();

    let run = |args: &[&str]| {
        output_json(
            fixture.command(args).env("PATH", &path).output().unwrap(),
            true,
        )
    };
    let status = run(&["status"]);
    assert_eq!(status["connections"][0]["kind"], "external");
    assert_eq!(status["services"].as_array().unwrap().len(), 1);
    let service = &status["services"][0];
    assert_eq!(service["registered"], true);
    assert_eq!(service["enabled"], true);
    assert_eq!(service["active"], false);
    assert_eq!(service["state"], "not reachable");
    assert_eq!(run(&["adopt", "fixture", "--dry-run"])["phase"], "preview");
    assert!(!root.join("data").exists());
    assert_eq!(run(&["adopt", "fixture"])["phase"], "complete");
    let registry = store::load(&root.join("data/mcp-gate")).unwrap();
    assert_eq!(registry.gateways[0].config(), config);
    assert_eq!(registry.gateways[0].binary, Path::new(binary));
    assert_eq!(run(&["status"])["services"][0]["ownership"], "managed");
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_migrates_an_adopted_unit_and_keeps_its_external_connection() {
    let fixture = Fixture::new();
    let path = fixture.mock_systemctl(include_str!("support/migration_systemctl.sh"));
    let (listen, busy, server) = migration_api().await;
    let root = fixture.root();
    let data = root.join("data/mcp-gate");
    let bin = root.join("legacy/mcp-gate");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    // Identical hashes still require a migration from outside the store.
    fs::copy(env!("CARGO_BIN_EXE_mcp-gate"), &bin).unwrap();
    let unit = root.join("config/systemd/user/local.mcp-gate.historical-id.service");
    let drop_in = unit.with_extension("service.d").join("override.conf");
    fs::create_dir_all(drop_in.parent().unwrap()).unwrap();
    let config = root.join("config.toml");
    let (config_bytes, token_bytes, catalog_bytes) = migration_config(root, &config, &bin, listen);
    let base = b"[Service]\nExecStart=/bin/false\nRestart=no\n[Install]\nWantedBy=custom.target\n";
    fs::write(&unit, base).unwrap();
    let definition = format!(
        "[Service]\nExecStart=\nExecStart=\"{}\" serve --config \"{}\" --header never-print-migration-fixture\nEnvironment=FIXTURE=never-print-migration-fixture\nWorkingDirectory={}\n",
        bin.display(),
        config.display(),
        root.display()
    );
    fs::write(&drop_in, &definition).unwrap();
    let settings = fixture.write_settings(&json!({
        "url":format!("http://{listen}/mcp"),
        "http_headers":{"Authorization":"never-print-migration-fixture"}
    }));
    fs::write(root.join("manager-state"), "active\n").unwrap();
    let run = |args: &[&str], flags: &[&str], failure: Option<&str>| {
        let mut command = fixture.command(args);
        command.env("PATH", &path).args(flags);
        if let Some(failure) = failure {
            command.env("MCP_GATE_TEST_FAIL", failure);
        }
        let output = command.output().unwrap();
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("never-print-migration-fixture"));
        }
        output_json(output, failure.is_none())
    };
    assert_eq!(
        run(&["adopt", "historical-id"], &[], None)["phase"],
        "complete"
    );
    let registry_path = data.join("registry.json");
    let original_registry = fs::read(&registry_path).unwrap();
    let preview = run(SETUP, &["--dry-run", "--diff"], None);
    assert_eq!(preview["phase"], "preview");
    assert_eq!(preview["plan"]["connections"][0]["kind"], "external");
    assert_eq!(
        preview["plan"]["managed"][0]["managed_store_migration"],
        true
    );
    assert_eq!(preview["plan"]["managed"][0]["connections"], json!([]));
    assert_eq!(fs::read(&registry_path).unwrap(), original_registry);
    assert_eq!(fs::read_to_string(&drop_in).unwrap(), definition);
    assert!(!data.join("binaries").exists());
    assert!(!root.join("manager-actions").exists());

    let pending = run(SETUP, &[], None);
    assert!(
        pending["updates"][0]
            .as_str()
            .unwrap()
            .contains("migration pending")
    );
    assert_eq!(fs::read(&registry_path).unwrap(), original_registry);
    assert!(!root.join("manager-actions").exists());
    busy.store(false, std::sync::atomic::Ordering::Relaxed);
    #[cfg(feature = "test-backend")]
    {
        let rolled_back = run(SETUP, &[], Some("upgrade-registered"));
        assert!(
            rolled_back["error"]
                .as_str()
                .unwrap()
                .contains("previous settings/service restored")
        );
        assert_eq!(fs::read(&registry_path).unwrap(), original_registry);
        assert_eq!(fs::read_to_string(&drop_in).unwrap(), definition);
    }

    let result = run(SETUP, &[], None);
    assert!(
        result["updates"][0]
            .as_str()
            .unwrap()
            .contains("migrated to the managed store")
    );
    assert!(
        result["updates"][0]
            .as_str()
            .unwrap()
            .contains("no saved bindings")
    );
    assert_eq!(result["restart_clients"], json!([]));
    let record = store::load(&data).unwrap().gateways.remove(0);
    let hash = mcp_gate::catalog::digest(&bin).unwrap();
    let target = data.join(format!(
        "binaries/{}-{hash}/mcp-gate",
        env!("CARGO_PKG_VERSION")
    ));
    assert_eq!(record.binary_hash, hash);
    assert_eq!(record.binary, target);
    assert!(record.bindings.is_empty());
    assert_eq!(
        fs::read_to_string(&drop_in).unwrap(),
        definition.replace(bin.to_str().unwrap(), target.to_str().unwrap())
    );
    assert_eq!(fs::read(&unit).unwrap(), base);
    assert_eq!(fs::read(&config).unwrap(), config_bytes);
    assert_eq!(fs::read(root.join("token")).unwrap(), token_bytes);
    assert_eq!(fs::read(root.join("catalog.json")).unwrap(), catalog_bytes);
    assert_eq!(fs::read(fixture.settings()).unwrap(), settings);
    assert_eq!(
        run(&["status"], &[], None)["connections"][0]["kind"],
        "external"
    );
    let actions_path = root.join("manager-actions");
    let actions = fs::read_to_string(&actions_path).unwrap();
    assert!(actions.ends_with("stop\ndaemon-reload\nreset-failed\nstart\n"));
    assert_eq!(
        run(SETUP, &[], None)["plan"]["managed"][0]["managed_store_migration"],
        false
    );
    assert_eq!(fs::read_to_string(&actions_path).unwrap(), actions);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

async fn migration_api() -> (
    std::net::SocketAddr,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    tokio::task::JoinHandle<()>,
) {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen = listener.local_addr().unwrap();
    let busy = Arc::new(AtomicBool::new(true));
    let maintenance = busy.clone();
    let api = Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"service":"mcp-gate","workers":0})) }),
        )
        .route(
            "/admin/maintenance",
            post(move || async move {
                if maintenance.load(Ordering::Relaxed) {
                    axum::http::StatusCode::CONFLICT
                } else {
                    axum::http::StatusCode::OK
                }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    (listen, busy, server)
}

fn migration_config(
    root: &Path,
    config: &Path,
    binary: &Path,
    listen: std::net::SocketAddr,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let token = root.join("token");
    let token_bytes = "ab".repeat(32).into_bytes();
    fs::write(&token, &token_bytes).unwrap();
    mcp_gate::platform::private_permissions(&token).unwrap();
    let state = root.join("state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join("gateway.lock"), b"").unwrap();
    let catalog = root.join("catalog.json");
    let catalog_bytes = b"retained legacy catalog".to_vec();
    fs::write(&catalog, &catalog_bytes).unwrap();
    let config_bytes = toml::to_string(&json!({
        "format_version":2,"listen":listen.to_string(),
        "token_file":token,"catalog_file":catalog,"state_dir":state,
        "backend":{"profile":"stdio","command":binary,"version":"fixture",
            "working_directory":root, "env":{"FIXTURE":"never-print-migration-fixture"}}
    }))
    .unwrap()
    .into_bytes();
    fs::write(config, &config_bytes).unwrap();
    (config_bytes, token_bytes, catalog_bytes)
}

#[test]
fn adopted_gateway_dry_run_is_read_only_without_a_reachable_gateway() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let data = root.join("data/mcp-gate");
    let bin = Path::new(env!("CARGO_BIN_EXE_mcp-gate"));
    let config = root.join("config.toml");
    let address = "127.0.0.1:5".parse().unwrap();
    migration_config(root, &config, bin, address);
    fs::remove_file(root.join("token")).unwrap();
    fs::write(root.join("manager-state"), "inactive\n").unwrap();
    let unit = root.join("config/systemd/user/local.mcp-gate.historical-id.service");
    let drop_in = unit.with_extension("service.d").join("override.conf");
    fs::create_dir_all(drop_in.parent().unwrap()).unwrap();
    fs::write(&unit, "[Service]\n").unwrap();
    let before = format!(
        "[Service]\nExecStart=\"{}\" serve --config \"{}\"\nEnvironment=FIXTURE=never-print-migration-fixture\n",
        bin.display(),
        config.display()
    );
    fs::write(&drop_in, &before).unwrap();
    let settings = fixture.write_settings(&json!({"url":format!("http://{address}/mcp")}));
    let path = fixture.mock_systemctl(include_str!("support/migration_systemctl.sh"));
    let run = |args: &[&str]| {
        let output = fixture.command(args).env("PATH", &path).output().unwrap();
        assert!(!String::from_utf8_lossy(&output.stdout).contains("never-print-migration-fixture"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("never-print-migration-fixture"));
        output_json(output, true)
    };
    assert_eq!(run(&["adopt", "historical-id"])["phase"], "complete");
    let registry = data.join("registry.json");
    let registry_bytes = fs::read(&registry).unwrap();
    let dry_run = run(&[
        "setup",
        "--client",
        "codex",
        "--server",
        "fixture",
        "--dry-run",
        "--diff",
    ]);
    assert_eq!(
        dry_run["plan"]["managed"][0]["managed_store_migration"],
        true
    );
    assert_eq!(dry_run["plan"]["managed"][0]["connections"], json!([]));
    assert_eq!(dry_run["plan"]["connections"][0]["kind"], "external");
    assert_eq!(fs::read_to_string(drop_in).unwrap(), before);
    assert_eq!(fs::read(registry).unwrap(), registry_bytes);
    assert_eq!(fs::read(fixture.settings()).unwrap(), settings);
    assert!(!root.join("manager-actions").exists());
    assert!(!data.join("binaries").exists());
}

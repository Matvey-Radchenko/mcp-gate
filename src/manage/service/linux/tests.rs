use super::*;
use std::{
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    process::{ExitStatus, Output},
};

fn record() -> Record {
    Record {
        removing: false,
        id: "fixture".into(),
        release: PathBuf::from(r#"/state with space/"quoted"\$HOME%"#),
        label: "local.mcp-gate.fixture".into(),
        binary_hash: "unused".into(),
        binary: PathBuf::from(r#"/binary path/"quoted"\$HOME%/mcp-gate"#),
        bindings: vec![],
    }
}

#[test]
fn renders_literal_systemd_paths_and_service_policy() {
    let record = record();
    let unit = unit_file::render(&record).unwrap();
    assert_eq!(
        unit,
        concat!(
            "[Service]\nType=exec\n",
            r#"ExecStart=":/binary path/\"quoted\"\\$HOME%%/mcp-gate" serve --config "/state with space/\"quoted\"\\$HOME%%/config.toml""#,
            "\nRestart=always\nRestartSec=10s\nTimeoutStopSec=20s\nUMask=0077\n",
            "StandardOutput=journal\nStandardError=journal\nKillMode=mixed\n\n",
            "[Install]\nWantedBy=default.target\n"
        )
    );
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(unit_name(&record));
    write_new(&path, unit.as_bytes(), &record.label).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(unit_file::identity(&path, "").unwrap().matches(&record));
    assert_eq!(
        restart_command(&record),
        "systemctl --user restart 'local.mcp-gate.fixture.service'"
    );
}

#[test]
fn treats_unit_files_symlinks_and_specific_manager_errors_as_collisions() {
    let root = tempfile::tempdir().unwrap();
    let record = record();
    let path = root.path().join(unit_name(&record));
    fs::write(&path, "existing unit").unwrap();
    assert!(super::super::label_in_use(
        &write_new(&path, b"test", &record.label).unwrap_err()
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "existing unit");
    fs::remove_file(&path).unwrap();
    let target = root.path().join("missing");
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(super::super::label_in_use(
        &write_new(&path, b"test", &record.label).unwrap_err()
    ));
    assert_eq!(fs::read_link(&path).unwrap(), target);
    for message in [
        "Failed to enable unit: File /fixture already exists.",
        "Failed to enable unit: File exists",
        "Failed to enable unit: Unit local.mcp-gate.fixture.service is masked",
        "Failed to enable unit: Unit local.mcp-gate.fixture.service is transient or generated",
    ] {
        assert!(
            super::super::label_in_use(&anyhow::anyhow!(message)),
            "{message}"
        );
    }
    for message in [
        "Failed to enable unit: Permission denied",
        "Failed to connect to bus: Address already in use",
        "Disk quota already exists for this service",
        "Failed to enable unit: Invalid configuration in unmasked unit",
    ] {
        assert!(
            !super::super::label_in_use(&anyhow::anyhow!(message)),
            "{message}"
        );
    }
}

fn response(code: i32, stderr: &str) -> Output {
    Output {
        status: ExitStatus::from_raw(code << 8),
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[test]
fn enable_resets_failure_and_start_limit_before_starting() {
    let unit = unit_name(&record());
    for detail in [None, Some("not loaded."), Some("not found.")] {
        let mut commands = Vec::new();
        manager::enable_with(&unit, |args| {
            commands.push(args.join(" "));
            Ok(match (args[0], detail) {
                ("reset-failed", Some(detail)) => response(
                    1,
                    &format!("Failed to reset failed state of unit {unit}: Unit {unit} {detail}"),
                ),
                _ => response(0, ""),
            })
        })
        .unwrap();
        assert_eq!(
            commands,
            [
                format!("reset-failed {unit}"),
                format!("enable --now {unit}")
            ]
        );
    }
}

#[test]
fn reset_failure_stops_enable_and_hides_manager_stderr() {
    let error = manager::enable_with(&unit_name(&record()), |args| {
        assert_eq!(args[0], "reset-failed");
        Ok(response(1, "arbitrary unit text"))
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("reset-failed failed"));
    assert!(!error.contains("arbitrary unit text"));
}

#[test]
fn startup_state_distinguishes_first_activation_from_exit_and_restart() {
    for (state, failed) in [
        (
            "ActiveState=inactive SubState=dead MainPID=0 Result=success ExecMainCode=0",
            false,
        ),
        (
            "ActiveState=activating SubState=start MainPID=0 Result=success ExecMainCode=0",
            false,
        ),
        (
            "ActiveState=active SubState=running MainPID=42 Result=success NRestarts=0",
            false,
        ),
        ("ActiveState=failed", true),
        (
            "ActiveState=activating SubState=auto-restart MainPID=0 NRestarts=0",
            true,
        ),
        (
            "ActiveState=active SubState=running MainPID=42 NRestarts=1",
            true,
        ),
        (
            "ActiveState=inactive MainPID=0 Result=exit-code ExecMainStatus=1",
            true,
        ),
        (
            "ActiveState=deactivating MainPID=0 Result=success ExecMainCode=1 ExecMainStatus=0",
            true,
        ),
    ] {
        let fields = state
            .split_whitespace()
            .map(|field| field.split_once('=').unwrap())
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        assert_eq!(startup::failed_start(&fields), failed, "{state}");
    }
}

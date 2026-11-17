use super::*;
use std::os::unix::fs::PermissionsExt;

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

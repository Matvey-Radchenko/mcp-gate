use super::*;
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

#[test]
fn registration_journals_retry_before_enabling_and_removes_only_its_unit() {
    // A subprocess gets an isolated XDG root and systemctl fixture without
    // changing process-wide environment used by parallel tests.
    if let Some(root) = std::env::var_os("MCP_GATE_SYSTEMD_FIXTURE") {
        verify_registration(&PathBuf::from(root));
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let bin = root.join("bin");
    let config = root.join("config");
    let units = config.join("systemd/user");
    fs::create_dir(&bin).unwrap();
    fs::create_dir_all(&units).unwrap();
    let foreign = units.join("local.mcp-gate.server.service");
    fs::write(&foreign, b"foreign unit").unwrap();
    let systemctl = bin.join("systemctl");
    fs::write(&systemctl, SYSTEMCTL).unwrap();
    fs::set_permissions(systemctl, fs::Permissions::from_mode(0o700)).unwrap();

    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "manage::setup::tests::registration_journals_retry_before_enabling_and_removes_only_its_unit"])
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("MCP_GATE_SYSTEMD_FIXTURE", root)
        .env("FIXTURE", root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_DATA_HOME", root.join("data"))
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    for marker in [
        "enable-saw-journal",
        "stop-was-called",
        "disable-was-called",
    ] {
        assert!(root.join(marker).is_file(), "{marker}");
    }
    assert_eq!(fs::read(foreign).unwrap(), b"foreign unit");
    assert_eq!(fs::read_dir(units).unwrap().count(), 1);
}

fn verify_registration(root: &Path) {
    let record = store::Record {
        removing: false,
        id: "fixture".into(),
        release: root.join("release"),
        label: "local.mcp-gate.server".into(),
        binary_hash: "fixture".into(),
        binary: root.join("mcp-gate"),
        bindings: Vec::new(),
    };
    let state = store::root().unwrap();
    store::prepare(&state).unwrap();
    let journal_path = state.join("operations/setup.json");
    let mut journal = Journal {
        format_version: 1,
        phase: "preparing".into(),
        services: vec![record.clone()],
        restart: vec![],
        changes: vec![],
    };
    journal.save(&journal_path).unwrap();

    let registered = register_free_label(record.clone(), &mut journal, &journal_path).unwrap();
    assert_eq!(registered.label, "local.mcp-gate.server-3");
    assert!(!service::linux::rollback_needed(&record).unwrap());
    assert!(service::linux::rollback_needed(&registered).unwrap());
    assert!(service::installed(&registered).unwrap());
    service::unregister(&registered).unwrap();
    assert!(!service::installed(&registered).unwrap());
}

const SYSTEMCTL: &str = r#"#!/bin/sh
shift 2
case "$1" in
  list-unit-files|list-units|daemon-reload) ;;
  show)
    if [ "$2" = --property=Version ]; then printf '%s\n' '257'; exit 0; fi
    unit="$5"
    fragment="$XDG_CONFIG_HOME/systemd/user/$unit"
    printf '%s\n' "Id=$unit" MainPID=0 ControlPID=0 NeedDaemonReload=no
    case "$unit" in
      local.mcp-gate.server-2.service)
        printf '%s\n' LoadState=loaded ActiveState=active UnitFileState=transient ;;
      *)
        printf '%s\n' ActiveState=inactive
        if [ -f "$fragment" ]; then
          printf '%s\n' LoadState=loaded "FragmentPath=$fragment"
        else
          printf '%s\n' LoadState=not-found
        fi ;;
    esac ;;
  reset-failed)
    [ "$2" = local.mcp-gate.server-3.service ] || exit 1
    touch "$FIXTURE/reset-was-called" ;;
  enable)
    [ "$2" = --now ] && [ "$3" = local.mcp-gate.server-3.service ] || exit 1
    [ -f "$FIXTURE/reset-was-called" ] || exit 1
    grep -q '"label": "local.mcp-gate.server-3"' "$XDG_DATA_HOME"/mcp-gate/operations/*.json || exit 1
    touch "$FIXTURE/enable-saw-journal" ;;
  stop|disable)
    [ "$2" = local.mcp-gate.server-3.service ] || exit 1
    touch "$FIXTURE/$1-was-called" ;;
  *) exit 1 ;;
esac
"#;

#![cfg(feature = "test-backend")]
use mcp_gate::{
    clients::{self, Candidate, Client},
    manage::{launch, runtime},
};
use serde_json::json;
use std::{fs, path::Path};

fn candidate(root: &Path) -> Candidate {
    let settings = root.join(".codex");
    fs::create_dir(&settings).unwrap();
    let config = json!({"mcp_servers":{"fixture":{
        "command":"mcp-gate-lookup-fixture","cwd":root,"args":[]
    }}});
    fs::write(
        settings.join("config.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    clients::discover(root, root, &[Client::Codex], false)
        .unwrap()
        .remove(0)
}

#[tokio::test]
async fn setup_resolves_the_backend_path_and_retains_its_private_file_reference() {
    let dir = tempfile::tempdir().unwrap();
    let root = mcp_gate::platform::project_path(dir.path()).unwrap();
    let bin = root.join("bin space Юникод");
    fs::create_dir(&bin).unwrap();
    let expected = bin.join(if cfg!(windows) {
        "mcp-gate-lookup-fixture.exe"
    } else {
        "mcp-gate-lookup-fixture"
    });
    fs::copy(env!("CARGO_BIN_EXE_mock-backend"), &expected).unwrap();
    mcp_gate::platform::executable(&expected).unwrap();
    let path_file = root.join("private-path");
    fs::write(&path_file, "bin space Юникод\n").unwrap();
    mcp_gate::platform::private_permissions(&path_file).unwrap();
    let mut c = candidate(&root);
    let path_key = if cfg!(windows) { "Path" } else { "PATH" };
    c.env.insert(path_key.into(), "not-the-backend-path".into());
    c.env_files.insert(path_key.into(), path_file.clone());
    let snapshot = launch::environment(&c).unwrap();
    assert!(!snapshot.keys().any(|k| k.eq_ignore_ascii_case("PATH")));
    assert_eq!(launch::executable(&c, &snapshot).unwrap(), expected);
    let managed = root.join("managed");
    mcp_gate::manage::store::prepare(&managed).unwrap();
    let record = runtime::prepare(&managed, &c).await.unwrap();
    let config = mcp_gate::config::Config::load(&record.config()).unwrap();
    assert_eq!(config.backend.command, expected);
    assert_eq!(config.backend.env_files[path_key], path_file);
    assert!(!config.backend.env.contains_key("PATH"));
    assert_eq!(
        config.backend.working_directory.as_deref(),
        Some(root.as_path())
    );
    c.command[0] = "missing-fixture-executable-459d".into();
    assert!(launch::executable(&c, &snapshot).is_err());
}

#[cfg(windows)]
#[test]
fn windows_lookup_uses_explicit_environment_case_and_native_extension_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = mcp_gate::platform::project_path(dir.path()).unwrap();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    for extension in ["", ".exe", ".cmd"] {
        fs::write(
            bin.join(format!("mcp-gate-lookup-fixture{extension}")),
            b"fixture",
        )
        .unwrap();
    }
    let mut c = candidate(&root);
    c.env.insert("Path".into(), "bin".into());
    c.env.insert("PathExt".into(), ".CMD;.EXE".into());
    c.env
        .insert("systemroot".into(), "explicit-fixture-value".into());
    let env = launch::environment(&c).unwrap();
    assert_eq!(env["SYSTEMROOT"], "explicit-fixture-value");
    assert_eq!(
        env.keys()
            .filter(|k| k.eq_ignore_ascii_case("PATH"))
            .count(),
        1
    );
    assert_eq!(
        launch::executable(&c, &env).unwrap(),
        bin.join("mcp-gate-lookup-fixture.cmd")
    );
}

#[cfg(unix)]
#[test]
fn non_executable_path_entries_do_not_shadow_a_later_executable() {
    let dir = tempfile::tempdir().unwrap();
    let root = mcp_gate::platform::project_path(dir.path()).unwrap();
    for directory in ["first", "second"] {
        fs::create_dir(root.join(directory)).unwrap();
        fs::write(
            root.join(directory).join("mcp-gate-lookup-fixture"),
            b"#!/bin/sh\nexit 0\n",
        )
        .unwrap();
    }
    let expected = root.join("second/mcp-gate-lookup-fixture");
    mcp_gate::platform::executable(&expected).unwrap();
    let mut c = candidate(&root);
    c.env.insert("PATH".into(), "first:second".into());
    assert_eq!(
        launch::executable(&c, &launch::environment(&c).unwrap()).unwrap(),
        expected
    );
}

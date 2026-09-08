#![cfg(feature = "test-backend")]
use serde_json::json;
use std::{fs, path::Path};

async fn launch(command: &Path, directory: &Path, args: Vec<String>) {
    let output = directory.join("context.json");
    let mut config: mcp_gate::config::Config = serde_json::from_value(json!({
        "format_version":2,"ownership":"session","listen":"127.0.0.1:1",
        "token_file":directory.join("token"),"catalog_file":directory.join("catalog.json"),"state_dir":directory.join("state"),
        "backend":{"profile":"stdio","command":command,"args":args,"version":"pending",
            "working_directory":directory,"env":{"MOCK_CONTEXT_FILE":output,"MOCK_CONTEXT_VALUE":"spaces Юникод\n"}}
    })).unwrap();
    config.validate().unwrap();
    mcp_gate::install::discover_and_pin(&mut config)
        .await
        .unwrap();
    let actual: serde_json::Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(actual["args"], json!(args));
    assert_eq!(actual["cwd"], json!(directory));
    assert_eq!(actual["value"], "spaces Юникод\n");
    assert_eq!(actual["relative"], "fixture-relative-path");
    assert_eq!(config.backend.version, "mock-1");
}
#[tokio::test]
async fn backend_launch_retains_cwd_environment_and_argument_boundaries() {
    let dir = tempfile::Builder::new()
        .prefix("context space Юникод ")
        .tempdir()
        .unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    fs::write(cwd.join("relative input.txt"), "fixture-relative-path").unwrap();
    let args = vec![
        "two words".into(),
        "Юникод".into(),
        "--literal=$();&".into(),
        "".into(),
    ];
    launch(Path::new(env!("CARGO_BIN_EXE_mock-backend")), &cwd, args).await;
    assert!(mcp_gate::manage::runtime::resolve("missing-fixture-executable-459d", &cwd).is_err());
}
#[cfg(windows)]
#[tokio::test]
async fn windows_cmd_wrapper_retains_spaces_and_unicode() {
    let dir = tempfile::Builder::new()
        .prefix("cmd space Юникод ")
        .tempdir()
        .unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    fs::write(cwd.join("relative input.txt"), "fixture-relative-path").unwrap();
    let wrapper = cwd.join("wrapper.cmd");
    fs::write(
        &wrapper,
        format!(
            "@echo off\r\n\"{}\" %*\r\n",
            env!("CARGO_BIN_EXE_mock-backend")
        ),
    )
    .unwrap();
    launch(&wrapper, &cwd, vec!["two words".into(), "Юникод".into()]).await;
}
#[cfg(windows)]
#[test]
fn windows_private_acl_rejects_an_added_public_reader() {
    let dir = tempfile::Builder::new()
        .prefix("ACL space Юникод ")
        .tempdir()
        .unwrap();
    let file = dir.path().join("credential");
    mcp_gate::manage::store::atomic(&file, b"fixture-only").unwrap();
    mcp_gate::platform::validate_private(&file).unwrap();
    let output = std::process::Command::new("icacls.exe")
        .arg(&file)
        .args(["/grant", "*S-1-1-0:R"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(mcp_gate::platform::validate_private(&file).is_err());
    mcp_gate::platform::private_permissions(&file).unwrap();
    mcp_gate::platform::validate_private(&file).unwrap();
    assert_eq!(fs::read(file).unwrap(), b"fixture-only");
}

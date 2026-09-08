#![cfg(feature = "test-backend")]
use serde_json::json;
use std::{fs, path::Path, process::Command};

async fn discover(command: &Path, args: Vec<String>, root: &Path, cwd: &Path) {
    let output = root.join("context.json");
    let mut c: mcp_gate::config::Config = serde_json::from_value(json!({
        "format_version":2,"ownership":"session","listen":"127.0.0.1:1",
        "token_file":root.join("token"),"catalog_file":root.join("catalog.json"),"state_dir":root.join("state"),
        "startup_timeout_seconds":60,
        "backend":{"profile":"stdio","command":command,"args":args,"version":"pending","working_directory":cwd,
            "env":{"MOCK_CONTEXT_FILE":output,"MOCK_CONTEXT_VALUE":"literal Юникод\n","MOCK_EXECUTABLE":env!("CARGO_BIN_EXE_mock-backend"),
                "UV_CACHE_DIR":root.join("uv-cache"),"UV_NO_PROGRESS":"1","npm_config_update_notifier":"false"}}
    })).unwrap();
    c.validate().unwrap();
    mcp_gate::install::discover_and_pin(&mut c).await.unwrap();
    let context: serde_json::Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(
        context["args"],
        json!(["two words", "Юникод", "--literal=$();&"])
    );
    assert_eq!(
        Path::new(context["cwd"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        cwd.canonicalize().unwrap()
    );
    assert_eq!(context["relative"], "relative fixture");
    assert_eq!(context["value"], "literal Юникод\n");
}
fn arguments() -> Vec<String> {
    vec![
        "two words".into(),
        "Юникод".into(),
        "--literal=$();&".into(),
    ]
}

#[tokio::test]
#[ignore = "Requires NPX_BINARY; installs only a local fixture package into a temporary offline npm cache"]
async fn npx_preserves_original_stdio_launch_context() {
    let dir = tempfile::Builder::new()
        .prefix("npx space Юникод ")
        .tempdir()
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    let package = root.join("package");
    fs::create_dir(&package).unwrap();
    fs::write(package.join("package.json"),json!({"name":"mcp-gate-wrapper-fixture","version":"1.0.0","bin":{"mcp-gate-wrapper-fixture":"cli.cjs"}}).to_string()).unwrap();
    let script = package.join("cli.cjs");
    fs::write(&script,"#!/usr/bin/env node\nconst {spawnSync}=require('node:child_process');\nconst result=spawnSync(process.env.MOCK_EXECUTABLE,process.argv.slice(2),{stdio:'inherit'});\nprocess.exit(result.status ?? 1);\n").unwrap();
    mcp_gate::platform::executable(&script).unwrap();
    let cwd = root.join("working directory");
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join("relative input.txt"), "relative fixture").unwrap();
    let binary = std::env::var("NPX_BINARY").unwrap();
    let mut args = vec![
        "--offline".into(),
        "--yes".into(),
        "--cache".into(),
        root.join("npm-cache").to_string_lossy().into_owned(),
        "--package".into(),
        package.to_string_lossy().into_owned(),
        "mcp-gate-wrapper-fixture".into(),
    ];
    args.extend(arguments());
    discover(Path::new(&binary), args, &root, &cwd).await;
}

#[tokio::test]
#[ignore = "Requires UVX_BINARY and PYTHON_BINARY; installs a local dependency-free wheel offline"]
async fn uvx_preserves_original_stdio_launch_context() {
    let dir = tempfile::Builder::new()
        .prefix("uvx space Юникод ")
        .tempdir()
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    let wheel = root.join("mcp_gate_wrapper_fixture-1.0.0-py3-none-any.whl");
    let python = std::env::var("PYTHON_BINARY").unwrap();
    let output = Command::new(&python).args(["-c",r#"
import sys, zipfile
files = {
 'gateway_fixture.py': 'import os, sys, subprocess\ndef main():\n    sys.exit(subprocess.call([os.environ["MOCK_EXECUTABLE"], *sys.argv[1:]]))\n',
 'mcp_gate_wrapper_fixture-1.0.0.dist-info/METADATA': 'Metadata-Version: 2.1\nName: mcp-gate-wrapper-fixture\nVersion: 1.0.0\n',
 'mcp_gate_wrapper_fixture-1.0.0.dist-info/WHEEL': 'Wheel-Version: 1.0\nGenerator: mcp-gate-fixture\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
 'mcp_gate_wrapper_fixture-1.0.0.dist-info/entry_points.txt': '[console_scripts]\nmcp-gate-wrapper-fixture = gateway_fixture:main\n',
}
record = 'mcp_gate_wrapper_fixture-1.0.0.dist-info/RECORD'
files[record] = ''.join(name + ',,\n' for name in [*files, record])
with zipfile.ZipFile(sys.argv[1], 'w') as archive:
    for name, content in files.items(): archive.writestr(name, content)
"#]).arg(&wheel).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cwd = root.join("working directory");
    fs::create_dir(&cwd).unwrap();
    fs::write(cwd.join("relative input.txt"), "relative fixture").unwrap();
    let binary = std::env::var("UVX_BINARY").unwrap();
    let mut args = vec![
        "--offline".into(),
        "--no-managed-python".into(),
        "--python".into(),
        python,
        "--from".into(),
        wheel.to_string_lossy().into_owned(),
        "mcp-gate-wrapper-fixture".into(),
    ];
    args.extend(arguments());
    discover(Path::new(&binary), args, &root, &cwd).await;
}

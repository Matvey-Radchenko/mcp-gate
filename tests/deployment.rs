#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared fixtures are exercised by different suites"
)]
mod support;
use mcp_gate::{catalog::Catalog, config::Config};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{fs, process::Command};

#[tokio::test]
async fn configured_release_is_separate_private_and_never_overwritten() {
    let mut h = support::Harness::generic("shared", 4, 10, 5).await;
    let dir = tempfile::tempdir().unwrap();
    let prefix = dir.path().join("release");
    let before = fs::read(&h.config).unwrap();
    let mut source = Config::load(&h.config).unwrap();
    let secret = dir.path().join("legacy-secret");
    fs::write(&secret, "fixture-value").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).unwrap();
    source
        .backend
        .env_files
        .insert("FIXTURE_CREDENTIAL".into(), secret.clone());
    let artifact_root = dir.path().join("node_modules");
    fs::create_dir(&artifact_root).unwrap();
    let fixture = if cfg!(windows) {
        "mock-backend.exe"
    } else {
        "mock-backend"
    };
    fs::copy(&source.backend.command, artifact_root.join(fixture)).unwrap();
    source.backend.command = artifact_root.join(fixture);
    let template = dir.path().join("template.toml");
    fs::write(&template, toml::to_string(&source).unwrap()).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_mcp-gate"))
            .args(["stage", "--config"])
            .arg(&template)
            .arg("--prefix")
            .arg(&prefix)
            .arg("--artifact-root")
            .arg(&artifact_root)
            .args(["--label", "local.fixture"])
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(&h.config).unwrap(), before);
    let staged = Config::load(&prefix.join("config.toml")).unwrap();
    assert_eq!(source.token().unwrap(), staged.token().unwrap());
    assert_eq!(staged.state_dir, prefix.join("state"));
    assert_eq!(
        staged.backend.command,
        prefix.join("runtime/node_modules").join(fixture)
    );
    let copied = &staged.backend.env_files["FIXTURE_CREDENTIAL"];
    assert_eq!(fs::read(copied).unwrap(), fs::read(&secret).unwrap());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(copied).unwrap().permissions().mode() & 0o777,
        0o600
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert!(Catalog::load(&staged.catalog_file, &staged.backend).is_ok());
    assert!(prefix.join("local.fixture.plist").exists());
    let unchanged = fs::read(&staged.catalog_file).unwrap();
    assert!(!run().status.success());
    assert_eq!(fs::read(&staged.catalog_file).unwrap(), unchanged);
    h.stop();
}

use mcp_gate::{
    backend::{Backend, Profile},
    catalog::{Catalog, fingerprint, validate_capabilities},
    config::Config,
};
use rmcp::model::*;
use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn backend() -> Backend {
    Backend {
        docker: false,
        working_directory: None,
        command_args: Vec::new(),
        directory_env: Vec::new(),
        working_directory_env: None,
        profile: Profile::Stdio,
        command: std::env::current_exe().unwrap(),
        entrypoint: None,
        version: "fixture".into(),
        args: vec![],
        env: BTreeMap::new(),
        env_files: BTreeMap::new(),
        inherit_env: vec![],
    }
}

#[test]
fn interpreter_arguments_are_ordered_and_optional_fields_preserve_catalogs() {
    let mut b = backend();
    let value = serde_json::to_value(&b).unwrap();
    assert!(value.get("command_args").is_none());
    assert!(value.get("directory_env").is_none());
    let before = fingerprint(&b).unwrap();
    b.entrypoint = Some("/tmp/backend.jar".into());
    b.command_args = vec!["--enable-native-access=ALL-UNNAMED".into(), "-jar".into()];
    b.args = vec!["serve".into(), "--transport".into(), "stdio".into()];
    let cmd = b.command().unwrap();
    let args: Vec<_> = cmd
        .as_std()
        .get_args()
        .map(|s| s.to_str().unwrap())
        .collect();
    assert_eq!(
        args,
        [
            "--enable-native-access=ALL-UNNAMED",
            "-jar",
            "/tmp/backend.jar",
            "serve",
            "--transport",
            "stdio"
        ]
    );
    b.entrypoint = Some(std::env::current_exe().unwrap());
    assert_ne!(before, fingerprint(&b).unwrap());
}

#[test]
fn worker_directories_are_private_unique_and_reject_symlink_parents() {
    let state = tempfile::tempdir().unwrap();
    let mut b = backend();
    b.directory_env = vec!["FIXTURE_OUTPUT_DIR".into()];
    b.working_directory_env = Some("FIXTURE_OUTPUT_DIR".into());
    b.validate().unwrap();
    let paths: Vec<_> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let mut cmd = b.command().unwrap();
                    b.configure_directories(&mut cmd, state.path()).unwrap();
                    let (_, value) = cmd
                        .as_std()
                        .get_envs()
                        .find(|(k, _)| *k == "FIXTURE_OUTPUT_DIR")
                        .unwrap();
                    let path = std::path::PathBuf::from(value.unwrap());
                    assert_eq!(cmd.as_std().get_current_dir(), Some(path.as_path()));
                    path
                })
            })
            .collect();
        jobs.into_iter().map(|job| job.join().unwrap()).collect()
    });
    let unique: std::collections::BTreeSet<_> = paths.iter().collect();
    assert_eq!(unique.len(), 8);
    for path in paths {
        assert!(path.starts_with(state.path()));
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    #[cfg(unix)]
    {
        let linked = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(state.path(), linked.path().join("worker-data")).unwrap();
        assert!(
            b.configure_directories(&mut b.command().unwrap(), linked.path())
                .is_err()
        );
    }
    for name in ["HOME", "../escape", "FIXTURE_OUTPUT_DIR"] {
        b.directory_env.push(name.into());
        assert!(b.validate().is_err());
        b.directory_env.pop();
    }
}
fn config() -> Config {
    serde_json::from_value(serde_json::json!({
        "format_version": 2, "ownership": "shared", "listen": "127.0.0.1:12345",
        "token_file": std::env::temp_dir().join("fixture-token"),
        "catalog_file": std::env::temp_dir().join("fixture-catalog"),
        "state_dir": std::env::temp_dir().join("fixture-state"), "max_workers": 1,
        "backend": backend()
    }))
    .unwrap()
}

#[test]
fn strict_version_profile_and_capacity_validation() {
    let original = config();
    original.validate().unwrap();
    for field in ["format_version", "max_workers", "queue_timeout_seconds"] {
        let mut value = toml::Value::try_from(&original).unwrap();
        value[field] = 0.into();
        assert!(value.try_into::<Config>().unwrap().validate().is_err());
    }
    let text = toml::to_string(&original).unwrap();
    assert!(toml::from_str::<Config>(&format!("typo = true\n{text}")).is_err());
    let mut chrome = backend();
    chrome.profile = Profile::ChromeDevtools;
    assert!(chrome.validate().is_err());
    chrome.entrypoint = Some(std::env::temp_dir().join("chrome-entry"));
    chrome.args = vec!["--isolated".into()];
    chrome.validate().unwrap();
    chrome
        .args
        .push("--browserUrl=http://localhost:9222".into());
    assert!(chrome.validate().is_err());
}

#[test]
fn private_environment_references_and_invocation_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("fake-secret");
    std::fs::write(&secret, "test-only-value\n").unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut b = backend();
    b.env_files.insert("TEST_CREDENTIAL".into(), secret.clone());
    assert!(b.command().is_err());
    mcp_gate::platform::private_permissions(&secret).unwrap();
    let cmd = b.command().unwrap();
    assert!(
        cmd.as_std()
            .get_envs()
            .any(|(k, v)| k == "TEST_CREDENTIAL" && v.unwrap() == "test-only-value")
    );
    let before = fingerprint(&b).unwrap();
    std::fs::write(&secret, "rotated-fake-value").unwrap();
    assert_eq!(before, fingerprint(&b).unwrap());
    b.args.push("changed-invocation".into());
    assert_ne!(before, fingerprint(&b).unwrap());
}

#[test]
fn catalog_rejects_unsupported_capabilities_and_changed_invocation() {
    let mut info = ServerInfo::default();
    info.protocol_version = ProtocolVersion::V_2025_11_25;
    info.server_info = Implementation::new("fixture", "fixture");
    info.capabilities = ServerCapabilities::builder().enable_tools().build();
    validate_capabilities(&info).unwrap();
    let mut unsupported = info.clone();
    unsupported.capabilities = ServerCapabilities::builder()
        .enable_tools()
        .enable_completions()
        .build();
    assert!(validate_capabilities(&unsupported).is_err());
    let mut dynamic = info.clone();
    dynamic.capabilities.tools.as_mut().unwrap().list_changed = Some(true);
    validate_capabilities(&dynamic).unwrap(); // Changes are checked against the pinned catalog at runtime.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    let mut b = backend();
    let catalog = Catalog {
        format_version: 2,
        backend_version: "fixture".into(),
        entrypoint_sha256: mcp_gate::catalog::digest(b.artifact()).unwrap(),
        args: vec![],
        invocation_sha256: Some(fingerprint(&b).unwrap()),
        server_info: info,
        resources: vec![],
        resource_templates: vec![],
        prompts: vec![],
        tools: vec![
            serde_json::from_value(
                serde_json::json!({"name":"test","inputSchema":{"type":"object"}}),
            )
            .unwrap(),
        ],
    };
    std::fs::write(&path, serde_json::to_vec(&catalog).unwrap()).unwrap();
    Catalog::load(&path, &b).unwrap();
    b.inherit_env.push("ADDITIONAL_CONTEXT".into());
    assert!(Catalog::load(&path, &b).is_err());
}

#[test]
fn core_capabilities_and_optional_ui_fallback_are_explicit() {
    let mut info = ServerInfo::default();
    info.protocol_version = ProtocolVersion::V_2025_11_25;
    info.capabilities = serde_json::from_value(serde_json::json!({
        "tools":{},"resources":{"subscribe":false},"prompts":{},"experimental":{},
        "extensions":{"io.modelcontextprotocol/ui":{}}
    }))
    .unwrap();
    validate_capabilities(&info).unwrap();
    info.capabilities.resources.as_mut().unwrap().subscribe = Some(true);
    assert!(validate_capabilities(&info).is_err());
    info.capabilities.resources.as_mut().unwrap().subscribe = Some(false);
    info.capabilities.extensions =
        Some(serde_json::from_value(serde_json::json!({"unknown/mandatory":{}})).unwrap());
    assert!(validate_capabilities(&info).is_err());
    info.capabilities.extensions = None;
    info.capabilities.experimental =
        Some(serde_json::from_value(serde_json::json!({"unknown":{}})).unwrap());
    assert!(validate_capabilities(&info).is_err());
}

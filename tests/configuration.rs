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
fn session_config_without_ownership_source_round_trips_unchanged() {
    let mut original = config();
    original.ownership = mcp_gate::config::Ownership::Session;
    let text = toml::to_string_pretty(&original).unwrap();
    assert!(!text.contains("ownership_source"));
    let reloaded: Config = toml::from_str(&text).unwrap();
    assert_eq!(reloaded.ownership_source, None);
    reloaded.validate().unwrap();
    assert_eq!(toml::to_string_pretty(&reloaded).unwrap(), text);
}

#[test]
fn parallel_calls_require_shared_ownership_and_a_bounded_integer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = toml::to_string(&config()).unwrap();
    assert!(!original.contains("max_concurrent_calls"));
    std::fs::write(&path, &original).unwrap();
    let omitted = Config::load(&path).unwrap();
    assert_eq!(omitted.max_concurrent_calls, 1);
    assert_eq!(toml::to_string(&omitted).unwrap(), original);

    for limit in [0, 65] {
        std::fs::write(&path, format!("max_concurrent_calls = {limit}\n{original}")).unwrap();
        assert_eq!(
            Config::load(&path).err().unwrap().to_string(),
            "max_concurrent_calls must be between 1 and 64"
        );
    }
    for limit in [1, 4, 64] {
        std::fs::write(&path, format!("max_concurrent_calls = {limit}\n{original}")).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.max_concurrent_calls, limit);
        let saved = toml::to_string(&config).unwrap();
        assert_eq!(saved.contains("max_concurrent_calls"), limit > 1);
    }
    let session = original.replace("ownership = \"shared\"", "ownership = \"session\"");
    std::fs::write(&path, format!("max_concurrent_calls = 2\n{session}")).unwrap();
    assert_eq!(
        Config::load(&path).err().unwrap().to_string(),
        "max_concurrent_calls greater than 1 requires ownership = \"shared\""
    );
}

#[test]
fn recovery_is_explicit_and_restricted_to_reviewed_shared_processes() {
    use mcp_gate::config::{Ownership, SharedRecovery};
    let mut c = config();
    assert!(c.shared_recovery == SharedRecovery::Manual);
    assert!(!toml::to_string(&c).unwrap().contains("shared_recovery"));
    c.shared_recovery = SharedRecovery::Restart;
    c.validate().unwrap();
    c.ownership = Ownership::Session;
    assert!(c.validate().is_err());
    c.ownership = Ownership::Shared;
    c.backend.docker = true;
    assert!(c.validate().is_err());
}

#[test]
fn idle_defaults_depend_on_ownership_and_shared_docker_cannot_enable_it() {
    use mcp_gate::config::Ownership;
    use std::time::Duration;
    let mut c = config();
    assert_eq!(c.idle_seconds, None);
    assert_eq!(c.idle_timeout(), None);
    let serialized = toml::Value::try_from(&c).unwrap();
    assert!(serialized.get("idle_seconds").is_none());
    c.idle_seconds = Some(900);
    c.validate().unwrap();
    assert_eq!(c.idle_timeout(), Some(Duration::from_secs(900)));
    c.backend.docker = true;
    c.backend.args = vec!["run".into(), "-i".into(), "fixture".into()];
    assert!(
        c.validate()
            .unwrap_err()
            .to_string()
            .contains("shared Docker")
    );
    for idle in [None, Some(0)] {
        c.idle_seconds = idle;
        c.validate().unwrap();
        assert_eq!(c.idle_timeout(), None);
    }
    c.ownership = Ownership::Session;
    c.idle_seconds = None;
    c.validate().unwrap();
    assert_eq!(c.idle_timeout(), Some(Duration::from_secs(900)));
}

#[test]
fn idle_aliases_round_trip_only_the_canonical_key_and_reject_duplicates() {
    let original = toml::to_string(&config()).unwrap();
    let keys = [
        "idle_seconds",
        "session_idle_seconds",
        "shared_idle_seconds",
    ];
    for key in keys {
        for (input, seconds) in [("900", 900), ("0", 0), ("\"off\"", 0)] {
            let text = format!("{key} = {input}\n{original}");
            let c: Config = toml::from_str(&text).unwrap();
            c.validate().unwrap();
            assert_eq!(c.idle_seconds, Some(seconds));
            assert_eq!(c.idle_timeout().is_none(), seconds == 0);
            let serialized = toml::to_string(&c).unwrap();
            assert!(serialized.contains(&format!("idle_seconds = {seconds}\n")));
            for alias in &keys[1..] {
                assert!(!serialized.contains(alias));
            }
            let loaded: Config = toml::from_str(&serialized).unwrap();
            assert_eq!(loaded.idle_seconds, c.idle_seconds);
        }
        for duplicate in keys {
            let text = format!("{key} = 900\n{duplicate} = 900\n{original}");
            assert!(toml::from_str::<Config>(&text).is_err());
        }
    }
    for invalid in ["-1", "1.5", "true", "\"15m\"", "\"never\""] {
        let text = format!("idle_seconds = {invalid}\n{original}");
        assert!(toml::from_str::<Config>(&text).is_err());
    }
}

#[test]
fn backend_replacement_preserves_session_idle_settings() {
    use mcp_gate::config::Ownership;
    use std::time::Duration;
    let mut original = config();
    original.ownership = Ownership::Session;
    original.session_idle_evict_seconds = 1;
    let original = toml::to_string(&original).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    for (setting, seconds, timeout) in [
        ("", None, Some(900)),
        ("3", Some(3), Some(3)),
        ("0", Some(0), None),
        ("\"off\"", Some(0), None),
    ] {
        let text = if setting.is_empty() {
            original.clone()
        } else {
            format!("idle_seconds = {setting}\n{original}")
        };
        std::fs::write(&path, text).unwrap();
        let mut c = Config::load(&path).unwrap();
        c.backend.env.insert("MOCK_CORE_CATALOG".into(), "1".into());
        std::fs::write(&path, toml::to_string(&c).unwrap()).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.idle_seconds, seconds);
        assert_eq!(loaded.idle_timeout(), timeout.map(Duration::from_secs));
        assert_eq!(loaded.session_idle_evict_seconds, 1);
    }
}

#[test]
fn eviction_is_session_only_and_remains_available_with_idle_disabled() {
    use mcp_gate::config::Ownership;
    let mut c = config();
    c.session_idle_evict_seconds = 10_000;
    c.validate().unwrap(); // Shared ownership ignores session eviction.
    c.ownership = Ownership::Session;
    assert!(c.validate().is_err());
    c.idle_seconds = Some(0);
    c.validate().unwrap();
    assert_eq!(c.idle_timeout(), None);
    c.idle_seconds = Some(120);
    c.session_idle_evict_seconds = 120;
    c.validate().unwrap();
    c.session_idle_evict_seconds = 121;
    assert!(c.validate().is_err());
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
    // Extensions are negotiated and mcp-gate declares none, so unknown ones,
    // with or without settings, are inert rather than a reason to fail.
    info.capabilities.extensions = Some(
        serde_json::from_value(serde_json::json!({
            "io.modelcontextprotocol/skills":{"directoryRead":true},
            "com.example/unknown":{"revision":"9"},
            "io.modelcontextprotocol/ui":{"mimeTypes":["text/html;profile=mcp-app"]}
        }))
        .unwrap(),
    );
    validate_capabilities(&info).unwrap();
    assert_eq!(
        mcp_gate::catalog::unforwarded_extensions(&info),
        [
            "com.example/unknown",
            "io.modelcontextprotocol/skills",
            "io.modelcontextprotocol/ui"
        ]
    );
    assert!(
        mcp_gate::catalog::unforwarded_note(&info)
            .unwrap()
            .contains("io.modelcontextprotocol/skills")
    );
    let mut legacy = info.clone();
    legacy.protocol_version = ProtocolVersion::V_2024_11_05;
    legacy.capabilities.resources = None;
    legacy.capabilities.prompts = None;
    validate_capabilities(&legacy).unwrap();
    info.capabilities.extensions = None;
    assert!(mcp_gate::catalog::unforwarded_note(&info).is_none());
    info.capabilities.experimental =
        Some(serde_json::from_value(serde_json::json!({"unknown":{}})).unwrap());
    assert!(validate_capabilities(&info).is_err());
}

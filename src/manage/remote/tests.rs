use super::*;
use crate::clients::{Client, document};
use std::fs;

const URL: &str = "\"http://127.0.0.1:4321/mcp\"";
const DIRECT_TOML: &str = "[mcp_servers.fixture]\ncommand = 'npx'\nargs = ['fixture']\n\
                          enabled = true\ntimeout = 42\n";

fn input(client: Client, direct: Value, section: &str, toml: bool) -> Binding {
    let keys = vec![section.into(), "fixture".into()];
    Binding {
        client,
        name: "fixture".into(),
        source: "fixture".into(),
        source_path: keys.clone(),
        target: "fixture".into(),
        path: keys,
        toml,
        before: direct.clone(),
        direct,
        project: None,
        after: Value::Null,
    }
}

fn config(root: &Path) -> Config {
    serde_json::from_value(json!({
        "listen": "127.0.0.1:4321",
        "token_file": root.join("client-token"),
        "catalog_file": root.join("catalog.json"),
        "state_dir": root.join("state"),
        "backend": { "command": "fixture", "version": "1" }
    }))
    .unwrap()
}

fn expected_helper(root: &Path) -> String {
    let binary = root.join("current").join(if cfg!(windows) {
        "mcp-gate.exe"
    } else {
        "mcp-gate"
    });
    let config = root.join("gateway config.toml");
    let quote = |path: &Path| {
        if cfg!(windows) {
            format!("\"{}\"", path.display())
        } else {
            format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
        }
    };
    format!("{} headers --config {}", quote(&binary), quote(&config))
}

#[test]
fn codex_binding_preserves_toml_key_order_and_uses_current_helper() {
    let root = Path::new("test root");
    let keys = ["mcp_servers".into(), "fixture".into()];
    let direct = document::entry(DIRECT_TOML, true, &keys).unwrap();
    let input = input(Client::CODEX, direct, "mcp_servers", true);
    let rendered = binding(
        &input,
        root,
        &root.join("gateway config.toml"),
        &config(root),
    )
    .unwrap();
    let helper = expected_helper(root);
    let helper = if cfg!(windows) {
        format!("'{helper}'")
    } else {
        format!("\"{helper}\"")
    };
    assert_eq!(
        document::replace(DIRECT_TOML, true, &input.path, &input.before, &rendered).unwrap(),
        format!(
            "[mcp_servers.fixture]\nenabled = true\ntimeout = 42\nhttp_headers_helper = {helper}\nurl = {URL}\n"
        )
    );
}

#[test]
fn json_bindings_preserve_key_order_and_authentication_layout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let config = config(root);
    let token = "ab".repeat(32);
    super::super::store::atomic(&config.token_file, token.as_bytes()).unwrap();
    let auth = root.join("authorization");
    let header = serde_json::to_string(&format!("{{file:{}}}", auth.display())).unwrap();
    let helper = serde_json::to_string(&expected_helper(root)).unwrap();
    let direct = json!({"args":["fixture"],"command":"npx","env":{"X":"a"},
        "environment":{"Y":"b"},"env_vars":["Z"],"cwd":".","enabled":true,"timeout":42,"type":"stdio"});
    let rendered = |client, section| {
        let input = input(client, direct.clone(), section, false);
        let rendered = binding(&input, root, &root.join("gateway config.toml"), &config).unwrap();
        serde_json::to_string(&rendered).unwrap()
    };
    assert_eq!(
        rendered(Client::CLAUDE_CODE, "mcpServers"),
        format!(
            r#"{{"enabled":true,"headersHelper":{helper},"timeout":42,"type":"http","url":{URL}}}"#
        )
    );
    assert!(!auth.exists());
    assert_eq!(
        rendered(Client::OPENCODE, "mcp"),
        format!(
            r#"{{"enabled":true,"headers":{{"Authorization":{header}}},"oauth":false,"timeout":42,"type":"remote","url":{URL}}}"#
        )
    );
    assert_eq!(fs::read_to_string(auth).unwrap(), format!("Bearer {token}"));
}

#[test]
fn helper_keys_match_existing_client_configs() {
    use super::super::helper::field;
    assert_eq!(field(Client::CODEX), Some("http_headers_helper"));
    assert_eq!(field(Client::CLAUDE_CODE), Some("headersHelper"));
    assert_eq!(field(Client::OPENCODE), None);
}

#[test]
fn claude_desktop_binding_replaces_stdio_fields_without_an_auth_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state root");
    let config_path = root.join("gateway config.toml");
    let desktop = Client::try_from("claude-desktop").unwrap();
    let direct = json!({"args":["fixture"],"command":"npx","env":{"X":"a"},
        "environment":{"Y":"b"},"env_vars":["Z"],"cwd":".","enabled":true,"timeout":42});
    let mut input = input(desktop, direct, "mcpServers", false);
    let config = config(&root);
    let binary = root.join("current").join(crate::platform::binary_name());
    let cmd = serde_json::to_string(&binary).unwrap();
    let config_json = serde_json::to_string(&config_path).unwrap();
    let rendered = binding(&input, &root, &config_path, &config).unwrap();
    assert!(Path::new(rendered["command"].as_str().unwrap()).is_absolute());
    assert_eq!(
        rendered.to_string(),
        format!(
            r#"{{"args":["connect","fixture","--config",{config_json}],"command":{cmd},"enabled":true,"timeout":42}}"#
        )
    );
    input.name = "--fixture".into();
    let rendered = binding(&input, &root, &config_path, &config).unwrap();
    assert_eq!(
        rendered["args"],
        json!(["connect", "--config", config_path, "--", "--fixture"])
    );
    assert!(!root.exists(), "Binding rendering must not write any files");
}

#[test]
fn claude_desktop_preview_shows_the_stdio_transition_and_placeholder_paths() {
    let desktop = Client::try_from("claude-desktop").unwrap();
    let direct =
        json!({"command":"npx","args":["fixture"],"env":{"PRIVATE":"hidden"},"timeout":42});
    let mut binding = input(desktop, direct, "mcpServers", false);
    binding.name = "--secret-looking-name".into();
    let candidate = crate::clients::Candidate {
        binding,
        command: vec!["npx".into(), "fixture".into()],
        cwd: ".".into(),
        env: Default::default(),
        env_files: Default::default(),
        inherit_env: vec![],
        issue: None,
        recipe: None,
        service_label: None,
    };
    let preview = super::super::wizard::server_plan(&candidate, true);
    assert_eq!(preview["transition"], "stdio -> gateway stdio bridge");
    assert_eq!(
        preview["diff"],
        json!({
            "before":{"command":"npx","args":["fixture"],"env":"<hidden>","timeout":42},
            "after":{
                "command":format!("<state-root>/current/{}", crate::platform::binary_name()),
                "args":["connect","--config","<gateway config>","--","--secret-looking-name"],
                "timeout":42
            }
        })
    );
}

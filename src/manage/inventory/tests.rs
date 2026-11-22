use super::*;
use serde_json::json;

#[test]
fn bridge_inventory_uses_saved_bindings_to_detect_ownership_and_drift() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let current = current::path(root);
    let settings = root.join("claude_desktop_config.json");
    let keys = vec!["mcpServers".into(), "fixture".into()];
    let bridge =
        json!({"command":current,"args":["connect","fixture","--config","saved config.toml"]});
    let mut candidate = Candidate {
        binding: Binding {
            client: clients::Client::try_from("claude-desktop").unwrap(),
            name: "fixture".into(),
            source: settings.clone(),
            source_path: keys.clone(),
            target: settings,
            path: keys,
            toml: false,
            before: bridge.clone(),
            direct: bridge.clone(),
            after: bridge,
            project: None,
        },
        command: vec![],
        cwd: root.into(),
        env: Default::default(),
        env_files: Default::default(),
        inherit_env: vec![],
        issue: None,
        recipe: None,
        service_label: None,
    };
    let mut registry = Registry::default();
    let observed = |candidate: &Candidate, registry: &Registry| {
        observed_connection(candidate, registry, &[], root, &current, false)
    };
    let external = observed(&candidate, &registry);
    assert_eq!(external.kind, "external");
    assert!(external.reason.unwrap().contains("stdio bridge"));
    registry.gateways.push(Record {
        removing: false,
        id: "gateway".into(),
        release: root.join("release"),
        label: "local.mcp-gate.fixture".into(),
        binary_hash: String::new(),
        binary: current.clone(),
        bindings: vec![candidate.binding.clone()],
    });
    let managed = observed(&candidate, &registry);
    assert_eq!(managed.kind, "managed");
    assert_eq!(managed.gateway.as_deref(), Some("gateway"));
    assert_eq!(managed.state, "not checked");
    candidate.binding.direct["args"][3] = json!("different config.toml");
    assert_eq!(observed(&candidate, &registry).state, "settings changed");
}

use super::Client;
use clap::ValueEnum;

#[test]
fn client_ids_match_cli_display_and_registry_in_discovery_order() {
    let mut clients = [Client::CLAUDE_CODE, Client::OPENCODE, Client::CODEX];
    clients.sort();
    let existing: Vec<_> = Client::value_variants()
        .iter()
        .copied()
        .filter(|client| clients.contains(client))
        .collect();
    assert_eq!(existing, clients);
    for (client, id) in clients
        .into_iter()
        .zip(["codex", "opencode", "claude-code"])
    {
        assert_eq!(Client::from_str(id, false), Ok(client));
        assert_eq!(client.to_string(), id);
        let registry_id = format!("\"{id}\"");
        assert_eq!(serde_json::to_string(&client).unwrap(), registry_id);
        assert_eq!(
            serde_json::from_str::<Client>(&registry_id).unwrap(),
            client
        );
    }
    assert!(Client::from_str("Codex", false).is_err());
    assert!(serde_json::from_str::<Client>("\"Codex\"").is_err());
}

#[test]
fn opencode_nested_servers_take_precedence_and_keep_their_document_path() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let dir = home.join(".config/opencode");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("opencode.jsonc");
    std::fs::write(
        &path,
        r#"{"mcp": {
          // The nested section takes precedence over adjacent entries.
          "outside": {"command": ["outside"]},
          "servers": {"fixture": {"command": ["fixture", "one arg"]}},
        }}"#,
    )
    .unwrap();
    let found = super::discover(home, home, &[Client::OPENCODE], false).unwrap();
    assert_eq!(found.len(), 1);
    let entry = &found[0];
    assert_eq!(entry.binding.source, path);
    assert_eq!(entry.binding.source_path, ["mcp", "servers", "fixture"]);
    assert_eq!(entry.binding.path, entry.binding.source_path);
    assert_eq!(entry.command, ["fixture", "one arg"]);
}

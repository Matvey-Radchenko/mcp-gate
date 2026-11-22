use super::*;

fn keys(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn binding(client: Client, file: &Path) -> clients::Binding {
    let section = if client == Client::CODEX {
        "mcp_servers"
    } else {
        "mcpServers"
    };
    let path = keys(&[section, "test"]);
    clients::Binding {
        client,
        name: "test".into(),
        source: file.into(),
        source_path: path.clone(),
        target: file.into(),
        path,
        toml: client == Client::CODEX,
        before: Value::Null,
        direct: Value::Null,
        project: None,
        after: Value::Null,
    }
}

#[test]
fn migrates_only_the_executable_prefix_in_toml_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state space 'Юникод");
    let config = root.join("old config.toml");
    let file = root.join("client");
    let hash = "a".repeat(64);
    for (client, version) in [
        (Client::CODEX, format!("0.1.0-{hash}")),
        (Client::CLAUDE_CODE, format!("0.1.0-local-recovery-{hash}")),
        (Client::CLAUDE_CODE, "legacy".into()),
    ] {
        let binding = binding(client, &file);
        let field = helper::field(client).unwrap();
        let old = root
            .join("binaries")
            .join(version)
            .join(crate::platform::binary_name());
        let before = helper::command(&old, &config).replace(" headers ", "  headers\t");
        let executable = helper::invocation(&before).unwrap();
        let encoded = serde_json::to_string(&before)
            .unwrap()
            .replace("state space", "state\\u0020space")
            .replace("old config", "old\\u0020config");
        let source = if binding.toml {
            format!(
                "# keep\r\n[mcp_servers.test]\r\n{field}  =  {encoded}  # keep\r\nfoo = 'unchanged'\r\n"
            )
        } else {
            format!(
                r#"{{"mcpServers": {{ "test" : {{ /* keep */ "{field}" : {encoded}, "foo" : "unchanged" }} }} }}"#
            )
        };
        let live = document::entry(&source, binding.toml, &binding.path).unwrap();
        let edit = rewrite(&root, &binding, &file, &binding.path, &live)
            .unwrap()
            .unwrap();
        let after = edit.edit(&source).unwrap();
        let old_prefix = serde_json::to_string(executable.prefix)
            .unwrap()
            .replace("state space", "state\\u0020space");
        let new_prefix =
            serde_json::to_string(&executable.replacement(&current::path(&root))).unwrap();
        assert_eq!(
            after,
            source.replacen(
                &old_prefix[1..old_prefix.len() - 1],
                &new_prefix[1..new_prefix.len() - 1],
                1
            )
        );
        let live = document::entry(&after, binding.toml, &binding.path).unwrap();
        let current = helper::executable(client, &live).unwrap();
        assert_eq!(current.executable, current::path(&root));
        assert_eq!(current.arguments, executable.arguments);
        assert!(
            rewrite(&root, &binding, &file, &binding.path, &live)
                .unwrap()
                .is_none()
        );

        let external = dir
            .path()
            .join("other/binaries/legacy")
            .join(crate::platform::binary_name());
        let value = json!({field:helper::command(&external, &config)});
        assert!(
            rewrite(&root, &binding, &file, &binding.path, &value)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn preserves_the_delimiters_of_a_toml_literal_helper() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state space");
    let file = root.join("client.toml");
    let binding = binding(Client::CODEX, &file);
    let old = root
        .join("binaries/legacy")
        .join(crate::platform::binary_name());
    let stable = current::path(&root);
    let config = root.join("gateway config.toml");
    let source = format!(
        "[mcp_servers.test]\nhttp_headers_helper = '\"{}\" headers --config \"{}\"' # keep\n",
        old.display(),
        config.display()
    );
    let entry = document::entry(&source, true, &binding.path).unwrap();
    let edit = rewrite(&root, &binding, &file, &binding.path, &entry)
        .unwrap()
        .unwrap();
    assert_eq!(
        edit.edit(&source).unwrap(),
        source.replacen(old.to_str().unwrap(), stable.to_str().unwrap(), 1)
    );
}

#[cfg(unix)]
#[test]
fn a_version_directory_symlink_does_not_claim_an_outside_helper() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    fs::create_dir_all(root.join("binaries")).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let name = crate::platform::binary_name();
    fs::write(outside.join(name), b"outside").unwrap();
    let legacy = root.join("binaries/legacy");
    std::os::unix::fs::symlink(outside, &legacy).unwrap();
    let client = root.join("client.toml");
    let binding = binding(Client::CODEX, &client);
    let value = json!({"http_headers_helper":helper::command(&legacy.join(name), &root.join("config.toml"))});
    assert!(
        rewrite(&root, &binding, &client, &binding.path, &value)
            .unwrap()
            .is_none()
    );
}

#[test]
fn rewrites_saved_after_without_adopting_live_drift_and_rolls_back_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    store::prepare(root).unwrap();
    let file = root.join("client.json");
    let mut binding = binding(Client::CLAUDE_CODE, &file);
    let old = root
        .join("binaries/legacy")
        .join(crate::platform::binary_name());
    let config = root.join("config.toml");
    let before = helper::command(&old, &config);
    binding.after = json!({"headersHelper":before,"timeout":1});
    let live = json!({"headersHelper":before,"timeout":2});
    let source = json!({"mcpServers":{"test":live}}).to_string();
    fs::write(&file, &source).unwrap();
    let edit = rewrite(root, &binding, &file, &binding.path, &live)
        .unwrap()
        .unwrap();
    let mut registry = Registry {
        format_version: 1,
        gateways: vec![store::Record {
            removing: false,
            id: "test".into(),
            release: root.join("release"),
            label: "test".into(),
            binary_hash: String::new(),
            binary: old,
            bindings: vec![binding],
        }],
    };
    store::save(root, &registry).unwrap();
    let mut outcome = Outcome::default();
    let later = Rewrite {
        file: root.join("missing"),
        ..edit.clone()
    };
    assert!(apply(root, &mut registry, &[edit.clone(), later], &mut outcome).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), source);
    assert!(outcome.messages.is_empty());
    registry = store::load(root).unwrap();
    apply(root, &mut registry, &[edit], &mut outcome).unwrap();
    let registry = store::load(root).unwrap();
    assert_eq!(
        registry.gateways[0].bindings[0].after,
        json!({
            "headersHelper":helper::command(&current::path(root), &config),"timeout":1
        })
    );
    assert_eq!(outcome.restart, BTreeSet::from([Client::CLAUDE_CODE]));
    assert_eq!(outcome.messages.len(), 1);
}

#[test]
fn migrates_a_versioned_bridge_command_and_restarts_claude_desktop() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    store::prepare(root).unwrap();
    let client = Client::try_from("claude-desktop").unwrap();
    let file = root.join("claude_desktop_config.json");
    let mut binding = binding(client, &file);
    let old = root
        .join("binaries")
        .join(format!("1-{}", "a".repeat(64)))
        .join(crate::platform::binary_name());
    let saved = json!({"command":old,"args":["connect","test"],"timeout":1});
    binding.after = saved.clone();
    let live =
        json!({"command":old,"args":["connect","test","--config","edited.toml"],"timeout":2});
    let source = format!(r#"{{ "mcpServers": {{ "test": /* keep this */ {live} }} }}"#);
    fs::write(&file, &source).unwrap();
    let edit = rewrite(root, &binding, &file, &binding.path, &live)
        .unwrap()
        .unwrap();
    assert_eq!(edit.summary()["kind"], "stdio bridge");
    assert!(edit.edit(&source.replace("connect", "headers")).is_err());
    let mut registry = Registry {
        format_version: 1,
        gateways: vec![store::Record {
            removing: false,
            id: "test".into(),
            release: root.join("release"),
            label: "test".into(),
            binary_hash: String::new(),
            binary: old.clone(),
            bindings: vec![binding],
        }],
    };
    store::save(root, &registry).unwrap();
    let mut outcome = Outcome::default();
    apply(root, &mut registry, &[edit], &mut outcome).unwrap();
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        source.replacen(
            &json!(old).to_string(),
            &json!(current::path(root)).to_string(),
            1
        )
    );
    let registry = store::load(root).unwrap();
    let mut expected = saved;
    expected["command"] = json!(current::path(root));
    assert_eq!(registry.gateways[0].bindings[0].after, expected);
    assert_eq!(outcome.restart, BTreeSet::from([client]));
    assert!(outcome.messages[0].contains("stdio bridge migrated"));
}

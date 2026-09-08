use mcp_gate::{
    clients::{self, Client, document},
    manage::store::{self, Journal},
};
use serde_json::json;
use std::fs;
fn keys(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn jsonc_edits_keep_foreign_bytes_comments_permissions_and_restore() {
    let source = r#"{
  // Keep this byte for byte.
  "permissions": {"deny":["shell:*"],"allow":[]},
  "mcp": {"browser": {"type":"local", /* argv comment */ "command":["npx", "a b", "Юникод"],"timeout":123,"enabled":true}},
  "unrelated": [1,2,3]
}"#;
    let path = keys(&["mcp", "browser"]);
    let before = document::entry(source, false, &path).unwrap();
    let after =
        json!({"type":"remote","url":"http://127.0.0.1:4567/mcp","timeout":123,"enabled":true});
    let changed = document::replace(source, false, &path, &before, &after).unwrap();
    assert!(changed.contains("/* argv comment */"));
    assert!(changed.starts_with(&source[..source.find("{\"type\"").unwrap()]));
    assert!(changed.ends_with(",\n  \"unrelated\": [1,2,3]\n}"));
    let roundtrip = document::replace(&changed, false, &path, &after, &before).unwrap();
    assert_eq!(
        document::parse(&roundtrip, false).unwrap(),
        document::parse(source, false).unwrap()
    );
    let later = changed.replace("123", "124");
    assert!(document::replace(&later, false, &path, &after, &before).is_err());
}
#[test]
fn local_override_add_remove_preserves_global_project_and_empty_parents() {
    let source = r#"{"projects":{"/a b/Юникод":{"trusted":true}},"other": 12}"#;
    let path = keys(&["projects", "/a b/Юникод", "mcpServers", "fixture"]);
    let entry = json!({"type":"http","url":"http://127.0.0.1:4567/mcp"});
    let inserted = document::replace(source, false, &path, &json!(null), &entry).unwrap();
    assert_eq!(document::entry(&inserted, false, &path).unwrap(), entry);
    let restored = document::replace(&inserted, false, &path, &entry, &json!(null)).unwrap();
    assert_eq!(
        document::entry(&restored, false, &path).unwrap(),
        json!(null)
    );
    assert_eq!(
        document::parse(&restored, false).unwrap()["projects"]["/a b/Юникод"]["trusted"],
        true
    );
    assert!(restored.ends_with(",\"other\": 12}"));
    for source in [
        "{}",
        "{\"mcpServers\":{}}",
        "{\"mcpServers\":{\"one\":{},}}",
    ] {
        let p = keys(&["mcpServers", "fixture"]);
        let changed = document::replace(source, false, &p, &json!(null), &entry).unwrap();
        let restored = document::replace(&changed, false, &p, &entry, &json!(null)).unwrap();
        assert!(document::entry(&restored, false, &p).unwrap().is_null());
    }
}
#[test]
fn toml_conversion_preserves_approvals_timeouts_and_comments() {
    let source = "# keep\n[features]\na = true\n[mcp_servers.test]\ncommand = 'npx'\nargs = ['fixture']\n# permission\nenabled_tools = ['safe']\ndisabled_tools = ['danger']\ntool_timeout_sec = 31\n";
    let path = keys(&["mcp_servers", "test"]);
    let before = document::entry(source, true, &path).unwrap();
    let mut after = before.clone();
    let map = after.as_object_mut().unwrap();
    map.remove("command");
    map.remove("args");
    map.insert("url".into(), json!("http://127.0.0.1:12345/mcp"));
    let changed = document::replace(source, true, &path, &before, &after).unwrap();
    assert!(changed.starts_with("# keep\n[features]\na = true\n"));
    assert!(changed.contains("# permission\nenabled_tools = ['safe']"));
    assert!(changed.contains("disabled_tools = ['danger']"));
    assert!(changed.contains("tool_timeout_sec = 31"));
    let restored = document::replace(&changed, true, &path, &after, &before).unwrap();
    assert_eq!(
        document::parse(&restored, true).unwrap(),
        document::parse(source, true).unwrap()
    );
}
#[test]
fn discovery_uses_local_claude_override_and_skips_other_shared_files() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    let personal = home.join(".claude.json");
    fs::write(&personal, "{\"permissions\":{\"allow\":[]}}").unwrap();
    let shared = project.join(".mcp.json");
    let original = b"{\"mcpServers\":{\"fixture\":{\"command\":\"npx\",\"args\":[\"fixture\"]}}}";
    fs::write(&shared, original).unwrap();
    let found = clients::discover(&home, &project, &[Client::ClaudeCode], true).unwrap();
    assert_eq!(found.len(), 1);
    let candidate = &found[0];
    assert!(candidate.issue.is_none());
    assert_eq!(candidate.binding.source, shared);
    assert_eq!(candidate.binding.target, personal);
    assert!(candidate.binding.before.is_null());
    assert_eq!(candidate.cwd, project);
    assert_eq!(fs::read(&shared).unwrap(), original);
    fs::write(
        project.join("opencode.jsonc"),
        r#"{"mcp":{"fixture":{"type":"local","command":["npx","fixture"]}}}"#,
    )
    .unwrap();
    let found = clients::discover(&home, &project, &[Client::Opencode], true).unwrap();
    assert!(found[0].issue.as_ref().unwrap().contains("Project file"));
}
#[test]
fn journals_recover_only_owned_writes_and_reject_future_formats() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("settings");
    let journal_file = root.join("operation.json");
    fs::write(&path, b"before").unwrap();
    let mut journal = Journal {
        format_version: 1,
        phase: "preparing".into(),
        services: vec![],
        changes: vec![],
    };
    journal
        .write(&journal_file, &path, b"before".to_vec(), b"after".to_vec())
        .unwrap();
    fs::write(&path, b"user edit").unwrap();
    assert_eq!(journal.restore().len(), 1);
    assert_eq!(fs::read(&path).unwrap(), b"user edit");
    fs::write(&path, b"after").unwrap();
    assert!(journal.restore().is_empty());
    assert_eq!(fs::read(&path).unwrap(), b"before");
    let created = root.join("new-registry");
    let mut new = Journal {
        format_version: 1,
        phase: "preparing".into(),
        services: vec![],
        changes: vec![],
    };
    new.write(&journal_file, &created, vec![], b"new".to_vec())
        .unwrap();
    assert!(new.restore().is_empty());
    assert!(!created.exists());
    store::atomic(
        &root.join("registry.json"),
        br#"{"format_version":999,"gateways":[]}"#,
    )
    .unwrap();
    assert!(store::load(root).is_err());
}
#[test]
fn parse_errors_do_not_echo_private_values() {
    for (source, toml) in [
        ("password = 'SECRET\nbroken", true),
        ("{\"password\": SECRET broken}", false),
    ] {
        assert!(!format!("{:#}", document::parse(source, toml).unwrap_err()).contains("SECRET"));
    }
}

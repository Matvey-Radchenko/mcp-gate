use super::*;
use serde_json::json;
use std::fs;

#[test]
fn claude_desktop_discovers_only_the_personal_file_for_the_selected_os() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let appdata = temp.path().join("roaming");
    let desktop = Client::try_from("claude-desktop").unwrap();
    let project = temp.path().join("project");
    let mac = home.join("Library").join("Application Support");
    let desktop_file = |base: &Path| base.join("Claude").join("claude_desktop_config.json");
    let pair = [
        ("macos", desktop_file(&mac)),
        ("windows", desktop_file(&appdata)),
    ];
    for (os, file) in &pair {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            file,
            json!({"mcpServers":{*os:{"command":"fixture","args":["${CLAUDE_FIXTURE}"]}}})
                .to_string(),
        )
        .unwrap();
    }
    fs::create_dir(&project).unwrap();
    fs::write(project.join("claude_desktop_config.json"), "{}").unwrap();
    let scan = |os| {
        collect(
            &home,
            &project,
            &[desktop],
            true,
            |key| {
                assert_eq!(key, "APPDATA");
                assert_eq!(os, "windows");
                Some(appdata.clone().into_os_string())
            },
            os,
        )
    };
    for (os, file) in &pair {
        let found = scan(os);
        assert!(found.issues.is_empty());
        let [candidate] = found.candidates.as_slice() else {
            panic!("Expected one Claude Desktop entry for {os}");
        };
        assert_eq!(candidate.binding.source, *file);
        assert_eq!(candidate.binding.name, *os);
        assert_eq!(candidate.binding.path, ["mcpServers", os]);
        assert!(candidate.binding.project.is_none());
        assert_eq!(candidate.command, ["fixture", "${CLAUDE_FIXTURE}"]);
    }
    assert!(scan("linux").candidates.is_empty());
    assert!(
        collect(&home, &project, &[desktop], true, |_| None, "windows")
            .candidates
            .is_empty()
    );
}

#[test]
fn gateway_connect_entries_are_ineligible_for_setup() {
    let desktop = Client::try_from("claude-desktop").unwrap();
    let file = Path::new("claude_desktop_config.json");
    let project = Path::new("project");
    for command in ["mcp-gate", "mcp-gate.exe"] {
        let executable = Path::new("state").join("current").join(command);
        let entry = json!({"command":executable,"args":["connect","fixture"],"cwd":"."});
        let candidate =
            candidate(desktop, "fixture", file, vec![], &entry, project, false).unwrap();
        assert!(
            candidate
                .issue
                .as_deref()
                .unwrap()
                .starts_with("External gateway stdio bridge")
        );
    }
    let other = json!({"command":"mcp-gate","args":["something-else"],"cwd":"."});
    assert!(
        candidate(desktop, "fixture", file, vec![], &other, project, false)
            .unwrap()
            .issue
            .is_none()
    );
}

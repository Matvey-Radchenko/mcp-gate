use super::*;
use crate::clients::Client;

fn assert_paths(
    client: Client,
    overrides: &[(&str, &str)],
    personal: &[&str],
    projected: &[&str],
    section: &str,
) {
    let home = Path::new("home");
    let project = Path::new("project");
    let descriptor = client.descriptor();
    let all_personal = personal_paths(Some(home), descriptor, |name| {
        overrides
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).into())
    });
    let expected = personal
        .iter()
        .map(|path| (PathBuf::from(path), false, section));
    let actual = |project: &Path, selected| {
        paths(
            descriptor,
            all_personal.clone(),
            selected_project(home, project, selected),
        )
        .into_iter()
        .map(|s| (s.path, s.project, descriptor.section.as_str()))
        .collect::<Vec<_>>()
    };
    assert_eq!(
        actual(project, true),
        expected
            .clone()
            .chain(
                projected
                    .iter()
                    .map(|path| (PathBuf::from(path), true, section))
            )
            .collect::<Vec<_>>()
    );
    assert_eq!(actual(project, false), expected.clone().collect::<Vec<_>>());
    assert_eq!(actual(home, true), expected.collect::<Vec<_>>());
}

#[test]
fn personal_and_project_configs_have_fixed_order() {
    assert_paths(
        Client::CODEX,
        &[],
        &["home/.codex/config.toml"],
        &["project/.codex/config.toml"],
        "mcp_servers",
    );
    assert_paths(
        Client::OPENCODE,
        &[],
        &[
            "home/.config/opencode/opencode.json",
            "home/.config/opencode/opencode.jsonc",
        ],
        &["project/opencode.json", "project/opencode.jsonc"],
        "mcp",
    );
    assert_paths(
        Client::CLAUDE_CODE,
        &[],
        &["home/.claude.json"],
        &["project/.mcp.json"],
        "mcpServers",
    );
}

#[test]
fn inherited_overrides_replace_defaults_and_keep_project_paths() {
    assert_paths(
        Client::CODEX,
        &[("CODEX_HOME", "codex home")],
        &["codex home/config.toml"],
        &["project/.codex/config.toml"],
        "mcp_servers",
    );
    assert_paths(
        Client::CLAUDE_CODE,
        &[("CLAUDE_CONFIG_DIR", "claude home")],
        &["claude home/.claude.json"],
        &["project/.mcp.json"],
        "mcpServers",
    );
    let project = ["project/opencode.json", "project/opencode.jsonc"];
    let xdg = [
        "config home/opencode/opencode.json",
        "config home/opencode/opencode.jsonc",
    ];
    assert_paths(
        Client::OPENCODE,
        &[("XDG_CONFIG_HOME", "config home")],
        &xdg,
        &project,
        "mcp",
    );
    assert_paths(
        Client::OPENCODE,
        &[("OPENCODE_CONFIG", "override.jsonc")],
        &["override.jsonc"],
        &project,
        "mcp",
    );
    assert_paths(
        Client::OPENCODE,
        &[
            ("XDG_CONFIG_HOME", "config home"),
            ("OPENCODE_CONFIG", "override.jsonc"),
        ],
        &[xdg[0], xdg[1], "override.jsonc"],
        &project,
        "mcp",
    );
}

#[test]
fn a_personal_override_wins_when_it_is_also_the_project_file() {
    assert_paths(
        Client::OPENCODE,
        &[("OPENCODE_CONFIG", "project/opencode.json")],
        &["project/opencode.json"],
        &["project/opencode.jsonc"],
        "mcp",
    );
}

#[test]
fn xdg_locations_keep_native_separators_in_serialized_paths() {
    let actual = personal_paths(None, Client::OPENCODE.descriptor(), |key| {
        (key == "XDG_CONFIG_HOME").then(|| "config home".into())
    });
    let expected = ["opencode.json", "opencode.jsonc"].map(|name| {
        Path::new("config home")
            .join("opencode")
            .join(name)
            .into_os_string()
    });
    assert_eq!(
        actual
            .into_iter()
            .map(PathBuf::into_os_string)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn personal_locations_can_be_limited_to_one_operating_system() {
    let current = std::env::consts::OS;
    let descriptor: Descriptor = toml::from_str(&format!(
        r#"id = "fixture"
display_name = "Fixture"
format = "json"
section = "mcpServers"
personal = [
  {{ os = "another-os", env = "FIXTURE_CONFIG" }},
  {{ os = "{current}", path = "App Support/fixture.json" }},
]
connection = {{ mode = "http", url_key = "url", auth = {{ kind = "helper", key = "headersHelper" }} }}"#
    ))
    .unwrap();
    let home = Path::new("home");
    let actual = personal_paths(Some(home), &descriptor, |_| Some("ignored".into()));
    assert_eq!(actual, [home.join("App Support/fixture.json")]);
}

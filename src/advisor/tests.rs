use super::*;
use serde_json::{Value, json};

fn backend(args: &[&str]) -> Backend {
    serde_json::from_value(json!({
        "profile": "stdio", "command": "/fixture/bin/npx", "version": "fixture", "args": args
    }))
    .unwrap()
}

fn catalog(name: &str, tools: &[&str]) -> Catalog {
    let tools = tools
        .iter()
        .map(|name| json!({"name": name, "inputSchema": {"type": "object"}}))
        .collect::<Vec<_>>();
    serde_json::from_value(json!({
        "format_version": 3, "backend_version": "fixture", "entrypoint_sha256": "fixture",
        "args": [], "tools": tools,
        "server_info": {
            "protocolVersion": "2025-11-25", "serverInfo": {"name": name, "version": "fixture"},
            "capabilities": {"logging": {}, "tools": {"listChanged": true}}
        }
    }))
    .unwrap()
}

fn signal_kinds(advice: &Advice) -> Vec<SignalKind> {
    advice.signals.iter().map(|signal| signal.kind).collect()
}

#[test]
fn checked_in_signal_data_parses() {
    serde_json::from_str::<Rules>(RULES).unwrap();
}

#[test]
fn playwright_matches_catalog_name_and_versioned_package() {
    let named = catalog("Playwright", &["get_value"]);
    let renamed = catalog("renamed", &["get_value"]);
    for advice in [
        advise(Some(&named), &backend(&[])),
        advise(
            Some(&renamed),
            &backend(&["--package=@playwright/mcp@latest"]),
        ),
    ] {
        assert_eq!(advice.recommendation, Recommendation::Session);
        assert_eq!(signal_kinds(&advice), [SignalKind::KnownServer]);
    }
}

#[test]
fn chrome_selected_page_is_stateful_even_when_annotated_read_only() {
    let mut catalog = catalog("chrome-devtools", &["select_page"]);
    catalog.tools[0].annotations = Some(
        serde_json::from_value(json!({
            "readOnlyHint": true
        }))
        .unwrap(),
    );
    let advice = advise(Some(&catalog), &backend(&[]));
    assert_eq!(advice.recommendation, Recommendation::Session);
    assert_eq!(
        signal_kinds(&advice),
        [SignalKind::KnownServer, SignalKind::StatefulTool]
    );
    catalog.server_info.server_info.name = "renamed".into();
    assert_eq!(
        signal_kinds(&advise(Some(&catalog), &backend(&[]))),
        [SignalKind::StatefulTool]
    );
}

#[test]
fn api_tool_names_do_not_imply_client_selection() {
    for (name, tools) in [
        (
            "atlassian",
            [
                "jira_get_project_issues",
                "confluence_get_page",
                "get_all_projects",
                "confluence_get_page_children",
                "jira_get_project_versions",
            ]
            .as_slice(),
        ),
        (
            "telegram",
            &["get_message_context", "send_message", "get_chats"],
        ),
        (
            "testops",
            &["configure_testops", "get_launch", "list_projects"],
        ),
    ] {
        let advice = advise(Some(&catalog(name, tools)), &backend(&[]));
        assert_eq!(
            advice.recommendation,
            Recommendation::SharedCandidate,
            "{name}"
        );
        assert!(advice.signals.is_empty(), "{name}");
    }
}

#[test]
fn selection_actions_are_recognized_across_tool_name_styles() {
    for tool in [
        "new_page",
        "switch-tab",
        "browser_tab_select",
        "browser_tabs",
        "selectPage",
        "BrowserTabSelect",
        "setCurrentContext",
        "SET-selected-project",
        "select_session",
        "APISetState",
    ] {
        let advice = advise(Some(&catalog("renamed", &[tool])), &backend(&[]));
        assert_eq!(advice.recommendation, Recommendation::Session, "{tool}");
        assert_eq!(signal_kinds(&advice), [SignalKind::StatefulTool], "{tool}");
    }
}

#[test]
fn page_navigation_and_closing_require_browser_context() {
    for name in ["documentation", "remoteBrowser"] {
        let catalog = catalog(name, &["navigate-page", "closePage"]);
        let advice = advise(Some(&catalog), &backend(&[]));
        if name == "documentation" {
            assert_eq!(advice.recommendation, Recommendation::SharedCandidate);
            assert!(advice.signals.is_empty());
        } else {
            assert_eq!(advice.recommendation, Recommendation::Session);
            assert_eq!(signal_kinds(&advice), [SignalKind::StatefulTool; 2]);
        }
    }
}

#[test]
fn read_actions_and_partial_tokens_do_not_count_as_selection() {
    let catalog = catalog(
        "remote-browser",
        &[
            "getSelectedPage",
            "getNewPage",
            "list_browser_tabs",
            "browser_tabs_count",
            "get_project_settings",
            "set_page_size",
            "set_statement",
            "select_pageant",
            "selectable_tab",
            "reset_session",
        ],
    );
    let advice = advise(Some(&catalog), &backend(&[]));
    assert_eq!(advice.recommendation, Recommendation::SharedCandidate);
    assert!(advice.signals.is_empty());
}

#[test]
fn filesystem_detects_roots_and_requires_a_fixed_directory_list_to_share() {
    let catalog = catalog(
        "secure-filesystem-server",
        &["list_allowed_directories", "get-ROOTS-list"],
    );
    let advice = advise(
        Some(&catalog),
        &backend(&["@modelcontextprotocol/server-filesystem"]),
    );
    assert_eq!(advice.recommendation, Recommendation::Session);
    assert_eq!(
        signal_kinds(&advice),
        [
            SignalKind::KnownServer,
            SignalKind::Roots,
            SignalKind::Roots
        ]
    );
    assert!(advice.signals[0].message.contains("fixed directory list"));
}

#[test]
fn time_and_shared_knowledge_graph_are_candidates() {
    for catalog in [
        catalog("time", &["get_current_time", "convert_time"]),
        catalog(
            "memory",
            &["create_entities", "add_observations", "read_graph"],
        ),
    ] {
        let advice = advise(Some(&catalog), &backend(&[]));
        assert_eq!(advice.recommendation, Recommendation::SharedCandidate);
        assert!(advice.signals.is_empty());
        assert!(advice.message.contains("review before sharing"));
        assert!(advice.annotation_note.contains("readOnlyHint"));
    }
}

#[test]
fn resource_subscriptions_signal_per_client_state() {
    let mut catalog = catalog("data", &["get_value"]);
    for (subscribe, expected) in [
        (false, Recommendation::SharedCandidate),
        (true, Recommendation::Session),
    ] {
        catalog.server_info.capabilities.resources =
            Some(serde_json::from_value(json!({"subscribe": subscribe})).unwrap());
        let advice = advise(Some(&catalog), &backend(&[]));
        assert_eq!(advice.recommendation, expected);
        if subscribe {
            assert_eq!(signal_kinds(&advice), [SignalKind::Capability]);
        }
    }
}

#[test]
fn missing_catalog_is_unknown_and_recommends_session() {
    let advice = advise(None, &backend(&["@playwright/mcp"]));
    assert_eq!(advice.recommendation, Recommendation::Session);
    assert_eq!(signal_kinds(&advice), [SignalKind::NoCatalog]);
    assert!(advice.message.contains("unknown"));
}

#[test]
fn signals_never_echo_backend_tokens_or_environment_values() {
    let catalog = catalog("renamed", &["get_value"]);
    let mut backend = backend(&[
        "--token=private-argument",
        "chrome-devtools-mcp@private-tag",
    ]);
    backend
        .env
        .insert("API_TOKEN".into(), "private-environment".into());
    let advice = advise(Some(&catalog), &backend);
    let serialized = serde_json::to_value(advice).unwrap();
    assert_eq!(serialized["recommendation"], "session");
    assert_ne!(serialized["signals"][0], Value::Null);
    assert!(!serialized.to_string().contains("private-"));
}

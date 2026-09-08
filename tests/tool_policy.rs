#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use mcp_gate::{
    catalog::validate_capabilities,
    policy::{ScopedDirectory, ToolPolicy},
};
use rmcp::model::*;
use serde_json::json;
use support::*;

fn policy(root: &std::path::Path) -> ToolPolicy {
    ToolPolicy {
        disabled: vec!["crash".into()],
        scoped_directories: vec![ScopedDirectory {
            tool: "state".into(),
            argument: "output_dir".into(),
            root: root.into(),
        }],
        instructions: "Fixture policy".into(),
    }
}

#[tokio::test]
async fn policies_apply_before_lazy_start_and_isolate_shared_output_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("cache");
    let mut h = Harness::generic("shared", 4, 10, 5).await;
    h.set_policy(policy(&root)).await;
    let (a, b) = tokio::join!(h.session(), h.session());
    let list = a.request("tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3);
    assert!(!tools.iter().any(|t| t["name"] == "crash"));
    assert!(tools.iter().find(|t| t["name"]=="state").unwrap()["inputSchema"]["properties"]["output_dir"]["description"].as_str().unwrap().contains("Relative"));
    assert!(a.call("crash", json!({})).await.get("error").is_some());
    for path in [
        "/tmp/escape",
        "../escape",
        "x/../../escape",
        "x\\escape",
        "C:escape",
        "x\0escape",
    ] {
        assert!(
            a.call("state", json!({"output_dir":path}))
                .await
                .get("error")
                .is_some()
        );
    }
    assert!(a.call("state", json!({})).await.get("error").is_some());
    h.workers(0).await;
    assert!(!root.exists());
    let (av, bv) = tokio::join!(
        a.call("state", json!({"output_dir":"images"})),
        b.call("state", json!({"output_dir":"images"}))
    );
    let av = mock_value(&av);
    let bv = mock_value(&bv);
    let ap = av["arguments"]["output_dir"].as_str().unwrap();
    let bp = bv["arguments"]["output_dir"].as_str().unwrap();
    assert_ne!(ap, bp);
    assert!(std::path::Path::new(ap).starts_with(&root));
    assert!(std::path::Path::new(bp).starts_with(&root));
    assert_eq!(av["pid"], bv["pid"]);
    assert_eq!(
        mock_value(&a.call("state", json!({"output_dir":"images"})).await)["arguments"]["output_dir"],
        ap
    );
    h.workers(1).await;
    a.close().await;
    b.close().await;
    h.stop();
}

#[test]
fn policy_rejects_symlinks_and_invalid_catalog_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("cache");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path(), &root).unwrap();
        let mut request: CallToolRequestParams =
            serde_json::from_value(json!({"name":"state","arguments":{"output_dir":"images"}}))
                .unwrap();
        assert!(
            policy(&root)
                .apply(&mut request, uuid::Uuid::new_v4())
                .is_err()
        );
    }
    let tool: Tool = serde_json::from_value(json!({"name":"state","inputSchema":{"type":"object","properties":{"output_dir":{"type":"string"}}}})).unwrap();
    let mut p = policy(&root);
    assert!(p.validate_catalog(std::slice::from_ref(&tool)).is_err());
    p.disabled.clear();
    p.validate_catalog(std::slice::from_ref(&tool)).unwrap();
    p.scoped_directories[0].argument = "missing".into();
    assert!(p.validate_catalog(&[tool]).is_err());
    p.scoped_directories[0].root = "/".into();
    assert!(p.validate().is_err());
}

#[test]
fn legacy_stdio_is_tools_only_not_legacy_http() {
    let mut info = ServerInfo::default();
    info.protocol_version = ProtocolVersion::V_2024_11_05;
    info.capabilities = ServerCapabilities::builder().enable_tools().build();
    validate_capabilities(&info).unwrap();
    info.capabilities = ServerCapabilities::builder()
        .enable_tools()
        .enable_resources()
        .build();
    assert!(validate_capabilities(&info).is_err());
}

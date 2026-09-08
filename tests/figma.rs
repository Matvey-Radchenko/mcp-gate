#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use mcp_gate::{
    backend::{Backend, Profile},
    policy::{ScopedDirectory, ToolPolicy},
};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};
use support::*;

#[tokio::test]
#[ignore = "Requires FIGMA_ENTRYPOINT, DEVTOOLS_NODE, CODEX_BINARY; network replaced with local fixture"]
async fn real_figma_two_native_clients_download_to_separate_cache_namespaces() {
    use support::native_codex::NativeCodex;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("cache");
    let preload =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/figma_network_fixture.mjs");
    let mut h = Harness::generic("shared", 4, 30, 10).await;
    h.replace_backend(Backend {
        docker: false,
        working_directory: None,
        command_args: Vec::new(),
        directory_env: Vec::new(),
        working_directory_env: None,
        profile: Profile::Stdio,
        command: std::env::var("DEVTOOLS_NODE").unwrap().into(),
        entrypoint: Some(std::env::var("FIGMA_ENTRYPOINT").unwrap().into()),
        version: "0.13.2".into(),
        args: vec!["--stdio".into(), "--no-telemetry".into()],
        inherit_env: vec![],
        env_files: BTreeMap::new(),
        env: BTreeMap::from([
            ("FIGMA_API_KEY".into(), "fixture-not-a-real-key".into()),
            ("IMAGE_DIR".into(), root.to_string_lossy().into()),
            (
                "NODE_OPTIONS".into(),
                format!("--import={}", preload.display()),
            ),
            ("DO_NOT_TRACK".into(), "1".into()),
        ]),
    })
    .await;
    h.ignore_shared_roots().await;
    h.set_policy(ToolPolicy {
        scoped_directories: vec![ScopedDirectory {
            tool: "download_figma_images".into(),
            argument: "localPath".into(),
            root: root.clone(),
        }],
        ..Default::default()
    })
    .await;
    let (mut a, mut b) = tokio::join!(NativeCodex::start(&h), NativeCodex::start(&h));
    let (ac, bc) = tokio::join!(a.discover(), b.discover());
    assert_eq!((ac, bc), (2, 2));
    h.workers(0).await;
    let args = json!({"fileKey":"fixture","localPath":"images","nodes":[{"nodeId":"1:1","fileName":"pixel.png"}]});
    let (av, bv) = tokio::join!(
        a.call("download_figma_images", args.clone()),
        b.call("download_figma_images", args.clone())
    );
    for value in [av, bv] {
        assert_ne!(value["isError"], true, "{value}");
        assert!(content(&json!({"result":value})).contains("Downloaded 1 images"));
    }
    let paths: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|p| p.unwrap().path().join("images/pixel.png"))
        .collect();
    assert_eq!(paths.len(), 2);
    for path in &paths {
        assert!(std::fs::metadata(path).unwrap().len() > 0);
    }
    h.workers(1).await;
    a.close().await;
    let result = b.call("download_figma_images", args).await;
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    let invalid = b.call("download_figma_images",json!({"fileKey":"fixture","localPath":"images","nodes":[{"nodeId":"1:1","fileName":"../../escape.png"}]})).await;
    assert_eq!(invalid["isError"], true);
    assert!(!root.join("escape.png").exists());
    b.close().await;
    h.stop();
}

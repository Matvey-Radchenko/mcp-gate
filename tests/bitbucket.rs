#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use mcp_gate::backend::{Backend, Profile};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::*;

async fn bitbucket_fixture() -> (Harness, tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    let node = std::env::var("DEVTOOLS_NODE").expect("Set Node executable");
    let entry =
        std::env::var("BITBUCKET_ENTRYPOINT").expect("Set installed Bitbucket MCP entrypoint");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let api = axum::Router::new().route("/rest/api/1.0/projects", axum::routing::get(
        move |axum::extract::Query(query): axum::extract::Query<BTreeMap<String,String>>| {
            let count = count.clone();
            async move {
                count.fetch_add(1,Ordering::SeqCst);
                axum::Json(json!({"values":[{"key":"TEST","name":query.get("name")}],"isLastPage":true}))
            }
        }
    ));
    let api_task = tokio::spawn(async move {
        axum::serve(listener, api).await.unwrap();
    });
    let mut h = Harness::generic("shared", 4, 10, 5).await;
    let backend = Backend {
        docker: false,
        working_directory: None,
        command_args: Vec::new(),
        directory_env: Vec::new(),
        working_directory_env: None,
        profile: Profile::Stdio,
        command: node.into(),
        entrypoint: Some(entry.into()),
        version: "3.0.0".into(),
        args: vec![],
        inherit_env: vec![],
        env_files: BTreeMap::new(),
        env: BTreeMap::from([
            ("BITBUCKET_BASE_URL".into(), base),
            ("BITBUCKET_USERNAME".into(), "fixture-user".into()),
            ("BITBUCKET_TOKEN".into(), "fixture-not-a-real-secret".into()),
            ("BITBUCKET_TOOL_GROUPS".into(), "discovery".into()),
            ("BITBUCKET_RETRY_MAX".into(), "0".into()),
        ]),
    };
    h.replace_backend(backend).await;
    h.ignore_shared_roots().await;
    (h, api_task, calls)
}

#[tokio::test]
#[ignore = "Requires BITBUCKET_ENTRYPOINT and DEVTOOLS_NODE; upstream API is a local fixture only"]
async fn real_bitbucket_config_only_shared_gateway() {
    let (mut h, api_task, calls) = bitbucket_fixture().await;
    let (a, b) = tokio::join!(Session::new(&h.base), Session::new(&h.base));
    let list = a.request("tools/list", json!({})).await;
    assert!(
        list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "list_projects")
    );
    h.workers(0).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let (av, bv) = tokio::join!(
        a.call("list_projects", json!({"name":"A"})),
        b.call("list_projects", json!({"name":"B"}))
    );
    assert_eq!(mock_value(&av)["projects"][0]["name"], "A");
    assert_eq!(mock_value(&bv)["projects"][0]["name"], "B");
    h.workers(1).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    a.close().await;
    assert_eq!(
        mock_value(&b.call("list_projects", json!({"name":"still-alive"})).await)["projects"][0]["name"],
        "still-alive"
    );
    h.stop();
    api_task.abort();
}

#[tokio::test]
#[ignore = "Requires CODEX_BINARY, BITBUCKET_ENTRYPOINT and DEVTOOLS_NODE; isolated local fixtures only"]
async fn real_bitbucket_via_native_codex() {
    use support::native_codex::NativeCodex;
    let (mut h, api_task, calls) = bitbucket_fixture().await;
    let (mut a, mut b) = tokio::join!(NativeCodex::start(&h), NativeCodex::start(&h));
    let (ac, bc) = tokio::join!(a.discover(), b.discover());
    assert!(ac > 0 && ac == bc);
    h.workers(0).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let (av, bv) = tokio::join!(
        a.call("list_projects", json!({"name":"Native-A"})),
        b.call("list_projects", json!({"name":"Native-B"}))
    );
    assert_eq!(
        mock_value(&json!({"result":av}))["projects"][0]["name"],
        "Native-A"
    );
    assert_eq!(
        mock_value(&json!({"result":bv}))["projects"][0]["name"],
        "Native-B"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    h.workers(1).await;
    a.close().await;
    let bv = b
        .call("list_projects", json!({"name":"Native-survivor"}))
        .await;
    assert_eq!(
        mock_value(&json!({"result":bv}))["projects"][0]["name"],
        "Native-survivor"
    );
    b.close().await;
    h.workers(1).await;
    h.stop();
    api_task.abort();
    eprintln!(
        "Native Codex -> HTTP gateway -> real Bitbucket MCP 3.0.0: lazy discovery, two clients, one backend, independent close passed; API fixture only"
    );
}

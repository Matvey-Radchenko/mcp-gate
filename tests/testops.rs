#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use mcp_gate::{
    backend::{Backend, Profile},
    policy::ToolPolicy,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::*;

async fn fixture() -> (Harness, tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let tokens = Arc::new(AtomicUsize::new(0));
    let count = tokens.clone();
    let api = axum::Router::new()
        .route(
            "/api/uaa/oauth/token",
            axum::routing::post(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({"access_token":"fixture-only", "expires_in":3600}))
                }
            }),
        )
        .route(
            "/api/testcase/{id}",
            axum::routing::get(
                |axum::extract::Path(id): axum::extract::Path<String>| async move {
                    axum::Json(json!({"id":id,"name":format!("Fixture-{id}")}))
                },
            ),
        )
        .route(
            "/api/testcase/{id}/step",
            axum::routing::get(|| async { axum::Json(json!([{"id":1,"action":"fixture step"}])) }),
        );
    let task = tokio::spawn(async move { axum::serve(listener, api).await.unwrap() });
    let mut h = Harness::generic("shared", 4, 10, 5).await;
    h.replace_backend(Backend {
        docker: false,
        working_directory: None,
        command_args: Vec::new(),
        directory_env: Vec::new(),
        working_directory_env: None,
        profile: Profile::Stdio,
        command: std::env::var("DEVTOOLS_NODE").unwrap().into(),
        entrypoint: Some(std::env::var("TESTOPS_ENTRYPOINT").unwrap().into()),
        version: "1.0.0".into(),
        args: vec![],
        inherit_env: vec![],
        env_files: BTreeMap::new(),
        env: BTreeMap::from([
            ("TESTOPS_BASE_URL".into(), base),
            ("ALLURE_TOKEN".into(), "fixture-only".into()),
        ]),
    })
    .await;
    h.ignore_shared_roots().await;
    h.set_policy(ToolPolicy {
        disabled: vec!["configure_testops".into()],
        ..Default::default()
    })
    .await;
    (h, task, tokens)
}

#[tokio::test]
#[ignore = "Requires TESTOPS_ENTRYPOINT, DEVTOOLS_NODE and CODEX_BINARY; local API fixture only"]
async fn legacy_testops_via_native_codex_shared_and_configuration_denied() {
    use support::native_codex::NativeCodex;
    let (mut h, task, tokens) = fixture().await;
    let (mut a, mut b) = tokio::join!(NativeCodex::start(&h), NativeCodex::start(&h));
    let (ac, bc) = tokio::join!(a.discover(), b.discover());
    assert_eq!((ac, bc), (3, 3));
    let raw = h.session().await;
    assert!(
        raw.call("configure_testops", json!({"baseUrl":"http://127.0.0.1:1"}))
            .await
            .get("error")
            .is_some()
    );
    h.workers(0).await;
    assert_eq!(tokens.load(Ordering::SeqCst), 0);
    let (av, bv) = tokio::join!(
        a.call("get_testcase", json!({"id":"11"})),
        b.call("get_testcase", json!({"id":"22"}))
    );
    assert!(content(&json!({"result":av})).contains("Fixture-11"));
    assert!(content(&json!({"result":bv})).contains("Fixture-22"));
    assert_eq!(tokens.load(Ordering::SeqCst), 1);
    h.workers(1).await;
    a.close().await;
    let result = b.call("get_testcase_full", json!({"id":"33"})).await;
    let wrapped = json!({"result":result});
    let result = content(&wrapped);
    assert!(result.contains("Fixture-33") && result.contains("fixture step"));
    assert_eq!(tokens.load(Ordering::SeqCst), 1);
    b.close().await;
    raw.close().await;
    h.workers(1).await;
    h.stop();
    task.abort();
}

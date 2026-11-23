use super::*;
use crate::config::Config;
use rmcp::service::serve_directly;
use serde_json::{Value, json};
use std::sync::atomic::AtomicUsize;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, duplex},
    time::timeout,
};

struct Frontend;
impl rmcp::ServerHandler for Frontend {}

#[tokio::test]
async fn shared_cancel_keeps_rmcp_response_waiter_and_execution_permit() {
    check_cancelled_dispatch(true).await;
}

#[tokio::test]
async fn shared_cancel_without_reply_fails_with_original_timeout() {
    check_cancelled_dispatch(false).await;
}

async fn check_cancelled_dispatch(backend_replies: bool) {
    timeout(Duration::from_secs(8), async {
        let mut config: Config = toml::from_str(
            r#"
ownership = "shared"
listen = "127.0.0.1:1"
token_file = "unused"
catalog_file = "unused"
state_dir = "unused"
[backend]
command = "unused"
version = "fixture"
"#,
        )
        .unwrap();
        let state = tempfile::tempdir().unwrap();
        config.state_dir = state.path().to_owned();
        let backend = BackendSlot::new(&config, Arc::new(AtomicUsize::new(0)));
        let admission = backend.admission.clone().try_acquire_owned().unwrap();
        let call = backend
            .begin_call(0, backend.execution.acquire().await)
            .unwrap()
            .unwrap();
        let (gateway, mock) = duplex(4096);
        let upstream = serve_directly::<RoleClient, _, _, _, _>((), gateway, None);
        let (gateway, _client) = duplex(4096);
        let frontend = serve_directly(Frontend, gateway, None);
        let context = RequestContext::new(RequestId::Number(900), frontend.peer().clone());
        let cancel = context.ct.clone();
        let task = tokio::spawn(invoke_shared(
            RequestPeer {
                peer: upstream.peer().clone(),
                progress: broadcast::channel(1).0,
            },
            backend.clone(),
            call,
            admission,
            Duration::from_secs(3),
            ClientRequest::CallToolRequest(CallToolRequest::new(CallToolRequestParams::new(
                "fixture",
            ))),
            context,
        ));

        let mut mock = BufReader::new(mock);
        let request = read_message(&mut mock).await;
        assert_eq!(request["method"], "tools/call");
        cancel.cancel();
        let ServerResult::CallToolResult(result) = task.await.unwrap().unwrap() else {
            panic!("expected cancellation result");
        };
        assert_eq!(result.is_error, Some(true));
        let note = read_message(&mut mock).await;
        assert_eq!(note["method"], "notifications/cancelled");
        assert_eq!(note["params"]["requestId"], request["id"]);
        // The backend has received cancellation but has yet to send its reply.
        assert!(
            timeout(Duration::from_millis(100), backend.execution.acquire())
                .await
                .is_err()
        );

        if backend_replies {
            let response = json!({"jsonrpc":"2.0","id":request["id"],"result":{"content":[]}});
            mock.write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
        }
        let _next = backend.execution.acquire().await;
        let status = backend.status();
        if backend_replies {
            assert_eq!(status["generation"], 0);
            assert!(status["last_failure"].is_null());
        } else {
            assert_eq!(status["generation"], 1);
            assert_eq!(status["last_failure"]["reason"], "timeout");
        }
    })
    .await
    .expect("shared dispatch should settle after the late reply or deadline");
}

async fn read_message(backend: &mut BufReader<DuplexStream>) -> Value {
    let mut line = String::new();
    assert!(backend.read_line(&mut line).await.unwrap() > 0);
    serde_json::from_str(&line).unwrap()
}

#![cfg(feature = "test-backend")]
#[allow(dead_code, reason = "Fixtures are shared with lifecycle suites")]
mod support;

use futures::StreamExt;
use mcp_gate::config::SharedRecovery;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};
use support::{Harness, Session, content, mock_value, records};
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

const DOWNSTREAM_ID: u64 = 1000;
const WAIT: Duration = Duration::from_secs(8);

struct Fixture {
    gateway: Harness,
    journal: PathBuf,
    gates: PathBuf,
    wire: PathBuf,
}

impl Fixture {
    async fn new(limit: Option<u32>, pending: usize, recovery: SharedRecovery) -> Self {
        let gateway = Harness::shared_with(pending, |config| {
            if let Some(limit) = limit {
                config.max_concurrent_calls = limit;
            }
            config.shared_recovery = recovery;
            config
                .backend
                .env
                .insert("MOCK_IGNORE_CANCELLATION".into(), "1".into());
            for (key, file) in [
                ("MOCK_ECHO_JOURNAL", "echo-events"),
                ("MOCK_ECHO_GATE_DIR", "echo-gates"),
                ("MOCK_RAW_WIRE_FILE", "wire"),
            ] {
                let path = config.state_dir.with_file_name(file);
                config
                    .backend
                    .env
                    .insert(key.into(), path.to_str().unwrap().into());
            }
        })
        .await;
        let journal = gateway.config.with_file_name("echo-events");
        let gates = gateway.config.with_file_name("echo-gates");
        let wire = gateway.config.with_file_name("wire");
        std::fs::create_dir(&gates).unwrap();
        // Observe lazy runtime requests only, following explicit catalog setup.
        std::fs::remove_file(&wire).unwrap();
        Self {
            gateway,
            journal,
            gates,
            wire,
        }
    }

    async fn session(&self) -> Session {
        self.gateway.session().await
    }

    async fn runtime(&self) -> Value {
        self.gateway.health().await.unwrap()["runtime"].clone()
    }

    async fn wait_for_runtime(&self, ready: impl Fn(&Value) -> bool) -> Value {
        timeout(WAIT, async {
            loop {
                let runtime = self.runtime().await;
                if ready(&runtime) {
                    return runtime;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("shared slot did not reach the expected runtime state")
    }

    fn release(&self, nonce: u64) {
        release(&self.gates, nonce);
    }

    async fn started(&self, nonce: u64) -> Value {
        eventually(|| {
            records(&self.journal)
                .into_iter()
                .find(|entry| entry["event"] == "start" && entry["nonce"] == nonce)
        })
        .await
    }

    fn hold(&self, session: &Session, nonce: u64, tool_error: bool) -> JoinHandle<Value> {
        let session = session.clone();
        tokio::spawn(async move {
            let response = session
                .post(echo_request(
                    DOWNSTREAM_ID,
                    json!({"nonce":nonce, "hold":true, "tool_error":tool_error}),
                    None,
                ))
                .await;
            assert!(response.status().is_success());
            session.read_message(response, Some(DOWNSTREAM_ID)).await
        })
    }

    fn start_stress_call(
        &self,
        session: &Session,
        nonce: u64,
        report_progress: Option<oneshot::Sender<()>>,
    ) -> JoinHandle<Value> {
        let session = session.clone();
        let gates = self.gates.clone();
        tokio::spawn(async move { stress_call(&session, &gates, nonce, report_progress).await })
    }
}

fn release(gates: &Path, nonce: u64) {
    std::fs::write(gates.join(nonce.to_string()), []).unwrap();
}

async fn eventually<T>(mut observe: impl FnMut() -> Option<T>) -> T {
    timeout(WAIT, async {
        loop {
            if let Some(value) = observe() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture event did not arrive")
}

fn echo_request(id: u64, args: Value, token: Option<&str>) -> Value {
    let mut params = json!({"name":"echo_delay","arguments":args});
    if let Some(token) = token {
        params["_meta"] = json!({"progressToken":token});
    }
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":params})
}

async fn stress_call(
    session: &Session,
    gates: &Path,
    nonce: u64,
    mut report_progress: Option<oneshot::Sender<()>>,
) -> Value {
    const PROGRESS: u64 = 3;
    // Three distinct sessions deliberately reuse downstream request IDs.
    let id = DOWNSTREAM_ID + nonce / 3;
    let token = format!("progress-{id}");
    let response = session
        .post(echo_request(
            id,
            json!({"nonce":nonce,"delay_ms":5 + (nonce * 17) % 45,"progress":PROGRESS,"hold":true}),
            Some(&token),
        ))
        .await;
    assert!(response.status().is_success());
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut progress = Vec::new();
    let mut result = None;
    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk.unwrap());
        while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = buffer.drain(..=end).collect();
            let Some(data) = std::str::from_utf8(&line).unwrap().strip_prefix("data:") else {
                continue;
            };
            if data.trim().is_empty() {
                continue;
            }
            let message: Value = serde_json::from_str(data).unwrap();
            if message["method"] == "notifications/progress" {
                assert_eq!(message["params"]["progressToken"], token);
                assert_eq!(message["params"]["message"], nonce.to_string());
                progress.push(message["params"]["progress"].as_f64().unwrap() as u64);
                if progress.len() == PROGRESS as usize {
                    if let Some(report) = report_progress.take() {
                        report.send(()).unwrap();
                    } else {
                        release(gates, nonce);
                    }
                }
            } else {
                assert_eq!(message["id"], id);
                assert!(result.replace(mock_value(&message)).is_none());
            }
        }
    }
    // rmcp runs incoming notification handlers as independent tasks.
    progress.sort_unstable();
    assert_eq!(progress, (1..=PROGRESS).collect::<Vec<_>>());
    let result = result.expect("missing response");
    assert_eq!(result["nonce"], nonce);
    result["upstream_id"].clone()
}

async fn round_trip_stress(limit: Option<u32>) {
    let f = Fixture::new(limit, 64, SharedRecovery::Manual).await;
    let (a, b, c) = tokio::join!(f.session(), f.session(), f.session());
    assert_eq!(HashSet::from([&a.id, &b.id, &c.id]).len(), 3);
    let clients = [a, b, c];
    timeout(Duration::from_secs(25), async {
        let (report, progress_ready) = oneshot::channel();
        let first = f.start_stress_call(&clients[0], 0, limit.map(|_| report));
        f.started(0).await;
        let second = f.start_stress_call(&clients[1], 1, None);
        let rest = (2..40)
            .map(|nonce| f.start_stress_call(&clients[(nonce % 3) as usize], nonce, None))
            .collect::<Vec<_>>();
        let next_id = timeout(WAIT, second).await.unwrap().unwrap();
        if limit.is_some() {
            timeout(WAIT, progress_ready).await.unwrap().unwrap();
            assert!(!first.is_finished(), "call 1 must finish before call 0");
            f.release(0);
        }
        let mut ids = HashSet::from([next_id, first.await.unwrap()]);
        for call in rest {
            assert!(
                ids.insert(call.await.unwrap()),
                "duplicate upstream request ID"
            );
        }
        assert_eq!(ids.len(), 40);
        let requests: Vec<_> = records(&f.wire)
            .into_iter()
            .filter(|entry| entry["method"] == "tools/call")
            .collect();
        assert_eq!(requests.len(), 40);
        let tokens: HashSet<_> = requests
            .iter()
            .map(|entry| entry["params"]["_meta"]["progressToken"].as_u64().unwrap())
            .collect();
        assert_eq!(tokens.len(), 40);
        assert!(requests.iter().all(|entry| ids.contains(&entry["id"])));
        let max = records(&f.journal)
            .iter()
            .map(|entry| entry["max_active"].as_u64().unwrap())
            .max()
            .unwrap();
        match limit {
            Some(limit) => assert!((2..=u64::from(limit)).contains(&max)),
            None => assert_eq!(max, 1),
        }
    })
    .await
    .expect("stress fixture timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_parallel_results_and_progress_remain_request_scoped() {
    round_trip_stress(Some(4)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn omitted_limit_is_serial_under_load() {
    round_trip_stress(None).await;
}

#[tokio::test]
async fn cancelling_one_call_keeps_its_slot_while_siblings_finish() {
    let f = Fixture::new(Some(4), 0, SharedRecovery::Manual).await;
    let clients = futures::future::join_all((0..4).map(|_| f.session())).await;
    let calls: Vec<_> = clients
        .iter()
        .enumerate()
        .map(|(nonce, client)| f.hold(client, nonce as u64, false))
        .collect();
    let cancelled = f.started(0).await;
    for nonce in 1..4 {
        f.started(nonce).await;
    }
    let full = clients[3].call("echo_delay", json!({"nonce":4})).await;
    assert!(content(&full).contains("Backend request queue is full"));
    let notification = clients[0]
        .post(json!({
            "jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":DOWNSTREAM_ID}
        }))
        .await;
    assert_eq!(notification.status(), reqwest::StatusCode::ACCEPTED);
    let mut calls = calls.into_iter();
    let result = timeout(WAIT, calls.next().unwrap()).await.unwrap().unwrap();
    // rmcp unregisters the cancelled POST stream and suppresses the handler's
    // terminal JSON-RPC response. The 202 plus prompt EOF is the cancellation
    // result on the HTTP wire.
    assert_eq!(result, json!({"transportEnded":true}));
    let upstream = eventually(|| {
        records(&f.wire)
            .into_iter()
            .find(|entry| entry["method"] == "notifications/cancelled")
    })
    .await;
    assert_ne!(upstream["params"]["requestId"], DOWNSTREAM_ID);
    assert_eq!(upstream["params"]["requestId"], cancelled["id"]);
    // The frontend is gone, but the cancelled upstream call still owns its slot.
    let full = clients[3].call("echo_delay", json!({"nonce":5})).await;
    assert!(content(&full).contains("Backend request queue is full"));
    for (index, call) in calls.enumerate() {
        let nonce = index as u64 + 1;
        assert!(!call.is_finished());
        f.release(nonce);
        let response = timeout(WAIT, call).await.unwrap().unwrap();
        assert_eq!(mock_value(&response)["nonce"], nonce);
    }
    f.release(0);
    eventually(|| {
        records(&f.journal)
            .into_iter()
            .find(|entry| entry["nonce"] == 0 && entry["event"] == "end")
    })
    .await;
    let next = clients[0].call("echo_delay", json!({"nonce":6})).await;
    assert_eq!(mock_value(&next)["nonce"], 6);
    let runtime = f.runtime().await;
    assert_eq!(runtime["shared_state"], "running");
    assert_eq!(runtime["degraded"], false);
    assert!(runtime["recovery"]["last_failure"].is_null());
    assert_eq!(
        records(&f.wire)
            .iter()
            .filter(|entry| entry["method"] == "notifications/cancelled")
            .count(),
        1
    );
    assert!(
        !records(&f.journal)
            .iter()
            .any(|entry| entry["nonce"] == 4 || entry["nonce"] == 5)
    );
}

#[tokio::test]
async fn crash_fails_every_inflight_call_before_its_call_timeout() {
    let f = Fixture::new(Some(4), 0, SharedRecovery::Manual).await;
    let clients = futures::future::join_all((0..4).map(|_| f.session())).await;
    let mut calls = Vec::new();
    for (nonce, client) in clients[..3].iter().enumerate() {
        calls.push(f.hold(client, nonce as u64, false));
        f.started(nonce as u64).await;
    }
    timeout(Duration::from_secs(5), async {
        let crash = clients[3].call("crash", json!({})).await;
        assert_eq!(crash["result"]["isError"], true);
        for call in calls {
            let response = call.await.unwrap();
            assert_eq!(response["id"], DOWNSTREAM_ID);
            assert_eq!(response["result"]["isError"], true);
            assert!(content(&response).starts_with("Backend request failed or timed out."));
        }
    })
    .await
    .expect("process exit did not promptly fail pending responses");
    let runtime = f.wait_for_runtime(|r| r["shared_state"] == "failed").await;
    assert_eq!(f.gateway.health().await.unwrap()["workers"], 0);
    assert_eq!(
        runtime["recovery"]["last_failure"]["reason"],
        "transport_closed"
    );
    assert_eq!(runtime["max_concurrent_calls"], 4);
    assert_eq!(runtime["concurrency_per_backend"], 1);
    assert_eq!(runtime["degraded"], true);
}

async fn assert_serial_burst(f: &Fixture, session: &Session, nonces: std::ops::Range<u64>) {
    let calls = nonces.clone().map(|nonce| async move {
        let result = session
            .call("echo_delay", json!({"nonce":nonce,"delay_ms":40}))
            .await;
        assert_eq!(mock_value(&result)["nonce"], nonce);
    });
    timeout(WAIT, futures::future::join_all(calls))
        .await
        .unwrap();
    let starts: Vec<_> = records(&f.journal)
        .into_iter()
        .filter(|entry| {
            entry["event"] == "start" && nonces.contains(&entry["nonce"].as_u64().unwrap())
        })
        .collect();
    assert_eq!(starts.len(), nonces.count());
    assert!(starts.iter().all(|entry| entry["active"] == 1));
}

#[tokio::test]
async fn only_overlapped_errors_degrade_and_fallback_survives_worker_restart() {
    let f = Fixture::new(Some(4), 16, SharedRecovery::Restart).await;
    let (a, b, c) = tokio::join!(f.session(), f.session(), f.session());
    let solo = a
        .call("echo_delay", json!({"nonce":0,"tool_error":true}))
        .await;
    assert_eq!(solo["result"]["isError"], true);
    let runtime = f.runtime().await;
    assert_eq!(runtime["degraded"], false);
    assert_eq!(runtime["concurrency_per_backend"], 4);
    let fails = f.hold(&a, 1, true);
    f.started(1).await;
    let sibling = f.hold(&b, 2, false);
    assert_eq!(f.started(2).await["active"], 2);
    f.release(1);
    assert_eq!(
        timeout(WAIT, fails).await.unwrap().unwrap()["result"]["isError"],
        true
    );
    let runtime = f.runtime().await;
    assert_eq!(runtime["max_concurrent_calls"], 4);
    assert_eq!(runtime["concurrency_per_backend"], 1);
    assert_eq!(runtime["degraded"], true);
    let burst = async {
        assert_serial_burst(&f, &c, 10..20).await;
    };
    let release_sibling = async {
        timeout(WAIT, async {
            loop {
                let health = f.gateway.health().await.unwrap();
                if health["session_details"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["id"] == c.id)
                    .is_some_and(|entry| entry["active_calls"].as_u64().unwrap() >= 2)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("serial calls did not queue while sibling was active");
        f.release(2);
        assert_eq!(
            mock_value(&timeout(WAIT, sibling).await.unwrap().unwrap())["nonce"],
            2
        );
    };
    tokio::join!(burst, release_sibling);
    assert_eq!(c.call("crash", json!({})).await["result"]["isError"], true);
    assert_eq!(
        f.wait_for_runtime(|r| r["shared_state"] == "dormant").await["degraded"],
        true
    );
    assert_serial_burst(&f, &a, 20..28).await;
}

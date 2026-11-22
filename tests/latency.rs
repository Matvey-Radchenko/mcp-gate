#![cfg(feature = "test-backend")]
#[allow(
    dead_code,
    reason = "Shared integration fixtures are exercised by different suites"
)]
mod support;
use serde_json::json;
use std::time::{Duration, Instant};
use support::*;

/// Sequential mock calls through the real gateway listener must stay well under
/// the ~40 ms Linux delayed-ACK stall that appears when accepted sockets leave
/// Nagle enabled. Warm the backend first, then time a reused HTTP connection.
#[tokio::test]
async fn sequential_gateway_calls_do_not_pay_delayed_ack() {
    let mut harness = Harness::generic("shared", 4, 10, 5).await;
    let session = harness.session().await;
    session.call("state", json!({"value": "warmup"})).await;

    let mut samples = Vec::with_capacity(8);
    for index in 0..8 {
        let started = Instant::now();
        let value = session
            .call("state", json!({"value": format!("nagle-{index}")}))
            .await;
        samples.push(started.elapsed());
        assert_eq!(
            mock_value(&value)["value"],
            format!("nagle-{index}"),
            "timed call {index} returned an unexpected payload"
        );
    }

    let mut ordered = samples.clone();
    ordered.sort();
    let median = ordered[ordered.len() / 2];
    const LIMIT: Duration = Duration::from_millis(25);
    assert!(
        median < LIMIT,
        "median sequential gateway call {median:?} reached the delayed-ACK range (~40 ms); samples: {samples:?}"
    );

    session.close().await;
    harness.stop();
}

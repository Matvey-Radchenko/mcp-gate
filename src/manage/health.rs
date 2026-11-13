//! Readiness is independent of listener reachability and worker count.
use serde_json::{Value, json};

pub fn state(health: Option<&Value>) -> &'static str {
    let Some(h) = health else {
        return "unavailable";
    };
    if h["maintenance"] == true {
        return "maintenance";
    }
    match h["runtime"]["shared_state"].as_str() {
        Some("failed") => "blocked; backend unavailable",
        Some("recovering") => "recovering; backend unavailable",
        Some("dormant") if h["workers"] == 0 => "ready; backend idle",
        Some("running") if h["workers"].as_u64().is_some_and(|w| w > 0) => "backend active",
        _ if h["runtime"]["ownership"] == "session" && h["workers"] == 0 => "ready; backend idle",
        _ if h["runtime"]["ownership"] == "session" => "backend active; session health separate",
        _ => "reachable; backend unknown",
    }
}

pub fn diagnostic(h: &Value) -> Option<String> {
    let r = &h["runtime"]["recovery"];
    let reason = match r["last_failure"]["reason"].as_str() {
        Some("timeout") => "request timed out; check network/VPN if required by the backend",
        Some("cancelled") => "request cancelled",
        Some("handler_dropped") => "request handler ended before completion",
        Some("transport_closed") => "backend connection closed",
        Some("startup_failed") => "backend initialization failed",
        Some("catalog_changed") => "backend catalog changed; rerun setup with review",
        _ => "failure reason unavailable in this gateway version",
    };
    match h["runtime"]["shared_state"].as_str() {
        Some("failed") => Some(format!(
            "Backend blocked: {reason}. Inspect the previous action before restarting the gateway; no action was retried."
        )),
        Some("recovering") => Some(format!(
            "Backend recovering: {reason}. Cleanup must finish; retry delay: {}s. No action was retried.",
            r["retry_after_seconds"].as_u64().unwrap_or(0)
        )),
        Some("running" | "dormant") if !r["last_failure"].is_null() => Some(format!(
            "Previous failure: {reason}. Backend available again; the previous action was not retried. Verify uncertain writes before replacing them."
        )),
        _ => None,
    }
}

/// External/older services may return arbitrary text. Preserve only typed,
/// recognized diagnostic fields; never echo upstream payloads into status.
pub fn sanitized_runtime(value: &Value) -> Value {
    let r = &value["recovery"];
    let mut result = json!({});
    for (key, allowed) in [
        (
            "shared_state",
            &["failed", "recovering", "running", "dormant"][..],
        ),
        ("ownership", &["shared", "session"][..]),
    ] {
        if let Some(s) = value[key].as_str().filter(|s| allowed.contains(s)) {
            result[key] = s.into();
        }
    }
    if let Some(reason) = r["last_failure"]["reason"].as_str().filter(|s| {
        [
            "timeout",
            "cancelled",
            "handler_dropped",
            "transport_closed",
            "startup_failed",
            "catalog_changed",
        ]
        .contains(s)
    }) {
        result["recovery"] = json!({"retry_after_seconds":r["retry_after_seconds"].as_u64(),
            "last_failure":{"reason":reason,"unix_seconds":r["last_failure"]["unix_seconds"].as_u64()}});
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_backend_is_never_ready_even_without_workers() {
        for workers in [0, 1] {
            let h = json!({"workers":workers,"runtime":{"shared_state":"failed"}});
            assert_eq!(state(Some(&h)), "blocked; backend unavailable");
            assert!(diagnostic(&h).unwrap().contains("reason unavailable"));
        }
        assert_eq!(
            state(Some(&json!({"workers":0}))),
            "reachable; backend unknown"
        );
        assert_eq!(
            state(Some(
                &json!({"workers":0,"runtime":{"shared_state":"dormant"}})
            )),
            "ready; backend idle"
        );
        assert_eq!(
            state(Some(
                &json!({"workers":0,"runtime":{"shared_state":"recovering"}})
            )),
            "recovering; backend unavailable"
        );
    }
    #[test]
    fn legacy_diagnostics_retain_state_without_peer_text() {
        let value = json!({"shared_state":"failed", "private":"secret", "recovery":{"last_failure":{"reason":"timeout","payload":"secret"}}});
        let safe = sanitized_runtime(&value);
        assert_eq!(safe["shared_state"], "failed");
        assert!(!safe.to_string().contains("secret"));
        assert!(
            diagnostic(&json!({"runtime":safe}))
                .unwrap()
                .contains("VPN")
        );
    }
}

//! Backend availability and uncertain request outcomes are separate. State is
//! guarded briefly by a synchronous mutex; process cleanup owns the async lock.
mod events;
use crate::config::SharedRecovery;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    Cancelled,
    HandlerDropped,
    Timeout,
    TransportClosed,
    StartupFailed,
    CatalogChanged,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Ready,
    Draining,
    Blocked,
}

pub(crate) struct Lifecycle {
    pub generation: u64,
    policy: SharedRecovery,
    phase: Phase,
    last_failure: Option<Value>,
    failures: VecDeque<Instant>,
    ready_after: Instant,
    journal: Option<PathBuf>,
}

impl Lifecycle {
    pub fn new(policy: SharedRecovery, journal: Option<PathBuf>) -> Self {
        Self {
            generation: 0,
            policy,
            phase: Phase::Ready,
            last_failure: None,
            failures: VecDeque::new(),
            ready_after: Instant::now(),
            journal,
        }
    }

    /// The epoch is captured before queueing. Calls from a lost generation are
    /// rejected without dispatch; only a fresh request can use the new backend.
    pub fn admit(&self, generation: u64) -> Result<(), &'static str> {
        if generation != self.generation {
            return Err(
                "Backend changed while this request was queued; no action was dispatched. Submit a new request after recovery.",
            );
        }
        match self.phase {
            Phase::Draining => Err(
                "Backend is recovering; no action was dispatched. Wait for cleanup and submit a new request.",
            ),
            Phase::Blocked => Err(
                if self
                    .last_failure
                    .as_ref()
                    .is_some_and(|f| f["reason"] == "catalog_changed")
                {
                    "Backend catalog changed. Rerun setup with review; no action was dispatched."
                } else {
                    "Backend state was lost; the last action was NOT retried. In shared mode restart the gateway after checking the previous action. Reconnect in session mode."
                },
            ),
            Phase::Ready if Instant::now() < self.ready_after => Err(
                "Backend recovery is cooling down after a failure; no action was dispatched. Check status for retry timing and check network/VPN if the backend needs it.",
            ),
            Phase::Ready => Ok(()),
        }
    }

    /// Returns an epoch owned exclusively by the cleanup task. Never clear a
    /// catalog block automatically, even when process restart is opted in.
    pub fn fail(&mut self, reason: Reason) -> Option<u64> {
        if self.phase != Phase::Ready {
            return None;
        }
        self.generation += 1;
        self.phase = Phase::Draining;
        let now = Instant::now();
        self.failures
            .retain(|at| now.duration_since(*at) < Duration::from_secs(60));
        self.failures.push_back(now);
        self.ready_after = if self.failures.len() >= 3 {
            self.failures[0] + Duration::from_secs(60)
        } else {
            now + Duration::from_secs(1 << (self.failures.len() - 1))
        };
        self.last_failure = Some(json!({"reason":reason, "generation":self.generation,
            "unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            "action_outcome": if matches!(reason, Reason::CatalogChanged | Reason::StartupFailed) {"not_dispatched"} else {"unknown"},
            "action_retried":false}));
        self.record("failed");
        Some(self.generation)
    }

    pub fn drained(&mut self, generation: u64) {
        if self.generation != generation || self.phase != Phase::Draining {
            return;
        }
        self.phase = if self.policy == SharedRecovery::Restart
            && self
                .last_failure
                .as_ref()
                .is_none_or(|f| f["reason"] != "catalog_changed")
        {
            Phase::Ready
        } else {
            Phase::Blocked
        };
        self.record("cleanup_complete");
    }

    pub fn snapshot(&self, live: usize) -> Value {
        let remaining = self.ready_after.saturating_duration_since(Instant::now());
        let state = match self.phase {
            Phase::Blocked => "failed",
            Phase::Draining => "recovering",
            Phase::Ready if !remaining.is_zero() => "recovering",
            Phase::Ready if live > 0 => "running",
            Phase::Ready => "dormant",
        };
        json!({"state":state, "policy":self.policy, "generation":self.generation,
            "phase":match self.phase {Phase::Draining => "cleanup", Phase::Blocked => "blocked", Phase::Ready if !remaining.is_zero() => "cooldown", _ => "ready"},
            "retry_after_seconds":remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0),
            "last_failure":self.last_failure})
    }

    fn record(&self, event: &str) {
        tracing::warn!(event, generation=self.generation, failure=?self.last_failure, "backend lifecycle transition");
        if let Some(path) = &self.journal
            && events::append(path, &json!({"format_version":1,"event":event,
                "daemon_pid":std::process::id(),
                "unix_seconds":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
                "recovery":self.snapshot(0)})).is_err()
        {
            tracing::error!(
                "Cannot persist private recovery journal; lifecycle event retained in service log"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cooldown_expires_and_stale_cleanup_cannot_unlock_current_generation() {
        let mut state = Lifecycle::new(SharedRecovery::Restart, None);
        let first = state.fail(Reason::Timeout).unwrap();
        state.drained(first);
        state.ready_after = Instant::now();
        assert!(state.admit(first).is_ok());
        let second = state.fail(Reason::TransportClosed).unwrap();
        state.drained(first);
        assert_eq!(state.snapshot(0)["phase"], "cleanup");
        assert!(state.admit(second).is_err());
        state.drained(second);
        state.ready_after = Instant::now();
        assert!(state.admit(second).is_ok());
        assert!(state.admit(first).is_err());
        state.fail(Reason::CatalogChanged);
        state.drained(state.generation);
        state.ready_after = Instant::now();
        assert!(state.admit(state.generation).is_err());
        assert_eq!(state.snapshot(0)["state"], "failed");
    }
}

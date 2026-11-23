//! HTTP session leases own capacity until response bodies and executable
//! handlers finish, even if the SDK removes the session during disconnect.
use super::HttpState;
use crate::config::Ownership;
use axum::body::{Body, Bytes};
use http_body::{Body as HttpBody, Frame, SizeHint};
use rmcp::transport::streamable_http_server::session::SessionManager;
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;

#[cfg(test)]
mod tests;

// Shared backend shutdown has its own independently configured timer.
const SHARED_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

pub(super) struct Lease {
    pub streams: usize,
    pub requests: usize,
    pub disconnected: Option<Instant>,
    pub last_activity: Instant,
    pub closing: bool,
    pub active: usize,
    ownership: Ownership,
    capacity: Option<OwnedSemaphorePermit>,
}
pub(super) type LeaseRef = Arc<Mutex<Lease>>;

impl Lease {
    pub fn new(ownership: Ownership, capacity: OwnedSemaphorePermit) -> Self {
        Self {
            streams: 0,
            requests: 0,
            disconnected: None,
            last_activity: Instant::now(),
            closing: false,
            active: 0,
            ownership,
            capacity: Some(capacity),
        }
    }

    fn can_close(&self) -> bool {
        !self.closing && self.streams == 0 && self.requests == 0 && self.active == 0
    }

    fn expiry(
        &self,
        now: Instant,
        backend_idle: Option<Duration>,
        grace: Duration,
    ) -> Option<&'static str> {
        if !self.can_close() {
            return None;
        }
        let idle = match self.ownership {
            Ownership::Session => backend_idle,
            Ownership::Shared => Some(SHARED_IDLE_TTL),
        };
        if self
            .disconnected
            .is_some_and(|t| now.duration_since(t) >= grace)
        {
            Some("disconnected")
        } else if idle.is_some_and(|idle| now.duration_since(self.last_activity) >= idle) {
            Some("idle")
        } else {
            None
        }
    }
}

pub(super) async fn reap_sessions(state: &HttpState, now: Instant) {
    // A shared maintenance request waits for any teardown already in progress
    // so a lease being reaped cannot cause a transient maintenance conflict.
    let _maintenance = if state.gateway.config.ownership == Ownership::Shared {
        Some(state.maintenance.lock().await)
    } else {
        None
    };
    let backend_idle = state.gateway.config.idle_timeout();
    let grace = Duration::from_secs(state.gateway.config.disconnect_grace_seconds);
    // Mark closing while holding the lease lock, excluding new HTTP admissions.
    // Release all blocking locks before SDK cancellation and backend cleanup.
    let expired: Vec<_> = state
        .leases
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(id, lease)| {
            let mut lease = lease.lock().unwrap();
            let reason = lease.expiry(now, backend_idle, grace)?;
            lease.closing = true;
            Some((id.clone(), reason))
        })
        .collect();
    for (id, reason) in expired {
        close_session(state, &id, reason).await;
    }
}

// The caller has paused initialization. Check every lease before marking any
// closing, holding their locks together so a delayed handler cannot start
// between the check and the transition.
pub(super) fn mark_idle_shared_sessions(state: &HttpState) -> Option<Vec<String>> {
    let leases = state.leases.lock().unwrap();
    if state.capacity.available_permits() + leases.len() != state.gateway.config.max_sessions {
        // An initializer or a removed lease still owns capacity.
        return None;
    }
    let leases: Vec<_> = leases
        .iter()
        .map(|(id, lease)| (id, lease.lock().unwrap()))
        .collect();
    if leases.iter().any(|(_, lease)| !lease.can_close()) {
        return None;
    }
    Some(
        leases
            .into_iter()
            .map(|(id, mut lease)| {
                lease.closing = true;
                id.clone()
            })
            .collect(),
    )
}

pub(super) async fn close_session(state: &HttpState, id: &str, reason: &'static str) {
    if state.manager.close_session(&id.into()).await.is_err() {
        tracing::warn!(reason, "HTTP session could not be closed by the SDK");
    }
    if let Some(lease) = state.leases.lock().unwrap().remove(id) {
        // The SDK removes the session ID before signalling its transport.
        // A closing lease has no active work and cannot admit any, so cached
        // SDK request extensions need not keep its capacity after removal.
        drop(lease.lock().unwrap().capacity.take());
    }
    tracing::info!(reason, "HTTP session lease removed");
}

pub(super) struct ConnectionGuard {
    pub lease: LeaseRef,
    pub stream: bool,
}
#[derive(Clone)]
pub(crate) struct Activity(pub(super) LeaseRef);
impl Activity {
    pub fn start(&self) -> Option<ActiveCall> {
        let mut lease = self.0.lock().unwrap();
        // The HTTP response may have been dropped before the SDK dispatches
        // its handler. A session selected for closure cannot start new work.
        if lease.closing {
            return None;
        }
        lease.active += 1;
        lease.last_activity = Instant::now();
        Some(ActiveCall(self.0.clone()))
    }
}
// The lease also owns its session-capacity permit. Cancellation may outlive the
// HTTP response; keep that permit and block expiry through handler completion.
pub(crate) struct ActiveCall(LeaseRef);
impl Drop for ActiveCall {
    fn drop(&mut self) {
        let mut lease = self.0.lock().unwrap();
        lease.last_activity = Instant::now();
        lease.active -= 1;
    }
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut value = self.lease.lock().unwrap();
        let now = Instant::now();
        if self.stream {
            // A GET stream protects session-owned recordings. Once the last
            // stream closes, allow a full idle interval to reconnect.
            value.last_activity = now;
            value.streams -= 1;
            if value.streams == 0 {
                value.disconnected = Some(now);
            }
        } else {
            value.requests -= 1;
            if value.ownership == Ownership::Shared {
                value.last_activity = now;
            }
            if value.disconnected.is_some() {
                value.disconnected = Some(now);
            }
        }
    }
}
pub(super) struct TrackedBody {
    pub body: Body,
    pub _guard: Option<ConnectionGuard>,
}
impl HttpBody for TrackedBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

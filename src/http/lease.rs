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
use tokio_util::sync::{CancellationToken, DropGuard};

#[cfg(test)]
mod tests;

// Shared backend shutdown has its own independently configured timer.
const SHARED_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(PartialEq, Eq)]
enum LeaseState {
    Open,
    Closing,
    Removed,
}

pub(super) struct Lease {
    pub streams: usize,
    pub requests: usize,
    pub disconnected: Option<Instant>,
    pub last_activity: Instant,
    pub termination: CancellationToken,
    pub active: usize,
    state: LeaseState,
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
            termination: CancellationToken::new(),
            active: 0,
            state: LeaseState::Open,
            ownership,
            capacity: Some(capacity),
        }
    }

    fn is_quiescent(&self) -> bool {
        self.streams == 0 && self.requests == 0 && self.active == 0
    }

    fn can_close(&self) -> bool {
        !self.is_closing() && self.is_quiescent()
    }

    pub(super) fn is_closing(&self) -> bool {
        self.state != LeaseState::Open
    }

    fn mark_closing(&mut self) {
        // Exclude new admissions and end existing POST waits at the same
        // transition. Shared dispatch keeps its own upstream execution permit.
        if !self.is_closing() {
            self.state = LeaseState::Closing;
            self.termination.cancel();
        }
    }

    fn mark_removed(&mut self) {
        self.mark_closing();
        self.state = LeaseState::Removed;
        self.release_removed_capacity();
    }

    fn release_removed_capacity(&mut self) {
        // Cached SDK extensions can retain a lease after the SDK has removed
        // its ID. Only tracked bodies and handlers still need its capacity.
        if self.state == LeaseState::Removed && self.is_quiescent() {
            drop(self.capacity.take());
        }
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
    // Release all blocking locks before awaiting SDK teardown.
    let expired: Vec<_> = state
        .leases
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(id, lease)| {
            let mut lease = lease.lock().unwrap();
            let reason = lease.expiry(now, backend_idle, grace)?;
            lease.mark_closing();
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
                lease.mark_closing();
                id.clone()
            })
            .collect(),
    )
}

pub(super) async fn close_session(state: &HttpState, id: &str, reason: &'static str) {
    if state.manager.close_session(&id.into()).await.is_err() {
        tracing::warn!(reason, "HTTP session could not be closed by the SDK");
    }
    remove_session(state, id);
    tracing::info!(reason, "HTTP session lease removed");
}

// The SDK has removed the session ID by this point. Mark before unlocking the
// map so requests that already cloned the lease also see the closing state.
pub(super) fn remove_session(state: &HttpState, id: &str) {
    let mut leases = state.leases.lock().unwrap();
    if let Some(lease) = leases.remove(id) {
        lease.lock().unwrap().mark_removed();
    }
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
        if lease.is_closing() {
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
        lease.release_removed_capacity();
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
        value.release_removed_capacity();
    }
}
pub(super) struct TrackedBody {
    body: Body,
    _guard: Option<ConnectionGuard>,
    disconnect: Option<DropGuard>,
}
impl TrackedBody {
    pub(super) fn new(
        body: Body,
        guard: Option<ConnectionGuard>,
        disconnect: Option<DropGuard>,
    ) -> Self {
        let mut tracked = Self {
            body,
            _guard: guard,
            disconnect,
        };
        if tracked.body.is_end_stream() {
            tracked.complete();
        }
        tracked
    }

    fn complete(&mut self) {
        if let Some(disconnect) = self.disconnect.take() {
            disconnect.disarm();
        }
    }
}
impl HttpBody for TrackedBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let frame = Pin::new(&mut self.body).poll_frame(cx);
        if matches!(frame, Poll::Ready(None))
            || (matches!(frame, Poll::Ready(Some(Ok(_)))) && self.body.is_end_stream())
        {
            self.complete();
        }
        frame
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

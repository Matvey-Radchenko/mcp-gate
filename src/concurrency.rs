//! Execution permits protect worker ownership. The call tracker records actual
//! dispatch intervals, excluding time spent waiting for startup or a permit.
use std::{net::SocketAddr, sync::Mutex};
use tokio::sync::{Semaphore, SemaphorePermit};

pub(crate) struct Execution {
    permits: Semaphore,
    limit: u32,
    gateway: SocketAddr,
    state: Mutex<Calls>,
}

#[derive(Default)]
struct Calls {
    started: u64,
    active: u32,
    degraded: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CallFailure {
    JsonRpcError,
    ToolError,
    Timeout,
    Cancelled,
    Transport,
}

impl Execution {
    pub fn new(limit: u32, gateway: SocketAddr) -> Self {
        Self {
            permits: Semaphore::new(limit as usize),
            limit,
            gateway,
            state: Mutex::default(),
        }
    }

    pub fn degraded(&self) -> bool {
        self.state.lock().unwrap().degraded
    }

    pub async fn acquire(&self) -> SemaphorePermit<'_> {
        let count = if self.degraded() { self.limit } else { 1 };
        self.acquire_many(count).await
    }

    pub async fn acquire_all(&self) -> SemaphorePermit<'_> {
        self.acquire_many(self.limit).await
    }

    pub fn try_acquire_all(&self) -> Option<SemaphorePermit<'_>> {
        self.permits.try_acquire_many(self.limit).ok()
    }

    async fn acquire_many(&self, count: u32) -> SemaphorePermit<'_> {
        self.permits
            .acquire_many(count)
            .await
            .expect("execution semaphore is never closed")
    }

    pub fn start<'a>(&'a self, permit: SemaphorePermit<'a>) -> Option<ActiveCall<'a>> {
        let mut state = self.state.lock().unwrap();
        // A previously queued one-permit waiter may wake after degradation.
        // Check in the same critical section as the start so it has to requeue
        // for all permits before it can dispatch.
        if state.degraded && permit.num_permits() != self.limit as usize {
            return None;
        }
        state.started = state.started.wrapping_add(1);
        state.active += 1;
        Some(ActiveCall {
            _permit: permit,
            execution: self,
            start: state.started,
            overlapped_at_start: state.active > 1,
            failure: Some(CallFailure::Cancelled),
        })
    }
}

/// Owned by the dispatch future. Dropping that future counts as cancellation.
pub(crate) struct ActiveCall<'a> {
    // Released after Drop records the outcome and any fallback policy change.
    _permit: SemaphorePermit<'a>,
    execution: &'a Execution,
    start: u64,
    overlapped_at_start: bool,
    pub failure: Option<CallFailure>,
}

impl Drop for ActiveCall<'_> {
    fn drop(&mut self) {
        let degraded = {
            let mut state = self.execution.state.lock().unwrap();
            state.active -= 1;
            let overlapped = self.overlapped_at_start || state.started != self.start;
            // Either part catches an overlap even if the other call already
            // finished. Only the first failure ever changes the slot policy.
            let degrade = overlapped && self.failure.is_some() && !state.degraded;
            state.degraded |= degrade;
            degrade
        };
        if degraded {
            tracing::warn!(
                gateway = %self.execution.gateway,
                reason = ?self.failure.expect("degradation requires a failure"),
                "shared backend degraded to serial execution"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn overlap_is_remembered_and_stale_single_permits_cannot_dispatch() {
        let execution = Execution::new(4, "127.0.0.1:12345".parse().unwrap());
        let mut first = execution.start(execution.acquire().await).unwrap();
        let mut sibling = execution.start(execution.acquire().await).unwrap();
        let stale = execution.acquire().await;
        sibling.failure = None;
        drop(sibling);
        first.failure = Some(CallFailure::ToolError);
        drop(first);

        assert!(execution.degraded());
        assert!(execution.start(stale).is_none());
        assert_eq!(execution.acquire().await.num_permits(), 4);
    }
}

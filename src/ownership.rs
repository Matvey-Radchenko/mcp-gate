//! Process ownership is independent of the HTTP session. Shared slots have a
//! gateway owner; session slots disappear with their last handler.
use crate::{
    config::{Config, Ownership, SharedRecovery},
    gateway::Gateway,
    recovery::{Lifecycle, Reason},
    worker::Worker,
};
use rmcp::{Peer, RoleServer};
use std::{
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};

pub(crate) struct BackendSlot {
    pub worker: Mutex<Option<Worker>>,
    lifecycle: StdMutex<Lifecycle>,
    live: Arc<AtomicUsize>,
    shared: bool,
    last_active: StdMutex<Instant>,
    pub admission: Semaphore,
}
impl BackendSlot {
    pub fn new(config: &Config, live: Arc<AtomicUsize>) -> Arc<Self> {
        Arc::new(Self {
            worker: Mutex::new(None),
            lifecycle: StdMutex::new(Lifecycle::new(
                config.shared_recovery,
                (config.ownership == Ownership::Shared)
                    .then(|| config.state_dir.join("recovery.jsonl")),
            )),
            live,
            shared: config.ownership == Ownership::Shared,
            last_active: StdMutex::new(Instant::now()),
            admission: Semaphore::new(config.max_pending_calls + 1),
        })
    }

    pub fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }

    pub fn idle(&self) -> Duration {
        self.last_active.lock().unwrap().elapsed()
    }

    // The caller holds `worker` across startup and execution: no duplicate spawn,
    // interleaved stateful calls, or unbounded admission queue.
    pub async fn start_worker(
        self: &Arc<Self>,
        gateway: &Gateway,
        peer: Option<Peer<RoleServer>>,
    ) -> Result<Worker, &'static str> {
        let permit = match gateway.capacity.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                // A session that never sends DELETE keeps its worker. When every
                // slot is taken, drop the one that has been idle longest.
                if !gateway.reclaim_idle_worker().await {
                    return Err(
                        "All backend worker slots are in use. Close another session, then retry.",
                    );
                }
                match gateway.capacity.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        return Err(
                            "All backend worker slots are in use. Close another session, then retry.",
                        );
                    }
                }
            }
        };
        let mut startup = FailureGuard::new(
            (gateway.config.shared_recovery == SharedRecovery::Restart).then(|| self.clone()),
            Reason::StartupFailed,
        );
        let worker =
            match Worker::start(&gateway.config, peer, Some(permit), gateway.live.clone()).await {
                Ok(worker) => worker,
                Err(_) => {
                    // Backend errors may contain private paths or peer-provided text.
                    tracing::warn!("backend startup failed");
                    return Err("Backend failed to initialize; no tool action was dispatched.");
                }
            };
        if let Err(message) = self.verify_catalog(gateway, &worker).await {
            worker.shutdown().await;
            return Err(message);
        }
        startup.disarm();
        Ok(worker)
    }

    pub async fn verify_catalog(
        self: &Arc<Self>,
        gateway: &Gateway,
        worker: &Worker,
    ) -> Result<(), &'static str> {
        let check = tokio::time::timeout(
            Duration::from_secs(gateway.config.startup_timeout_seconds),
            worker.catalog(),
        )
        .await;
        let Ok(Ok(snapshot)) = check else {
            // A discovery timeout/disconnect is not evidence of catalog drift.
            self.fail(Reason::StartupFailed);
            return Err(
                "Backend discovery failed; no action was dispatched. Check status for recovery and check network/VPN if required by the backend.",
            );
        };
        let valid = snapshot.info.server_info.version == gateway.catalog.backend_version
            && crate::catalog::validate_capabilities(&snapshot.info).is_ok()
            && serde_json::to_value(&snapshot.info.capabilities).ok()
                == serde_json::to_value(&gateway.catalog.server_info.capabilities).ok()
            && serde_json::to_value(&snapshot.tools).ok()
                == serde_json::to_value(&gateway.catalog.tools).ok()
            && serde_json::to_value(&snapshot.resources).ok()
                == serde_json::to_value(&gateway.catalog.resources).ok()
            && serde_json::to_value(&snapshot.resource_templates).ok()
                == serde_json::to_value(&gateway.catalog.resource_templates).ok()
            && serde_json::to_value(&snapshot.prompts).ok()
                == serde_json::to_value(&gateway.catalog.prompts).ok();
        let valid = valid && !worker.catalog_changed();
        if !valid {
            self.fail(Reason::CatalogChanged);
            return Err(
                "Backend catalog changed. Regenerate/review it before use; no action was dispatched.",
            );
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        if let Some(worker) = self.worker.lock().await.take() {
            worker.shutdown().await;
        }
    }

    pub fn generation(&self) -> u64 {
        self.lifecycle.lock().unwrap().generation
    }
    pub fn admit(&self, generation: u64) -> Result<(), &'static str> {
        self.lifecycle.lock().unwrap().admit(generation)
    }
    pub fn status(&self) -> serde_json::Value {
        self.lifecycle
            .lock()
            .unwrap()
            .snapshot(self.live.load(Ordering::SeqCst))
    }

    pub fn fail(self: &Arc<Self>, reason: Reason) {
        let slot = self.clone();
        tokio::spawn(async move {
            slot.retire(reason).await;
        });
    }

    /// Stop the current worker, if one is idle, and record why it cannot be reused.
    /// A call that holds the worker lock is left running.
    pub async fn retire(self: &Arc<Self>, reason: Reason) -> bool {
        let Some(generation) = self.lifecycle.lock().unwrap().fail(reason) else {
            return false;
        };
        // Hold the execution lock until owned resources are gone. No new
        // worker can appear between taking the old one and its cleanup.
        // Keep the worker lock through shutdown. The next call on this session
        // waits, then observes the drained state instead of a half-finished one.
        let mut guard = self.worker.lock().await;
        if self.generation() != generation {
            return false;
        }
        let stopped = guard.is_some();
        if let Some(worker) = guard.take() {
            worker.shutdown().await;
        }
        // A cancelled startup can leave a ProcessOwner cleaning up via Drop.
        // Shared mode owns the entire single-worker capacity, including that
        // cleanup. Never mark it ready until the permit is actually released.
        self.finish_drain(generation).await;
        drop(guard);
        stopped
    }

    pub async fn retire_if_idle(self: &Arc<Self>, reason: Reason) -> bool {
        // Do not wait for the worker lock. The caller may already hold another
        // session's lock, and waiting here could deadlock two startups.
        let worker = {
            let mut guard = match self.worker.try_lock() {
                Ok(guard) => guard,
                Err(_) => return false,
            };
            guard.take()
        };
        let Some(worker) = worker else {
            return false;
        };
        let generation = self.lifecycle.lock().unwrap().fail(reason);
        worker.shutdown().await;
        if let Some(generation) = generation {
            self.finish_drain(generation).await;
        }
        true
    }

    async fn finish_drain(&self, generation: u64) {
        while self.shared && self.live.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.lifecycle.lock().unwrap().drained(generation);
    }
}

/// A cancelled/timed-out action may still be running upstream. A serial shared
/// backend must not accept the next client's call in that uncertain state.
pub(crate) struct FailureGuard {
    slot: Option<Arc<BackendSlot>>,
    pub reason: Reason,
}
impl FailureGuard {
    pub fn new(slot: Option<Arc<BackendSlot>>, reason: Reason) -> Self {
        Self { slot, reason }
    }
    pub fn disarm(&mut self) {
        self.slot = None;
    }
}
impl Drop for FailureGuard {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.fail(self.reason);
        }
    }
}

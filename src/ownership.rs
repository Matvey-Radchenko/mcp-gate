//! Process ownership is independent of the HTTP session. Shared slots have a
//! gateway owner; session slots disappear with their last handler.
use crate::{
    concurrency::{ActiveCall, Execution},
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
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

pub(crate) struct BackendSlot {
    pub(crate) worker: Mutex<Option<Worker>>,
    pub(crate) execution: Arc<Execution>,
    pub(crate) closing: CancellationToken,
    lifecycle: StdMutex<Lifecycle>,
    live: Arc<AtomicUsize>,
    shared: bool,
    last_active: StdMutex<Instant>,
    pub admission: Arc<Semaphore>,
    admission_capacity: usize,
}
impl BackendSlot {
    pub fn new(config: &Config, live: Arc<AtomicUsize>) -> Arc<Self> {
        let admission_capacity = config.max_pending_calls + config.max_concurrent_calls as usize;
        Arc::new(Self {
            worker: Mutex::new(None),
            execution: Arc::new(Execution::new(config.max_concurrent_calls, config.listen)),
            closing: CancellationToken::new(),
            lifecycle: StdMutex::new(Lifecycle::new(
                config.shared_recovery,
                (config.ownership == Ownership::Shared)
                    .then(|| config.state_dir.join("recovery.jsonl")),
            )),
            live,
            shared: config.ownership == Ownership::Shared,
            last_active: StdMutex::new(Instant::now()),
            admission: Arc::new(Semaphore::new(admission_capacity)),
            admission_capacity,
        })
    }

    pub fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }

    pub fn idle(&self) -> Duration {
        self.last_active.lock().unwrap().elapsed()
    }

    // The caller holds an execution permit before locking `worker`. Startup
    // and catalog verification run under that mutex to prevent duplicate work.
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
        // Daemon shutdown closes the process itself, so shared dispatches can
        // release their permits without waiting for a reply from that process.
        self.closing.cancel();
        let _exclusive = self.execution.acquire_all().await;
        let worker = self.worker.lock().await.take();
        if let Some(worker) = worker {
            worker.shutdown().await;
        }
    }

    /// Normal idle shutdown preserves the lifecycle and every HTTP session
    pub async fn shutdown_if_idle(&self, minimum: Duration) -> bool {
        let Some(_exclusive) = self.execution.try_acquire_all() else {
            return false;
        };
        let Ok(mut guard) = self.worker.try_lock() else {
            return false;
        };
        if self.idle() < minimum
            || self.admission.available_permits() != self.admission_capacity
            || self.admit(self.generation()).is_err()
            || guard.as_ref().is_none_or(Worker::closed)
        {
            return false;
        }
        // Keep all permits through cleanup so a queued call starts a fresh
        // worker only after its predecessor is fully gone.
        let worker = guard.take();
        drop(guard);
        if let Some(worker) = worker {
            worker.shutdown().await;
        }
        tracing::info!("stopped idle shared backend; next executable request starts it again");
        true
    }

    pub fn generation(&self) -> u64 {
        self.lifecycle.lock().unwrap().generation
    }
    pub fn admit(&self, generation: u64) -> Result<(), &'static str> {
        self.lifecycle.lock().unwrap().admit(generation)
    }
    pub fn begin_call(
        &self,
        generation: u64,
        permit: OwnedSemaphorePermit,
    ) -> Result<Option<ActiveCall>, &'static str> {
        // Linearize dispatch admission against failure, including failures
        // that occurred while this caller was waiting for catalog verification.
        let lifecycle = self.lifecycle.lock().unwrap();
        lifecycle.admit(generation)?;
        Ok(self.execution.start(permit))
    }
    pub fn status(&self) -> serde_json::Value {
        self.lifecycle
            .lock()
            .unwrap()
            .snapshot(self.live.load(Ordering::SeqCst))
    }

    /// Block new dispatches immediately. Cleanup runs separately because the
    /// failing caller and its siblings may still hold execution permits.
    pub fn fail(self: &Arc<Self>, reason: Reason) {
        let Some(generation) = self.lifecycle.lock().unwrap().fail(reason) else {
            return;
        };
        let slot = self.clone();
        tokio::spawn(async move {
            slot.reap_generation(generation).await;
        });
    }

    async fn reap_generation(self: &Arc<Self>, generation: u64) {
        // Let admitted siblings finish with their own deadlines. Keep all
        // permits through cleanup so no waiter can observe a half-drained slot.
        let _exclusive = self.execution.acquire_all().await;
        if self.generation() != generation {
            return;
        }
        let worker = self.worker.lock().await.take();
        if let Some(worker) = worker {
            worker.shutdown().await;
        }
        // A cancelled startup can leave a ProcessOwner cleaning up via Drop.
        // Shared mode owns the entire single-worker capacity, including that
        // cleanup. Never mark it ready until the permit is actually released.
        self.finish_drain(generation).await;
    }

    pub async fn retire_if_idle(self: &Arc<Self>, reason: Reason) -> bool {
        // Do not wait for permits or the mutex. The caller may already hold
        // another session's locks, and waiting here could deadlock two startups.
        let Some(_exclusive) = self.execution.try_acquire_all() else {
            return false;
        };
        let worker = {
            let mut guard = match self.worker.try_lock() {
                Ok(guard) => guard,
                Err(_) => return false,
            };
            if reason == Reason::TransportClosed && guard.as_ref().is_none_or(|w| !w.closed()) {
                return false;
            }
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

    #[allow(
        deprecated,
        reason = "Session roots are part of the supported MCP protocol"
    )]
    pub async fn set_roots(&self, roots: rmcp::model::ListRootsResult) {
        let _exclusive = self.execution.acquire_all().await;
        if let Some(worker) = self.worker.lock().await.as_ref() {
            worker.set_roots(roots).await;
        }
    }

    async fn finish_drain(&self, generation: u64) {
        while self.shared && self.live.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.lifecycle.lock().unwrap().drained(generation);
    }
}

/// An abandoned or failed startup can still own a process that needs reaping.
struct FailureGuard {
    slot: Option<Arc<BackendSlot>>,
    reason: Reason,
}
impl FailureGuard {
    fn new(slot: Option<Arc<BackendSlot>>, reason: Reason) -> Self {
        Self { slot, reason }
    }
    fn disarm(&mut self) {
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

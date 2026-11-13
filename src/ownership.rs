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
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore};

pub(crate) struct BackendSlot {
    pub worker: Mutex<Option<Worker>>,
    lifecycle: StdMutex<Lifecycle>,
    live: Arc<AtomicUsize>,
    shared: bool,
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
            admission: Semaphore::new(config.max_pending_calls + 1),
        })
    }

    // The caller holds `worker` across startup and execution: no duplicate spawn,
    // interleaved stateful calls, or unbounded admission queue.
    pub async fn start_worker(
        self: &Arc<Self>,
        gateway: &Gateway,
        peer: Option<Peer<RoleServer>>,
    ) -> Result<Worker, &'static str> {
        let Ok(permit) = gateway.capacity.clone().try_acquire_owned() else {
            return Err("All backend worker slots are in use. Close another session, then retry.");
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
        let Some(generation) = self.lifecycle.lock().unwrap().fail(reason) else {
            return;
        };
        let slot = self.clone();
        tokio::spawn(async move {
            // Hold the execution lock until owned resources are gone. No new
            // worker can appear between taking the old one and its cleanup.
            let mut worker = slot.worker.lock().await;
            if slot.generation() != generation {
                return;
            }
            if let Some(worker) = worker.take() {
                worker.shutdown().await;
            }
            // A cancelled startup can leave a ProcessOwner cleaning up via Drop.
            // Shared mode owns the entire single-worker capacity, including that
            // cleanup. Never mark it ready until the permit is actually released.
            while slot.shared && slot.live.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            slot.lifecycle.lock().unwrap().drained(generation);
        });
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

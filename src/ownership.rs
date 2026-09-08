//! Process ownership is independent of the HTTP session. Shared slots have a
//! gateway owner; session slots disappear with their last handler.
use crate::{gateway::Gateway, worker::Worker};
use rmcp::{Peer, RoleServer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore};

pub(crate) struct BackendSlot {
    pub worker: Mutex<Option<Worker>>,
    pub failed: AtomicBool,
    pub admission: Semaphore,
}
impl BackendSlot {
    pub fn new(max_pending: usize) -> Arc<Self> {
        Arc::new(Self {
            worker: Mutex::new(None),
            failed: AtomicBool::new(false),
            admission: Semaphore::new(max_pending + 1),
        })
    }

    // The caller holds `worker` across startup and execution: no duplicate spawn,
    // interleaved stateful calls, or unbounded admission queue.
    pub async fn start_worker(
        &self,
        gateway: &Gateway,
        peer: Option<Peer<RoleServer>>,
    ) -> Result<Worker, &'static str> {
        let Ok(permit) = gateway.capacity.clone().try_acquire_owned() else {
            return Err("All backend worker slots are in use. Close another session, then retry.");
        };
        let worker =
            match Worker::start(&gateway.config, peer, Some(permit), gateway.live.clone()).await {
                Ok(worker) => worker,
                Err(_) => {
                    // Backend errors may contain private paths or peer-provided text.
                    tracing::warn!("backend startup failed");
                    return Err("Backend failed to initialize; no tool action was dispatched.");
                }
            };
        if !self.verify_catalog(gateway, &worker).await {
            worker.shutdown().await;
            return Err(
                "Backend catalog differs from the pinned catalog. Regenerate/review it before use.",
            );
        }
        Ok(worker)
    }

    pub async fn verify_catalog(&self, gateway: &Gateway, worker: &Worker) -> bool {
        let check = tokio::time::timeout(
            Duration::from_secs(gateway.config.startup_timeout_seconds),
            worker.catalog(),
        )
        .await;
        let valid = matches!(check, Ok(Ok(ref snapshot))
            if snapshot.info.server_info.version == gateway.catalog.backend_version
            && crate::catalog::validate_capabilities(&snapshot.info).is_ok()
            && serde_json::to_value(&snapshot.info.capabilities).ok()
                == serde_json::to_value(&gateway.catalog.server_info.capabilities).ok()
            && serde_json::to_value(&snapshot.tools).ok() == serde_json::to_value(&gateway.catalog.tools).ok()
            && serde_json::to_value(&snapshot.resources).ok() == serde_json::to_value(&gateway.catalog.resources).ok()
            && serde_json::to_value(&snapshot.resource_templates).ok() == serde_json::to_value(&gateway.catalog.resource_templates).ok()
            && serde_json::to_value(&snapshot.prompts).ok() == serde_json::to_value(&gateway.catalog.prompts).ok());
        let valid = valid && !worker.catalog_changed();
        if !valid {
            self.failed.store(true, Ordering::SeqCst);
        }
        valid
    }

    pub async fn shutdown(&self) {
        if let Some(worker) = self.worker.lock().await.take() {
            worker.shutdown().await;
        }
    }
}

/// A cancelled/timed-out action may still be running upstream. A serial shared
/// backend must not accept the next client's call in that uncertain state.
pub(crate) struct SharedCallGuard(pub Option<Arc<BackendSlot>>);
impl Drop for SharedCallGuard {
    fn drop(&mut self) {
        if let Some(slot) = self.0.take() {
            slot.failed.store(true, Ordering::SeqCst);
            tokio::spawn(async move { slot.shutdown().await });
        }
    }
}

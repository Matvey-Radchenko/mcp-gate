//! Shared admission and lifecycle for all executable MCP operations.
use crate::{
    config::SharedClientRoots, dispatch::failure, gateway::Handler, recovery::Reason,
    worker::Worker,
};
use rmcp::{RoleServer, model::*, service::RequestContext};
use std::{sync::atomic::Ordering, time::Duration};
impl Handler {
    pub(crate) async fn execute(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, ErrorData> {
        let is_tool = matches!(&request, ClientRequest::CallToolRequest(_));
        let _activity = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|p| p.extensions.get::<crate::http::Activity>())
            .map(|a| a.start());
        let session = &self.session;
        let gateway = &session.gateway;
        let backend = &session.backend;
        let shared = gateway.shared.is_some();
        let generation = backend.generation();
        #[allow(deprecated)]
        if shared
            && gateway.config.shared_client_roots == SharedClientRoots::Reject
            && context
                .peer
                .peer_info()
                .is_some_and(|i| i.capabilities.roots.is_some())
        {
            return failure(
                is_tool,
                "Shared mode does not accept client roots. Use session mode for client-specific filesystem context.",
            );
        }
        let Ok(_admission) = backend.admission.try_acquire() else {
            return failure(
                is_tool,
                "Backend request queue is full; no action was dispatched.",
            );
        };
        let mut slot = tokio::select! {
            slot = tokio::time::timeout(Duration::from_secs(gateway.config.queue_timeout_seconds), backend.worker.lock()) => {
                match slot {
                    Ok(slot) => slot,
                    Err(_) => return failure(is_tool, "Backend queue wait timed out; no action was dispatched."),
                }
            },
            _ = context.ct.cancelled() => return failure(is_tool, "Request cancelled before execution"),
        };
        if gateway.stopping.load(Ordering::SeqCst) {
            return failure(is_tool, "Gateway is stopping");
        }
        if let Err(message) = backend.admit(generation) {
            return failure(is_tool, message);
        }
        if slot.as_ref().is_some_and(Worker::closed) {
            backend.fail(Reason::TransportClosed);
            return failure(
                is_tool,
                "Backend exited; no action was dispatched. Check status for recovery; the previous action was NOT retried.",
            );
        }
        if slot.is_none() {
            match backend
                .start_worker(gateway, (!shared).then(|| context.peer.clone()))
                .await
            {
                Ok(worker) => *slot = Some(worker),
                Err(message) => return failure(is_tool, message),
            }
        }
        if let Some(worker) = slot.as_ref()
            && worker.catalog_changed()
            && let Err(message) = backend.verify_catalog(gateway, worker).await
        {
            slot.take();
            return failure(is_tool, message);
        }
        if context.ct.is_cancelled() {
            return failure(is_tool, "Request cancelled before execution");
        }
        crate::dispatch::invoke(
            slot.as_ref().expect("worker initialized under slot lock"),
            backend,
            gateway.config.call_timeout_seconds,
            shared,
            request,
            context,
        )
        .await
    }
}

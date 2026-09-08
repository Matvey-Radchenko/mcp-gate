//! Shared admission and lifecycle for all executable MCP operations.
use crate::{config::SharedClientRoots, dispatch::failure, gateway::Handler, worker::Worker};
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
        if backend.failed.load(Ordering::SeqCst) {
            slot.take();
            return failure(
                is_tool,
                "Backend state was lost; the last action was NOT retried. In shared mode restart the gateway. Reconnect in session mode.",
            );
        }
        if slot.as_ref().is_some_and(Worker::closed) {
            backend.failed.store(true, Ordering::SeqCst);
            slot.take();
            return failure(
                is_tool,
                "Backend exited; the last action was NOT retried. In shared mode restart the gateway. Reconnect in session mode.",
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
            && !backend.verify_catalog(gateway, worker).await
        {
            slot.take();
            return failure(
                is_tool,
                "Backend catalog changed. Regenerate/review it before use; no action was dispatched.",
            );
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

//! Shared admission and lifecycle for all executable MCP operations.
use crate::{
    config::SharedClientRoots, dispatch::failure, gateway::Handler, recovery::Reason,
    worker::Worker,
};
use rmcp::{RoleServer, model::*, service::RequestContext};
use std::{future::Future, sync::atomic::Ordering, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

impl Handler {
    pub(crate) async fn execute(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, ErrorData> {
        let is_tool = matches!(&request, ClientRequest::CallToolRequest(_));
        let activity = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|p| p.extensions.get::<crate::http::Activity>());
        let _activity = if let Some(activity) = activity {
            let Some(call) = activity.start() else {
                return failure(is_tool, "MCP session is closing. Reinitialize the session.");
            };
            Some(call)
        } else {
            None
        };
        let gateway = &self.session.gateway;
        let backend = &self.session.backend;
        backend.touch();
        let generation = backend.generation();
        if self.reject_client_roots(&context) {
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
        let deadline =
            Instant::now().checked_add(Duration::from_secs(gateway.config.queue_timeout_seconds));
        let (mut call, upstream) = loop {
            match self.prepare_dispatch(generation, deadline, &context).await {
                Ok(Some(prepared)) => break prepared,
                Ok(None) => continue, // Degraded while waiting for a single permit.
                Err(message) => return failure(is_tool, message),
            }
        };
        let result = crate::dispatch::invoke(
            upstream,
            backend,
            &gateway.config,
            &mut call,
            request,
            context,
        )
        .await;
        // Idle time starts when the call finishes, not when it was queued.
        backend.touch();
        result
    }

    async fn prepare_dispatch(
        &self,
        generation: u64,
        deadline: Option<Instant>,
        context: &RequestContext<RoleServer>,
    ) -> Result<
        Option<(
            crate::concurrency::ActiveCall<'_>,
            crate::worker::RequestPeer,
        )>,
        &'static str,
    > {
        let gateway = &self.session.gateway;
        let backend = &self.session.backend;
        let permit = wait_in_queue(backend.execution.acquire(), deadline, &context.ct).await?;
        // Permits always precede the mutex. Only preparation owns the mutex.
        // The permit alone keeps the worker alive for the rest of the call.
        let mut slot = match backend.worker.try_lock() {
            Ok(slot) => slot,
            Err(_) => wait_in_queue(backend.worker.lock(), deadline, &context.ct).await?,
        };
        if gateway.stopping.load(Ordering::SeqCst) {
            return Err("Gateway is stopping");
        }
        backend.admit(generation)?;
        if slot.as_ref().is_some_and(Worker::closed) {
            backend.fail(Reason::TransportClosed);
            return Err(
                "Backend exited; no action was dispatched. Check status for recovery; the previous action was NOT retried.",
            );
        }
        if slot.is_none() {
            let downstream = gateway.shared.is_none().then(|| context.peer.clone());
            *slot = Some(backend.start_worker(gateway, downstream).await?);
        } else if let Some(worker) = slot.as_ref()
            && worker.catalog_changed()
        {
            // On failure the separate reaper owns cleanup, after all active
            // calls release their permits.
            backend.verify_catalog(gateway, worker).await?;
        }
        if context.ct.is_cancelled() {
            return Err("Request cancelled before execution");
        }
        let upstream = slot
            .as_ref()
            .expect("worker prepared under slot lock")
            .request_peer();
        let Some(call) = backend.begin_call(generation, permit)? else {
            return Ok(None);
        };
        Ok(Some((call, upstream)))
    }

    #[allow(
        deprecated,
        reason = "Root capability negotiation is part of the supported session protocol"
    )]
    fn reject_client_roots(&self, context: &RequestContext<RoleServer>) -> bool {
        let gateway = &self.session.gateway;
        gateway.shared.is_some()
            && gateway.config.shared_client_roots == SharedClientRoots::Reject
            && context
                .peer
                .peer_info()
                .is_some_and(|i| i.capabilities.roots.is_some())
    }
}

async fn wait_in_queue<T>(
    wait: impl Future<Output = T>,
    deadline: Option<Instant>,
    cancellation: &CancellationToken,
) -> Result<T, &'static str> {
    let queued = async {
        match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, wait).await,
            None => Ok(wait.await),
        }
    };
    tokio::select! {
        result = queued => {
            result.map_err(|_| "Backend queue wait timed out; no action was dispatched.")
        }
        _ = cancellation.cancelled() => Err("Request cancelled before execution"),
    }
}

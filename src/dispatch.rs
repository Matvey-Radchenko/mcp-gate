//! Request-scoped forwarding: response IDs, progress and cancellation never cross clients.
use crate::{
    ownership::{BackendSlot, FailureGuard},
    recovery::Reason,
    worker::Worker,
};
use rmcp::{
    Peer, RoleClient, RoleServer,
    model::*,
    service::{PeerRequestOptions, RequestContext},
};
use std::{sync::Arc, time::Duration};

// Covers both an explicit cancellation and a dropped handler future.
struct CancelOnDrop {
    peer: Peer<RoleClient>,
    id: Option<RequestId>,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let peer = self.peer.clone();
            tokio::spawn(async move {
                if peer
                    .notify_cancelled(CancelledNotificationParam::new(
                        Some(id),
                        Some("Gateway request cancelled".into()),
                    ))
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "Upstream cancellation notification could not be delivered; action outcome remains unknown"
                    );
                }
            });
        }
    }
}
pub(crate) async fn invoke(
    worker: &Worker,
    backend: &Arc<BackendSlot>,
    timeout_seconds: u64,
    shared: bool,
    request: ClientRequest,
    context: RequestContext<RoleServer>,
) -> Result<ServerResult, ErrorData> {
    let is_tool = matches!(&request, ClientRequest::CallToolRequest(_));
    let peer = worker.peer();
    let mut shared_guard =
        FailureGuard::new(shared.then(|| backend.clone()), Reason::HandlerDropped);
    let mut progress = worker.subscribe_progress();
    let downstream_progress: Option<ProgressToken> = context
        .meta
        .get("progressToken")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let handle = match peer
        .send_request_with_option(
            request,
            PeerRequestOptions::with_timeout(Duration::from_secs(timeout_seconds))
                .with_meta(context.meta),
        )
        .await
    {
        Ok(handle) => handle,
        Err(_) => {
            backend.fail(Reason::TransportClosed);
            return failure(
                is_tool,
                "Backend connection lost; the action was NOT retried and its outcome is unknown. Check status for recovery. Check network/VPN if required by the backend.",
            );
        }
    };
    let mut cancel = CancelOnDrop {
        peer: peer.clone(),
        id: Some(handle.id.clone()),
    };
    let upstream_progress = handle.progress_token.clone();
    let response = handle.await_response();
    tokio::pin!(response);
    let result = loop {
        tokio::select! {
            result = &mut response => break result,
            notification = progress.recv(), if downstream_progress.is_some() => {
                if let Ok(mut notification) = notification
                    && notification.progress_token == upstream_progress {
                        notification.progress_token = downstream_progress.clone().unwrap();
                        let _ = context.peer.notify_progress(notification).await;
                    }
            }
            _ = context.ct.cancelled() => {
                shared_guard.reason = Reason::Cancelled;
                return failure(is_tool, "Request cancelled. An in-flight tool action may already have occurred; it was not retried. Check status for recovery.");
            },
        }
    };
    if result.is_ok() || matches!(&result, Err(rmcp::service::ServiceError::McpError(_))) {
        cancel.id = None;
        shared_guard.disarm();
    }
    match result {
        Ok(result) => Ok(result),
        Err(rmcp::service::ServiceError::McpError(error)) => Err(error),
        Err(error) => {
            shared_guard.reason = match &error {
                rmcp::service::ServiceError::Timeout { .. } => Reason::Timeout,
                rmcp::service::ServiceError::Cancelled { .. } => Reason::Cancelled,
                _ => Reason::TransportClosed,
            };
            if matches!(
                error,
                rmcp::service::ServiceError::TransportClosed
                    | rmcp::service::ServiceError::TransportSend(_)
            ) {
                backend.fail(Reason::TransportClosed);
            }
            failure(
                is_tool,
                "Backend request failed or timed out. An action may already have occurred; it was NOT retried. Check status for recovery. Check network/VPN if required by the backend.",
            )
        }
    }
}

/// Tools use isError results; other MCP methods use JSON-RPC errors.
pub(crate) fn failure(is_tool: bool, message: &str) -> Result<ServerResult, ErrorData> {
    if is_tool {
        Ok(ServerResult::CallToolResult(CallToolResult::error(vec![
            ContentBlock::text(message),
        ])))
    } else {
        Err(ErrorData::internal_error(message.to_owned(), None))
    }
}

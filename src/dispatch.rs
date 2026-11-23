//! Request-scoped forwarding: response IDs, progress and cancellation never cross clients.
use crate::{
    concurrency::{ActiveCall, CallFailure},
    config::{Config, Ownership},
    ownership::{BackendSlot, FailureGuard},
    recovery::Reason,
    worker::RequestPeer,
};
use rmcp::{
    Peer, RoleClient, RoleServer,
    model::*,
    service::{PeerRequestOptions, RequestContext, ServiceError},
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
    upstream: RequestPeer,
    backend: &Arc<BackendSlot>,
    config: &Config,
    call: &mut ActiveCall<'_>,
    request: ClientRequest,
    context: RequestContext<RoleServer>,
) -> Result<ServerResult, ErrorData> {
    let is_tool = matches!(&request, ClientRequest::CallToolRequest(_));
    let mut shared_guard = FailureGuard::new(
        (config.ownership == Ownership::Shared).then(|| backend.clone()),
        Reason::HandlerDropped,
    );
    let timeout = Duration::from_secs(config.call_timeout_seconds);
    let result = if config.max_concurrent_calls == 1 {
        forward(
            upstream,
            request,
            context,
            PeerRequestOptions::with_timeout(timeout),
        )
        .await
    } else {
        // Bound the whole opted-in dispatch, including peer backpressure and
        // progress forwarding, so a failed generation can always drain.
        let cancellation = context.ct.clone();
        tokio::select! {
            result = tokio::time::timeout(
                timeout,
                forward(upstream, request, context, PeerRequestOptions::no_options()),
            ) => result.unwrap_or(Err(ForwardError::Response(ServiceError::Timeout { timeout }))),
            _ = cancellation.cancelled() => Err(ForwardError::Cancelled),
        }
    };
    complete(result, backend, &mut shared_guard, call, is_tool)
}

enum ForwardError {
    Send,
    Cancelled,
    Response(ServiceError),
}

async fn forward(
    upstream: RequestPeer,
    request: ClientRequest,
    context: RequestContext<RoleServer>,
    options: PeerRequestOptions,
) -> Result<ServerResult, ForwardError> {
    let peer = upstream.peer;
    let bounded_dispatch = options.timeout.is_none();
    let mut progress = upstream.progress.subscribe();
    let downstream_progress: Option<ProgressToken> = context
        .meta
        .get("progressToken")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let handle = peer
        .send_request_with_option(request, options.with_meta(context.meta))
        .await
        .map_err(|_| ForwardError::Send)?;
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
                        let notify = context.peer.notify_progress(notification);
                        if bounded_dispatch {
                            // A blocked downstream progress write must not delay
                            // an upstream response or transport failure.
                            tokio::select! {
                                result = &mut response => break result,
                                _ = notify => {},
                            }
                        } else {
                            // A disconnected client can no longer receive progress.
                            let _ = notify.await;
                        }
                    }
            }
            _ = context.ct.cancelled() => {
                return Err(ForwardError::Cancelled);
            },
        }
    };
    if result.is_ok()
        || matches!(&result, Err(ServiceError::McpError(_)))
        || (bounded_dispatch && matches!(&result, Err(ServiceError::TransportClosed)))
    {
        cancel.id = None;
    }
    result.map_err(ForwardError::Response)
}

fn complete(
    result: Result<ServerResult, ForwardError>,
    backend: &Arc<BackendSlot>,
    shared_guard: &mut FailureGuard,
    call: &mut ActiveCall<'_>,
    is_tool: bool,
) -> Result<ServerResult, ErrorData> {
    match result {
        Ok(result) => {
            let tool_failed = matches!(&result, ServerResult::CallToolResult(tool) if tool.is_error == Some(true));
            call.failure = tool_failed.then_some(CallFailure::ToolError);
            shared_guard.disarm();
            Ok(result)
        }
        Err(ForwardError::Response(ServiceError::McpError(error))) => {
            call.failure = Some(CallFailure::JsonRpcError);
            shared_guard.disarm();
            Err(error)
        }
        Err(ForwardError::Send) => {
            call.failure = Some(CallFailure::Transport);
            backend.fail(Reason::TransportClosed);
            failure(
                is_tool,
                "Backend connection lost; the action was NOT retried and its outcome is unknown. Check status for recovery. Check network/VPN if required by the backend.",
            )
        }
        Err(ForwardError::Cancelled) => {
            shared_guard.reason = Reason::Cancelled;
            failure(
                is_tool,
                "Request cancelled. An in-flight tool action may already have occurred; it was not retried. Check status for recovery.",
            )
        }
        Err(ForwardError::Response(error)) => {
            let (reason, category) = match &error {
                ServiceError::Timeout { .. } => (Reason::Timeout, CallFailure::Timeout),
                ServiceError::Cancelled { .. } => (Reason::Cancelled, CallFailure::Cancelled),
                _ => (Reason::TransportClosed, CallFailure::Transport),
            };
            shared_guard.reason = reason;
            call.failure = Some(category);
            if matches!(
                error,
                ServiceError::TransportClosed | ServiceError::TransportSend(_)
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

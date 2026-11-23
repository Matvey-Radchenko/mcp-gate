//! Request-scoped forwarding: response IDs, progress and cancellation never cross clients.
use crate::{
    concurrency::{ActiveCall, CallFailure},
    http::ResponseCancellation,
    ownership::BackendSlot,
    recovery::Reason,
    worker::RequestPeer,
};
use rmcp::{
    Peer, RoleClient, RoleServer,
    model::*,
    service::{PeerRequestOptions, RequestContext, RequestHandle, ServiceError},
};
use std::{pin::Pin, sync::Arc, time::Duration};
use tokio::{
    sync::{OwnedSemaphorePermit, broadcast, oneshot},
    time::{Sleep, sleep},
};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests;

const CANCEL_REASON: &str = "Gateway request cancelled";
type DispatchResult = Result<ServerResult, ErrorData>;
type Response = Result<ServerResult, ServiceError>;

pub(crate) struct RequestCancellation {
    protocol: CancellationToken,
    disconnect: Option<CancellationToken>,
}

impl RequestCancellation {
    pub fn new(context: &RequestContext<RoleServer>) -> Self {
        Self {
            protocol: context.ct.clone(),
            disconnect: context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|p| p.extensions.get::<ResponseCancellation>())
                .map(|disconnect| disconnect.0.clone()),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.protocol.is_cancelled()
            || self
                .disconnect
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
    }

    pub async fn cancelled(&self) {
        let Some(disconnect) = &self.disconnect else {
            return self.protocol.cancelled().await;
        };
        tokio::select! {
            () = self.protocol.cancelled() => {},
            () = disconnect.cancelled() => {},
        }
    }
}

// Session-owned calls may abandon the reply when their handler ends.
struct CancelOnDrop {
    peer: Peer<RoleClient>,
    id: Option<RequestId>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            cancel_without_draining(self.peer.clone(), id);
        }
    }
}

fn cancel_without_draining(peer: Peer<RoleClient>, id: RequestId) {
    tokio::spawn(async move {
        if peer
            .notify_cancelled(CancelledNotificationParam::new(
                Some(id),
                Some(CANCEL_REASON.into()),
            ))
            .await
            .is_err()
        {
            tracing::warn!("Upstream cancellation notification could not be delivered");
        }
    });
}

pub(crate) async fn invoke_session(
    upstream: RequestPeer,
    backend: &Arc<BackendSlot>,
    mut call: ActiveCall,
    timeout: Duration,
    request: ClientRequest,
    context: RequestContext<RoleServer>,
) -> DispatchResult {
    let is_tool = matches!(&request, ClientRequest::CallToolRequest(_));
    let progress = Progress::new(&upstream, &context);
    let cancellation = context.ct;
    let result = async {
        let handle = send_request(
            &upstream.peer,
            request,
            PeerRequestOptions::with_timeout(timeout).with_meta(context.meta),
        )
        .await?;
        let mut cancel = CancelOnDrop {
            peer: upstream.peer,
            id: Some(handle.id.clone()),
        };
        let progress_token = handle.progress_token.clone();
        let mut response = std::pin::pin!(handle.await_response());
        let result = tokio::select! {
            result = progress.wait(response.as_mut(), progress_token) => result,
            () = cancellation.cancelled() => return Err(ForwardError::Cancelled),
        };
        if result.is_ok() || matches!(&result, Err(ServiceError::McpError(_))) {
            cancel.id = None;
        }
        result.map_err(ForwardError::Response)
    }
    .await;
    let result = complete(result, backend, &mut call, is_tool, false);
    backend.touch();
    result
}

pub(crate) async fn invoke_shared(
    upstream: RequestPeer,
    backend: Arc<BackendSlot>,
    mut call: ActiveCall,
    admission: OwnedSemaphorePermit,
    timeout: Duration,
    request: ClientRequest,
    context: RequestContext<RoleServer>,
) -> DispatchResult {
    let is_tool = matches!(&request, ClientRequest::CallToolRequest(_));
    let cancellation = RequestCancellation::new(&context);
    let deadline = sleep(timeout);
    let (mut reply, result) = oneshot::channel();
    // The slot owns both permits until the upstream request settles. Dropping
    // the frontend receiver signals cancellation without dropping that work.
    tokio::spawn(async move {
        let result = tokio::select! {
            biased;
            () = backend.closing.cancelled() => None,
            result = forward_shared(upstream, request, context, deadline, timeout, &mut reply) => result,
        };
        let result = result.map(|result| complete(result, &backend, &mut call, is_tool, true));
        // Start the idle interval when the backend settles, including after cancellation.
        backend.touch();
        drop(call);
        drop(admission);
        if let Some(result) = result {
            // A cancelled frontend has already dropped its receiver.
            drop(reply.send(result));
        }
    });
    tokio::select! {
        biased;
        () = cancellation.cancelled() => cancelled(is_tool),
        result = result => result.unwrap_or_else(|_| failure(is_tool, "Backend dispatch ended; the action was NOT retried.")),
    }
}

async fn forward_shared(
    upstream: RequestPeer,
    request: ClientRequest,
    mut context: RequestContext<RoleServer>,
    deadline: Sleep,
    timeout: Duration,
    reply: &mut oneshot::Sender<DispatchResult>,
) -> Option<Result<ServerResult, ForwardError>> {
    let progress = Progress::new(&upstream, &context);
    let cancellation = RequestCancellation::new(&context);
    let send = send_request(
        &upstream.peer,
        request,
        PeerRequestOptions::no_options().with_meta(std::mem::take(&mut context.meta)),
    );
    // The HTTP lease belongs to the frontend handler and its response body.
    // Release the task's copy of the context before waiting on the backend.
    drop(context);
    tokio::pin!(deadline);
    let handle = tokio::select! {
        biased;
        () = reply.closed() => return None,
        () = cancellation.cancelled() => return None,
        result = send => match result {
            Ok(handle) => handle,
            Err(error) => return Some(Err(error)),
        },
        () = &mut deadline => return Some(Err(ForwardError::Response(ServiceError::Timeout { timeout }))),
    };
    let id = handle.id.clone();
    let progress_token = handle.progress_token.clone();
    // Use one deadline for dispatch and drain. The SDK's timed handle would
    // send another cancellation and wait for that write before reporting timeout.
    let mut response = std::pin::pin!(async {
        tokio::select! {
            biased;
            result = handle.await_response() => result,
            () = &mut deadline => Err(ServiceError::Timeout { timeout }),
        }
    });
    let result = tokio::select! {
        biased;
        () = reply.closed() => drain_cancelled(upstream.peer, id, response.as_mut()).await,
        result = progress.wait(response.as_mut(), progress_token) => {
            if matches!(result, Err(ServiceError::Timeout { .. })) {
                cancel_without_draining(upstream.peer, id);
            }
            result
        },
    };
    Some(result.map_err(ForwardError::Response))
}

async fn drain_cancelled(
    peer: Peer<RoleClient>,
    id: RequestId,
    mut response: Pin<&mut impl Future<Output = Response>>,
) -> Response {
    // rmcp's typed outbound cancellation also removes its local response
    // waiter. Send the same wire notification while keeping that waiter alive.
    let notification = ClientNotification::CustomNotification(CustomNotification::new(
        "notifications/cancelled",
        Some(serde_json::json!({"requestId": id, "reason": CANCEL_REASON})),
    ));
    tokio::select! {
        biased;
        result = &mut response => result,
        result = peer.send_notification(notification) => {
            if result.is_err() {
                tracing::warn!("Upstream cancellation notification could not be delivered");
            }
            response.await
        },
    }
}

async fn send_request(
    peer: &Peer<RoleClient>,
    request: ClientRequest,
    options: PeerRequestOptions,
) -> Result<RequestHandle<RoleClient>, ForwardError> {
    peer.send_request_with_option(request, options)
        .await
        .map_err(|_| ForwardError::Send)
}

struct Progress {
    source: broadcast::Receiver<ProgressNotificationParam>,
    downstream: Peer<RoleServer>,
    token: Option<ProgressToken>,
}

impl Progress {
    fn new(upstream: &RequestPeer, context: &RequestContext<RoleServer>) -> Self {
        Self {
            source: upstream.progress.subscribe(),
            downstream: context.peer.clone(),
            token: context.meta.get_progress_token(),
        }
    }

    async fn wait(
        mut self,
        mut response: Pin<&mut impl Future<Output = Response>>,
        upstream_token: ProgressToken,
    ) -> Response {
        let Some(downstream_token) = self.token else {
            return response.await;
        };
        loop {
            let mut progress = tokio::select! {
                result = &mut response => return result,
                event = self.source.recv() => match event {
                    Ok(progress) if progress.progress_token == upstream_token => progress,
                    Err(broadcast::error::RecvError::Closed) => return response.await,
                    _ => continue, // Other requests' progress or a lagging receiver.
                },
            };
            progress.progress_token = downstream_token.clone();
            // A blocked downstream write cannot delay the upstream response.
            tokio::select! {
                result = &mut response => return result,
                result = self.downstream.notify_progress(progress) => {
                    if result.is_err() {
                        return response.await;
                    }
                },
            }
        }
    }
}

enum ForwardError {
    Send,
    Cancelled,
    Response(ServiceError),
}

fn complete(
    result: Result<ServerResult, ForwardError>,
    backend: &Arc<BackendSlot>,
    call: &mut ActiveCall,
    is_tool: bool,
    shared: bool,
) -> DispatchResult {
    match result {
        Ok(result) => {
            let tool_failed = matches!(&result, ServerResult::CallToolResult(tool) if tool.is_error == Some(true));
            call.failure = tool_failed.then_some(CallFailure::ToolError);
            Ok(result)
        }
        Err(ForwardError::Response(ServiceError::McpError(error))) => {
            call.failure = Some(CallFailure::JsonRpcError);
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
        Err(ForwardError::Cancelled) => cancelled(is_tool),
        Err(ForwardError::Response(error)) => {
            let (reason, category) = match &error {
                ServiceError::Timeout { .. } => (Reason::Timeout, CallFailure::Timeout),
                ServiceError::Cancelled { .. } => (Reason::Cancelled, CallFailure::Cancelled),
                _ => (Reason::TransportClosed, CallFailure::Transport),
            };
            call.failure = Some(category);
            if shared
                || matches!(
                    error,
                    ServiceError::TransportClosed | ServiceError::TransportSend(_)
                )
            {
                backend.fail(reason);
            }
            failure(
                is_tool,
                "Backend request failed or timed out. An action may already have occurred; it was NOT retried. Check status for recovery. Check network/VPN if required by the backend.",
            )
        }
    }
}

fn cancelled(is_tool: bool) -> DispatchResult {
    failure(
        is_tool,
        "Request cancelled. An in-flight tool action may already have occurred; it was not retried.",
    )
}

/// Tools use isError results; other MCP methods use JSON-RPC errors.
pub(crate) fn failure(is_tool: bool, message: &str) -> DispatchResult {
    if is_tool {
        Ok(ServerResult::CallToolResult(CallToolResult::error(vec![
            ContentBlock::text(message),
        ])))
    } else {
        Err(ErrorData::internal_error(message.to_owned(), None))
    }
}

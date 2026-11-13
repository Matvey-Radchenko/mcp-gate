//! Authentication, DNS-rebinding protection and HTTP connection leases.
//! MCP framing, negotiation, SSE and session IDs belong to the official SDK.
use crate::gateway::Gateway;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use http_body::{Body as HttpBody, Frame, SizeHint};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::{SessionManager, local::LocalSessionManager},
};
use serde_json::json;
use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

struct Lease {
    streams: usize,
    requests: usize,
    disconnected: Option<Instant>,
    closing: bool,
    active: Arc<std::sync::atomic::AtomicUsize>,
    _capacity: Arc<OwnedSemaphorePermit>,
}
type LeaseRef = Arc<Mutex<Lease>>;
#[derive(Clone)]
struct HttpState {
    gateway: Arc<Gateway>,
    expected_auth: Arc<Vec<u8>>,
    host: String,
    leases: Arc<Mutex<HashMap<String, LeaseRef>>>,
    capacity: Arc<Semaphore>,
    maintenance: Arc<Mutex<Option<OwnedSemaphorePermit>>>,
}
pub struct Server {
    pub router: Router,
    pub cancellation: CancellationToken,
    manager: Arc<LocalSessionManager>,
}
impl Server {
    pub fn new(gateway: Arc<Gateway>, token: String) -> Self {
        let cancellation = CancellationToken::new();
        let mut manager = LocalSessionManager::default();
        // A quiet, connected session may hold a performance recording. A generic
        // idle timeout would destroy it. Only explicit termination/lost GET cleans it.
        manager.session_config.keep_alive = None;
        manager.session_config.completed_cache_ttl = Duration::from_secs(15);
        let manager = Arc::new(manager);
        let mut config = StreamableHttpServerConfig::default();
        config.cancellation_token = cancellation.clone();
        config.allowed_hosts = vec![gateway.config.listen.to_string()];
        config.max_request_body_bytes = 4 * 1024 * 1024;
        let factory = gateway.clone();
        let service =
            StreamableHttpService::new(move || factory.handler(), manager.clone(), config);
        let state = HttpState {
            host: gateway.config.listen.to_string(),
            expected_auth: Arc::new(format!("Bearer {token}").into_bytes()),
            capacity: Arc::new(Semaphore::new(gateway.config.max_sessions)),
            maintenance: Arc::default(),
            gateway,
            leases: Arc::new(Mutex::new(HashMap::new())),
        };
        let clean_state = state.clone();
        let clean_manager = manager.clone();
        let clean_cancel = cancellation.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! { _ = clean_cancel.cancelled() => break, _ = tick.tick() => {} }
                let expired: Vec<_> = {
                    let leases = clean_state.leases.lock().unwrap();
                    leases
                        .iter()
                        .filter_map(|(id, lease)| {
                            let mut lease = lease.lock().unwrap();
                            if !lease.closing
                                && lease.streams == 0
                                && lease.requests == 0
                                && lease.active.load(std::sync::atomic::Ordering::SeqCst) == 0
                                && lease.disconnected.is_some_and(|t| {
                                    t.elapsed()
                                        >= Duration::from_secs(
                                            clean_state.gateway.config.disconnect_grace_seconds,
                                        )
                                })
                            {
                                lease.closing = true;
                                Some(id.clone())
                            } else {
                                None
                            }
                        })
                        .collect()
                };
                for id in expired {
                    let _ = clean_manager.close_session(&id.clone().into()).await;
                    clean_state.leases.lock().unwrap().remove(&id);
                    tracing::info!("disconnected session expired");
                }
            }
        });
        let router = Router::new()
            .route("/health", get(health))
            .route("/admin/maintenance", post(maintenance).delete(resume))
            .nest_service("/mcp", service)
            .with_state(state.clone())
            .layer(middleware::from_fn_with_state(state, guard));
        Self {
            router,
            cancellation,
            manager,
        }
    }
    pub async fn shutdown(&self) {
        self.cancellation.cancel();
        let ids: Vec<_> = self.manager.sessions.read().await.keys().cloned().collect();
        futures::future::join_all(ids.iter().map(|id| self.manager.close_session(id))).await;
    }
}
// Holding every permit atomically excludes live sessions, initialization and active handlers.
async fn maintenance(State(state): State<HttpState>) -> StatusCode {
    let mut held = state.maintenance.lock().unwrap();
    if held.is_some() {
        return StatusCode::OK;
    }
    let Ok(count) = u32::try_from(state.gateway.config.max_sessions) else {
        return StatusCode::CONFLICT;
    };
    match state.capacity.clone().try_acquire_many_owned(count) {
        Ok(permit) => {
            *held = Some(permit);
            StatusCode::OK
        }
        Err(_) => StatusCode::CONFLICT,
    }
}
async fn resume(State(state): State<HttpState>) -> StatusCode {
    state.maintenance.lock().unwrap().take();
    StatusCode::OK
}
async fn health(State(state): State<HttpState>) -> impl IntoResponse {
    let details: Vec<_> = state
        .leases
        .lock()
        .unwrap()
        .iter()
        .map(|(id, l)| {
            let l = l.lock().unwrap();
            json!({"id":id,"streams":l.streams,"http_requests":l.requests,
            "active_calls":l.active.load(std::sync::atomic::Ordering::SeqCst),
            "disconnected_seconds":l.disconnected.map(|t|t.elapsed().as_secs())})
        })
        .collect();
    axum::Json(
        json!({ "service": "mcp-gate", "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(), "sessions": state.leases.lock().unwrap().len(),
        "maintenance":state.maintenance.lock().unwrap().is_some(),
        "workers": state.gateway.live.load(std::sync::atomic::Ordering::SeqCst),
        "max_workers": state.gateway.config.max_workers, "tools": state.gateway.catalog.tools.len(),
        "runtime": state.gateway.runtime_status(),
        "backend_version": state.gateway.catalog.backend_version, "session_details":details }),
    )
}
async fn guard(State(state): State<HttpState>, mut req: Request, next: Next) -> Response {
    for header in [
        "authorization",
        "host",
        "mcp-session-id",
        "mcp-protocol-version",
    ] {
        if req.headers().get_all(header).iter().count() > 1 {
            return StatusCode::BAD_REQUEST.into_response();
        }
    }
    let auth = req
        .headers()
        .get("authorization")
        .map(|h| h.as_bytes())
        .unwrap_or_default();
    if auth.len() != state.expected_auth.len() || !bool::from(auth.ct_eq(&state.expected_auth)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.headers().contains_key("origin")
        || req.headers().get("host").and_then(|h| h.to_str().ok()) != Some(&state.host)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if req.uri().query().is_none() && matches!(req.uri().path(), "/health" | "/admin/maintenance") {
        return next.run(req).await;
    }
    if req.uri().path() != "/mcp" || req.uri().query().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(version) = req.headers().get("mcp-protocol-version")
        && !["2025-03-26", "2025-06-18", "2025-11-25"]
            .iter()
            .any(|v| version == *v)
    {
        return (
            StatusCode::BAD_REQUEST,
            "Session gateway requires a session-based MCP version",
        )
            .into_response();
    }
    let id = req
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let method = req.method().clone();
    let mut capacity = None;
    let mut body_guard = None;
    if let Some(id) = &id {
        let lease = state.leases.lock().unwrap().get(id).cloned();
        let Some(lease) = lease else {
            return StatusCode::NOT_FOUND.into_response();
        };
        {
            let mut value = lease.lock().unwrap();
            if value.closing {
                return StatusCode::NOT_FOUND.into_response();
            }
            req.extensions_mut()
                .insert(Activity(value.active.clone(), value._capacity.clone()));
            if method == Method::GET {
                value.streams += 1;
                value.disconnected = None;
            } else {
                value.requests += 1;
                if value.disconnected.is_some() {
                    value.disconnected = Some(Instant::now());
                }
            }
        }
        body_guard = Some(ConnectionGuard {
            lease,
            stream: method == Method::GET,
        });
    } else {
        if method != Method::POST {
            return StatusCode::BAD_REQUEST.into_response();
        }
        match state.capacity.clone().try_acquire_owned() {
            Ok(permit) => capacity = Some(permit),
            Err(_) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "Session capacity reached")
                    .into_response();
            }
        }
    }
    let response = next.run(req).await;
    if let Some(permit) = capacity
        && response.status().is_success()
        && let Some(id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|h| h.to_str().ok())
    {
        state.leases.lock().unwrap().insert(
            id.into(),
            Arc::new(Mutex::new(Lease {
                streams: 0,
                requests: 0,
                disconnected: None,
                closing: false,
                active: Arc::default(),
                _capacity: Arc::new(permit),
            })),
        );
    }
    if method == Method::DELETE
        && response.status().is_success()
        && let Some(id) = id
    {
        state.leases.lock().unwrap().remove(&id);
    }
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::new(TrackedBody {
            body,
            _guard: body_guard,
        }),
    )
}
struct ConnectionGuard {
    lease: LeaseRef,
    stream: bool,
}
#[derive(Clone)]
pub(crate) struct Activity(
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<OwnedSemaphorePermit>,
);
impl Activity {
    pub fn start(&self) -> ActiveCall {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ActiveCall(self.0.clone(), self.1.clone())
    }
}
pub(crate) struct ActiveCall(
    Arc<std::sync::atomic::AtomicUsize>,
    #[allow(
        dead_code,
        reason = "Permit lifetime covers executable handlers after HTTP disconnect"
    )]
    Arc<OwnedSemaphorePermit>,
);
impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut value = self.lease.lock().unwrap();
        if self.stream {
            value.streams -= 1;
            if value.streams == 0 {
                value.disconnected = Some(Instant::now());
            }
        } else {
            value.requests -= 1;
            if value.disconnected.is_some() {
                value.disconnected = Some(Instant::now());
            }
        }
    }
}
struct TrackedBody {
    body: Body,
    _guard: Option<ConnectionGuard>,
}
impl HttpBody for TrackedBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

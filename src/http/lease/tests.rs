use super::*;
use crate::{catalog::Catalog, config::Config, gateway::Gateway, http::Server};
use axum::{
    ServiceExt,
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use std::convert::Infallible;
use tokio::sync::Semaphore;

fn lease(ownership: Ownership) -> (Arc<Semaphore>, LeaseRef) {
    let capacity = Arc::new(Semaphore::new(1));
    let lease = Arc::new(Mutex::new(Lease::new(
        ownership,
        capacity.clone().try_acquire_owned().unwrap(),
    )));
    lease.lock().unwrap().last_activity -= Duration::from_millis(1);
    (capacity, lease)
}

#[test]
fn completed_discovery_does_not_extend_idle_but_executable_completion_does() {
    let (_, lease) = lease(Ownership::Session);
    let before = lease.lock().unwrap().last_activity;
    lease.lock().unwrap().requests = 1;
    drop(ConnectionGuard {
        lease: lease.clone(),
        stream: false,
    });
    assert_eq!(lease.lock().unwrap().last_activity, before);
    assert_eq!(
        lease.lock().unwrap().expiry(
            before + Duration::from_secs(30),
            Some(Duration::from_secs(10)),
            Duration::from_secs(60)
        ),
        Some("idle")
    );
    let activity = Activity(lease.clone());
    let running = activity.start().unwrap();
    let queued = activity.start().unwrap();
    assert!(lease.lock().unwrap().last_activity > before);
    assert_eq!(
        lease
            .lock()
            .unwrap()
            .expiry(Instant::now(), Some(Duration::ZERO), Duration::ZERO),
        None
    );
    drop(running);
    assert_eq!(lease.lock().unwrap().active, 1);
    drop(queued);
    assert_eq!(lease.lock().unwrap().active, 0);
    assert_eq!(
        lease.lock().unwrap().expiry(
            Instant::now(),
            Some(Duration::from_secs(10)),
            Duration::from_secs(60)
        ),
        None
    );
}

#[test]
fn open_get_stream_protects_recordings_and_closure_refreshes_idle() {
    let (_, lease) = lease(Ownership::Session);
    lease.lock().unwrap().streams = 1;
    assert_eq!(
        lease
            .lock()
            .unwrap()
            .expiry(Instant::now(), Some(Duration::ZERO), Duration::ZERO),
        None
    );
    let before = lease.lock().unwrap().last_activity;
    drop(ConnectionGuard {
        lease: lease.clone(),
        stream: true,
    });
    let lease = lease.lock().unwrap();
    assert!(lease.last_activity > before);
    assert_eq!(lease.streams, 0);
    assert!(lease.disconnected.is_some());
    assert_eq!(
        lease.expiry(
            Instant::now(),
            Some(Duration::from_secs(10)),
            Duration::from_secs(60)
        ),
        None
    );
}

#[test]
fn disabled_idle_preserves_disconnect_grace_and_handler_capacity_ownership() {
    let (capacity, lease) = lease(Ownership::Session);
    let now = Instant::now();
    assert_eq!(
        lease.lock().unwrap().expiry(now, None, Duration::ZERO),
        None
    );
    lease.lock().unwrap().disconnected = Some(now);
    assert_eq!(
        lease
            .lock()
            .unwrap()
            .expiry(now + Duration::from_secs(30), None, Duration::from_secs(10)),
        Some("disconnected")
    );
    let activity = Activity(lease.clone());
    let running = activity.start().unwrap();
    drop(activity);
    drop(lease);
    assert_eq!(capacity.available_permits(), 0);
    drop(running);
    assert_eq!(capacity.available_permits(), 1);
}

#[test]
fn shared_activity_protects_the_lease_and_request_completion_refreshes_its_ttl() {
    let (_, lease) = lease(Ownership::Shared);
    let activity = Activity(lease.clone());
    let running = activity.start().unwrap();
    let later = Instant::now() + SHARED_IDLE_TTL;
    let expiry = || lease.lock().unwrap().expiry(later, None, Duration::ZERO);
    assert_eq!(expiry(), None);
    drop(running);

    lease.lock().unwrap().streams = 1;
    assert_eq!(expiry(), None);
    let mut value = lease.lock().unwrap();
    value.streams = 0;
    value.requests = 1;
    value.last_activity -= Duration::from_millis(1);
    let original_deadline = value.last_activity + SHARED_IDLE_TTL;
    drop(value);
    assert_eq!(expiry(), None);
    let request = ConnectionGuard {
        lease: lease.clone(),
        stream: false,
    };
    drop(request);
    let mut value = lease.lock().unwrap();
    assert_eq!(value.expiry(original_deadline, None, Duration::ZERO), None);
    value.mark_closing();
    drop(value);
    assert!(activity.start().is_none());
}

const HOST: &str = "127.0.0.1:12345";

fn shared_server() -> Server {
    let config: Config = serde_json::from_value(json!({
        "format_version": 2, "ownership": "shared", "listen": HOST,
        "token_file": "unused", "catalog_file": "unused", "state_dir": "unused",
        "max_workers": 1, "max_sessions": 1, "idle_seconds": 1,
        "backend": {"profile": "stdio", "command": "unused", "version": "fixture"}
    }))
    .unwrap();
    let catalog = Catalog {
        format_version: 3,
        backend_version: "fixture".into(),
        entrypoint_sha256: String::new(),
        args: vec![],
        server_info: Default::default(),
        tools: vec![],
        resources: vec![],
        resource_templates: vec![],
        prompts: vec![],
        invocation_sha256: None,
    };
    Server::new(Gateway::new(config, catalog), "test-token".into())
}

async fn route(
    service: &mut impl ServiceExt<Request, Response = Response, Error = Infallible>,
    method: Method,
    path: &str,
    id: Option<&str>,
    payload: Option<Value>,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", HOST)
        .header("authorization", "Bearer test-token")
        .header("accept", "application/json, text/event-stream");
    if let Some(id) = id {
        request = request
            .header("mcp-session-id", id)
            .header("mcp-protocol-version", "2025-11-25");
    }
    let body = if let Some(payload) = payload {
        request = request.header("content-type", "application/json");
        Body::from(payload.to_string())
    } else {
        Body::empty()
    };
    service.call(request.body(body).unwrap()).await.unwrap()
}

async fn initialize_response(router: &mut axum::Router) -> Response {
    let params = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}
    }});
    route(router, Method::POST, "/mcp", None, Some(params)).await
}

async fn initialize(router: &mut axum::Router) -> String {
    let response = initialize_response(router).await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let response = route(router, Method::POST, "/mcp", Some(&id), Some(initialized)).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    id
}

async fn maintenance(router: &mut axum::Router, method: Method) -> StatusCode {
    route(router, method, "/admin/maintenance", None, None)
        .await
        .status()
}

#[tokio::test]
async fn shared_session_reaps_at_http_ttl_and_its_id_returns_404() {
    let mut server = shared_server();
    let id = initialize(&mut server.router).await;

    let state = &server.state;
    let lease = state.leases.lock().unwrap()[&id].clone();
    let (deadline, cancellation) = {
        let lease = lease.lock().unwrap();
        (
            lease.last_activity + SHARED_IDLE_TTL,
            lease.termination.child_token(),
        )
    };
    let sdk_id = id.clone().into();
    reap_sessions(state, deadline - Duration::from_secs(1)).await;
    assert!(state.manager.has_session(&sdk_id).await.unwrap());
    assert_eq!(state.capacity.available_permits(), 0);
    assert!(!cancellation.is_cancelled());
    reap_sessions(state, deadline).await;
    assert!(!state.manager.has_session(&sdk_id).await.unwrap());
    assert_eq!(state.capacity.available_permits(), 1);
    assert!(state.leases.lock().unwrap().is_empty());
    assert!(cancellation.is_cancelled());
    assert!(Activity(lease).start().is_none());

    let ping = json!({"jsonrpc":"2.0","id":1,"method":"ping"});
    let response = route(
        &mut server.router,
        Method::POST,
        "/mcp",
        Some(&id),
        Some(ping),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_ne!(initialize(&mut server.router).await, id);
    server.shutdown().await;
}

#[tokio::test]
async fn shared_maintenance_conflicts_while_busy_and_closes_the_lease_when_idle() {
    let mut server = shared_server();
    let id = initialize(&mut server.router).await;
    let lease = server.state.leases.lock().unwrap()[&id].clone();
    let cancellation = lease.lock().unwrap().termination.child_token();
    let running = Activity(lease.clone()).start().unwrap();
    let router = &mut server.router;
    assert_eq!(
        maintenance(router, Method::POST).await,
        StatusCode::CONFLICT
    );
    drop(running);

    let stream = route(router, Method::GET, "/mcp", Some(&id), None).await;
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(
        maintenance(router, Method::POST).await,
        StatusCode::CONFLICT
    );
    drop(stream);

    assert!(!cancellation.is_cancelled());
    assert_eq!(maintenance(router, Method::POST).await, StatusCode::OK);
    assert!(cancellation.is_cancelled());
    assert!(Activity(lease).start().is_none());
    assert_eq!(
        route(router, Method::GET, "/mcp", Some(&id), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(server.state.capacity.available_permits(), 0);
    assert_eq!(
        route(router, Method::POST, "/mcp", None, None)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(maintenance(router, Method::DELETE).await, StatusCode::OK);
    assert_ne!(initialize(router).await, id);
    server.shutdown().await;
}

#[tokio::test]
async fn shared_maintenance_waits_for_the_initialization_response_to_finish() {
    let mut server = shared_server();
    let response = initialize_response(&mut server.router).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        maintenance(&mut server.router, Method::POST).await,
        StatusCode::CONFLICT
    );
    drop(response);
    assert_eq!(
        maintenance(&mut server.router, Method::POST).await,
        StatusCode::OK
    );
    server.shutdown().await;
}

#[tokio::test]
async fn delete_terminates_the_lease_and_keeps_capacity_for_active_handlers() {
    let mut server = shared_server();
    let id = initialize(&mut server.router).await;
    let state = &server.state;
    let lease = state.leases.lock().unwrap()[&id].clone();
    let cancellation = lease.lock().unwrap().termination.child_token();
    let activity = Activity(lease);
    let running = activity.start().unwrap();

    let response = route(&mut server.router, Method::DELETE, "/mcp", Some(&id), None).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    drop(response);
    assert!(state.leases.lock().unwrap().is_empty());
    assert!(cancellation.is_cancelled());
    assert!(activity.start().is_none());
    assert_eq!(state.capacity.available_permits(), 0);
    drop(running);
    assert_eq!(state.capacity.available_permits(), 1);
    drop(activity);
    server.shutdown().await;
}

#[tokio::test]
async fn tracked_body_cancels_only_on_incomplete_drop() {
    let session = CancellationToken::new();
    let abandoned = session.child_token();
    drop(TrackedBody::new(
        Body::from("unread"),
        None,
        Some(abandoned.clone().drop_guard()),
    ));
    assert!(abandoned.is_cancelled());
    assert!(!session.is_cancelled());

    let completed = session.child_token();
    let mut body = TrackedBody::new(
        Body::from("read"),
        None,
        Some(completed.clone().drop_guard()),
    );
    let frame = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
    assert!(matches!(frame, Some(Ok(_))));
    drop(body);
    assert!(!completed.is_cancelled());
    session.cancel();
    assert!(completed.is_cancelled());
}

use super::*;
use serde_json::json;

#[test]
fn routes_accepted_json_and_sse_responses() {
    let response = |status, content_type| response_kind(status, content_type).unwrap();
    assert_eq!(response(StatusCode::ACCEPTED, None), Kind::Empty);
    assert_eq!(response(StatusCode::ACCEPTED, Some(SSE)), Kind::Empty);
    assert_eq!(
        response(StatusCode::OK, Some("Application/JSON; charset=UTF-8")),
        Kind::Json
    );
    assert_eq!(
        response(StatusCode::OK, Some("text/event-stream; charset=utf-8")),
        Kind::Events
    );
    assert!(response_kind(StatusCode::OK, Some("text/plain")).is_err());
}

#[tokio::test]
async fn reads_json_and_sse_bodies_into_discrete_messages() {
    let response = |kind, body| {
        axum::http::Response::builder()
            .header(CONTENT_TYPE, kind)
            .body(body)
            .unwrap()
            .into()
    };
    let payload = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
    let expected = json!({"jsonrpc":"2.0","id":1,"result":{}});
    let mut json = Incoming::from_response(response(JSON, payload.to_owned())).unwrap();
    assert_eq!(json.next().await.unwrap(), Some(expected.clone()));
    assert_eq!(json.next().await.unwrap(), None);

    let events = format!("data: {payload}\n\ndata: {payload}\n\n");
    let mut stream = Incoming::from_response(response(SSE, events)).unwrap();
    assert_eq!(stream.next().await.unwrap(), Some(expected.clone()));
    assert_eq!(stream.next().await.unwrap(), Some(expected));
    assert_eq!(stream.next().await.unwrap(), None);
}

#[test]
fn attaches_only_session_headers_acquired_during_initialization() {
    let connection = Connection::new("127.0.0.1:1234".parse().unwrap(), "fixture".into()).unwrap();
    let mut session = Session::default();
    let build = |session: &Session, method| connection.request(method, session).build().unwrap();

    let first = build(&session, Method::POST);
    assert!(!first.headers().contains_key(SESSION_ID));
    assert!(!first.headers().contains_key(PROTOCOL_VERSION));
    assert!(!session.started());

    let mut headers = HeaderMap::new();
    headers.insert(SESSION_ID, HeaderValue::from_static("fixture-session"));
    session.capture(&headers).unwrap();
    assert!(session.started());
    assert!(!session.negotiated());
    session.negotiate(Some("2025-06-18")).unwrap();
    for method in [Method::POST, Method::GET, Method::DELETE] {
        let request = build(&session, method);
        assert_eq!(request.headers()[SESSION_ID], "fixture-session");
        assert_eq!(request.headers()[PROTOCOL_VERSION], "2025-06-18");
        assert_eq!(request.headers()["authorization"], "Bearer fixture");
    }
    assert!(
        check_status(StatusCode::NOT_FOUND, &session)
            .unwrap_err()
            .contains("session expired")
    );
    assert_eq!(
        check_status(StatusCode::NOT_FOUND, &Session::default()).unwrap_err(),
        "Gateway returned HTTP 404"
    );
}

#[test]
fn accepts_stateless_sessions_and_rejects_malformed_headers() {
    let mut session = Session::default();
    session.capture(&HeaderMap::new()).unwrap();
    assert!(session.negotiate(None).is_err());
    assert!(session.negotiate(Some("malformed\r\nversion")).is_err());
    session.negotiate(Some("2025-03-26")).unwrap();
    assert!(session.negotiated());
    assert!(session.id.is_none());

    let mut headers = HeaderMap::new();
    headers.insert(SESSION_ID, HeaderValue::from_static("not visible"));
    assert!(session.capture(&headers).is_err());
}

//! Streamable HTTP requests and response framing. Errors expose only local
//! diagnostics and status codes, since upstream bodies can contain secrets.
use super::sse;
use axum::body::Bytes;
use reqwest::{
    Client, Method, RequestBuilder, Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::Value;
use std::{net::SocketAddr, time::Duration};

const SESSION_ID: &str = "mcp-session-id";
const PROTOCOL_VERSION: &str = "mcp-protocol-version";
const JSON: &str = "application/json";
const SSE: &str = "text/event-stream";

#[derive(Clone, Default)]
pub(super) struct Session {
    id: Option<HeaderValue>,
    version: Option<HeaderValue>,
}

impl Session {
    pub(super) fn started(&self) -> bool {
        self.id.is_some() || self.negotiated()
    }

    pub(super) fn negotiated(&self) -> bool {
        self.version.is_some()
    }

    pub(super) fn capture(&mut self, headers: &HeaderMap) -> Result<(), String> {
        if let Some(id) = headers.get(SESSION_ID) {
            if id.is_empty() || !id.as_bytes().iter().all(u8::is_ascii_graphic) {
                return Err("Gateway returned an invalid MCP session ID".into());
            }
            self.id = Some(id.clone());
        }
        Ok(())
    }

    pub(super) fn negotiate(&mut self, version: Option<&str>) -> Result<(), String> {
        self.version = version
            .filter(|version| !version.is_empty())
            .and_then(|version| HeaderValue::from_str(version).ok());
        self.version.as_ref().map(|_| ()).ok_or_else(|| {
            "Gateway initialize result did not include a valid MCP protocol version".into()
        })
    }

    fn apply(&self, mut request: RequestBuilder) -> RequestBuilder {
        if let Some(id) = &self.id {
            request = request.header(SESSION_ID, id);
        }
        if let Some(version) = &self.version {
            request = request.header(PROTOCOL_VERSION, version);
        }
        request
    }
}

#[derive(Clone)]
pub(super) struct Connection {
    client: Client,
    endpoint: String,
    token: String,
}

impl Connection {
    pub(super) fn new(listen: SocketAddr, token: String) -> anyhow::Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .tcp_nodelay(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            client,
            endpoint: format!("http://{listen}/mcp"),
            token,
        })
    }

    fn request(&self, method: Method, session: &Session) -> RequestBuilder {
        session.apply(
            self.client
                .request(method, &self.endpoint)
                .bearer_auth(&self.token),
        )
    }

    pub(super) async fn post(
        &self,
        session: &Session,
        message: &Value,
    ) -> Result<Response, String> {
        let response = self
            .request(Method::POST, session)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(message)
            .send()
            .await
            .map_err(connection_error)?;
        check_status(response.status(), session)?;
        Ok(response)
    }

    pub(super) async fn listen(&self, session: &Session) -> Result<Incoming, String> {
        let response = self
            .request(Method::GET, session)
            .header(ACCEPT, SSE)
            .send()
            .await
            .map_err(connection_error)?;
        match response.status() {
            StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_IMPLEMENTED
            | StatusCode::NO_CONTENT => {
                return Ok(Incoming::Empty);
            }
            status => check_status(status, session)?,
        }
        match Incoming::from_response(response)? {
            stream @ (Incoming::Events(_) | Incoming::Empty) => Ok(stream),
            Incoming::Json(_) => Err("Gateway GET response did not contain an SSE stream".into()),
        }
    }

    pub(super) async fn close(&self, session: &Session) -> Result<(), String> {
        if session.id.is_none() {
            return Ok(());
        }
        let response = self
            .request(Method::DELETE, session)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(connection_error)?;
        if response.status() != StatusCode::METHOD_NOT_ALLOWED {
            check_status(response.status(), session)?;
        }
        Ok(())
    }
}

fn connection_error(error: reqwest::Error) -> String {
    if error.is_connect() {
        "Cannot reach the gateway. Run mcp-gate status and start the gateway service, or use mcp-gate serve --config <path>.".into()
    } else if error.is_timeout() {
        "Timed out contacting the gateway".into()
    } else {
        "HTTP connection to the gateway failed".into()
    }
}

fn check_status(status: StatusCode, session: &Session) -> Result<(), String> {
    if status.is_success() {
        Ok(())
    } else if status == StatusCode::NOT_FOUND && session.id.is_some() {
        Err(
            "Gateway MCP session expired or was closed (HTTP 404). Reconnect the MCP client."
                .into(),
        )
    } else {
        Err(format!("Gateway returned HTTP {}", status.as_u16()))
    }
}

pub(super) enum Incoming {
    Empty,
    Json(Option<Response>),
    Events(EventStream),
}

impl Incoming {
    pub(super) fn from_response(response: Response) -> Result<Self, String> {
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        match response_kind(response.status(), content_type)? {
            Kind::Empty => Ok(Self::Empty),
            Kind::Json => Ok(Self::Json(Some(response))),
            Kind::Events => Ok(Self::Events(EventStream {
                response,
                parser: sse::Parser::new(),
                chunk: Bytes::new(),
            })),
        }
    }

    pub(super) async fn next(&mut self) -> Result<Option<Value>, String> {
        match self {
            Self::Empty => Ok(None),
            Self::Json(response) => {
                let Some(response) = response.take() else {
                    return Ok(None);
                };
                let body = response.bytes().await.map_err(connection_error)?;
                serde_json::from_slice(&body)
                    .map(Some)
                    .map_err(|_| "Invalid JSON in gateway HTTP response".into())
            }
            Self::Events(stream) => stream.next().await,
        }
    }
}

pub(super) struct EventStream {
    response: Response,
    parser: sse::Parser,
    chunk: Bytes,
}

impl EventStream {
    async fn next(&mut self) -> Result<Option<Value>, String> {
        loop {
            let mut unread = self.chunk.as_ref();
            let event = self.parser.next(&mut unread);
            self.chunk = self.chunk.slice(self.chunk.len() - unread.len()..);
            if let Some(event) = event {
                return serde_json::from_slice(&event)
                    .map(Some)
                    .map_err(|_| "Invalid JSON in gateway SSE event".into());
            }
            let Some(chunk) = self.response.chunk().await.map_err(connection_error)? else {
                return Ok(None);
            };
            self.chunk = chunk;
        }
    }
}

#[derive(Debug, PartialEq)]
enum Kind {
    Empty,
    Json,
    Events,
}

fn response_kind(status: StatusCode, content_type: Option<&str>) -> Result<Kind, String> {
    if status == StatusCode::ACCEPTED {
        return Ok(Kind::Empty);
    }
    let media = content_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    if media.eq_ignore_ascii_case(JSON) {
        Ok(Kind::Json)
    } else if media.eq_ignore_ascii_case(SSE) {
        Ok(Kind::Events)
    } else {
        Err("Gateway HTTP response did not contain JSON or SSE".into())
    }
}

#[cfg(test)]
mod tests;

//! The same one-in-flight JSON-RPC workload over newline stdio and Streamable HTTP.
use super::TIMEOUT;
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Response, header::HeaderValue};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    time::timeout,
};

const VERSION: &str = "2025-11-25";
const ACCEPT: &str = "application/json, text/event-stream";
const TOOL: &str = "state";
const VALUE: &str = "benchmark";

pub(super) struct Session<'a> {
    transport: Transport<'a>,
    initialized_at: Instant,
    next_id: u64,
}

impl<'a> Session<'a> {
    pub(super) async fn stdio(child: &'a mut Child) -> Result<Self> {
        Self::initialize(Transport::Stdio {
            input: child.stdin.as_mut().context("Missing mock stdin")?,
            output: BufReader::new(child.stdout.as_mut().context("Missing mock stdout")?),
        })
        .await
    }

    pub(super) async fn http(client: Client, base: &str, token: &str) -> Result<Self> {
        Self::initialize(Transport::Http(Http {
            client,
            url: format!("{base}/mcp"),
            token: token.to_owned(),
            session_id: None,
        }))
        .await
    }

    async fn initialize(transport: Transport<'a>) -> Result<Self> {
        let mut session = Self {
            transport,
            initialized_at: Instant::now(),
            next_id: 0,
        };
        let response = session
            .request(
                "initialize",
                json!({
                    "protocolVersion": VERSION, "capabilities": {},
                    "clientInfo": {"name": "mcp-gate-bench", "version": "1"}
                }),
            )
            .await?;
        ensure!(
            response["protocolVersion"] == VERSION,
            "Unexpected negotiated MCP version"
        );
        session.initialized_at = Instant::now();
        session
            .transport
            .exchange(json!({
                "jsonrpc": "2.0", "method": "notifications/initialized"
            }))
            .await
            .context("Cannot send initialized notification")?;
        let list = session.request("tools/list", json!({})).await?;
        ensure!(
            list["tools"]
                .as_array()
                .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == TOOL)),
            "The mock tool is missing from the catalog"
        );
        Ok(session)
    }

    pub(super) fn initialized_at(&self) -> Instant {
        self.initialized_at
    }

    pub(super) async fn first_call(&mut self) -> Result<Instant> {
        self.call().await?;
        Ok(Instant::now())
    }

    pub(super) async fn measure(&mut self, calls: u32) -> Result<Vec<Duration>> {
        let mut latencies = Vec::new();
        for _ in 0..calls {
            let started = Instant::now();
            self.call().await?;
            latencies.push(started.elapsed());
        }
        Ok(latencies)
    }

    async fn call(&mut self) -> Result<()> {
        let result = self
            .request(
                "tools/call",
                json!({
                    "name": TOOL, "arguments": {"value": VALUE}
                }),
            )
            .await?;
        ensure!(result["isError"] != true, "The mock tool returned an error");
        let text = result["content"][0]["text"]
            .as_str()
            .context("Missing mock tool result")?;
        let value: Value = serde_json::from_str(text).context("Invalid mock tool result")?;
        ensure!(
            value["value"] == VALUE,
            "The mock tool returned the wrong value"
        );
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let response = self
            .transport
            .exchange(json!({
                "jsonrpc": "2.0", "id": id, "method": method, "params": params
            }))
            .await
            .with_context(|| format!("{method} request failed"))?;
        ensure!(
            response["id"] == id && response["jsonrpc"] == "2.0",
            "Invalid response to {method}"
        );
        ensure!(response.get("error").is_none(), "MCP error from {method}");
        response
            .get("result")
            .cloned()
            .with_context(|| format!("Missing result from {method}"))
    }

    pub(super) async fn close(self) -> Result<()> {
        if let Transport::Http(http) = self.transport {
            http.close().await?;
        }
        Ok(())
    }
}

enum Transport<'a> {
    Stdio {
        input: &'a mut ChildStdin,
        output: BufReader<&'a mut ChildStdout>,
    },
    Http(Http),
}

impl Transport<'_> {
    async fn exchange(&mut self, message: Value) -> Result<Value> {
        match self {
            Self::Http(http) => http.post(message).await,
            Self::Stdio { input, output } => timeout(TIMEOUT, async {
                let mut request = serde_json::to_vec(&message)?;
                request.push(b'\n');
                input.write_all(&request).await?;
                if message.get("id").is_none() {
                    return Ok(Value::Null);
                }
                let mut line = String::new();
                ensure!(
                    output.read_line(&mut line).await? > 0,
                    "Mock backend closed stdout"
                );
                Ok(serde_json::from_str(&line)?)
            })
            .await
            .context("Stdio exchange timed out")?,
        }
    }
}

struct Http {
    client: Client,
    url: String,
    token: String,
    session_id: Option<HeaderValue>,
}

impl Http {
    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let request = self
            .client
            .request(method, &self.url)
            .bearer_auth(&self.token)
            .header("Accept", ACCEPT);
        match &self.session_id {
            Some(id) => request
                .header("mcp-session-id", id)
                .header("mcp-protocol-version", VERSION),
            None => request,
        }
    }

    async fn post(&mut self, message: Value) -> Result<Value> {
        let response = checked(
            self.request(reqwest::Method::POST)
                .json(&message)
                .send()
                .await?,
        )?;
        if self.session_id.is_none() {
            self.session_id = Some(
                response
                    .headers()
                    .get("mcp-session-id")
                    .context("Missing MCP session header")?
                    .clone(),
            );
        }
        if message.get("id").is_none() {
            response.bytes().await?;
            return Ok(Value::Null);
        }
        read_response(response, &message["id"]).await
    }

    async fn close(self) -> Result<()> {
        checked(self.request(reqwest::Method::DELETE).send().await?)?
            .bytes()
            .await?;
        Ok(())
    }
}

fn checked(response: Response) -> Result<Response> {
    ensure!(
        response.status().is_success(),
        "MCP HTTP status {}",
        response.status()
    );
    Ok(response)
}

async fn read_response(response: Response, id: &Value) -> Result<Value> {
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .context("Missing MCP response content type")?;
    match content_type.split(';').next() {
        Some("application/json") => Ok(response.json().await?),
        Some("text/event-stream") => {
            // Drain the whole body before reusing the connection, as in the integration tests.
            let body = response.bytes().await?;
            parse_sse(std::str::from_utf8(&body)?, id)
        }
        _ => bail!("Unexpected MCP response content type"),
    }
}

fn parse_sse(body: &str, id: &Value) -> Result<Value> {
    let body = body.replace("\r\n", "\n");
    for event in body.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .collect::<Vec<_>>()
            .join("\n");
        if data.trim().is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&data).context("Invalid MCP SSE data")?;
        if &message["id"] == id && message.get("method").is_none() {
            return Ok(message);
        }
    }
    bail!("Missing MCP response in event stream");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_ignores_priming_events_and_matches_the_call_id() {
        let body = concat!(
            "retry: 1000\r\ndata: \r\n\r\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\r\n\r\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{}}\r\n\r\n"
        );
        assert_eq!(parse_sse(body, &json!(3)).unwrap()["id"], 3);
        assert!(parse_sse(body, &json!(4)).is_err());
    }
}

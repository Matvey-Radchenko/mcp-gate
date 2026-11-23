//! Instrumented opt-in tool for concurrency and routing tests.
use rmcp::{RoleServer, model::*, service::RequestContext};
use serde::Deserialize;
use serde_json::json;
use std::{io::Write, path::PathBuf, sync::Mutex, time::Duration};

pub(super) struct Echo {
    journal: PathBuf,
    gates: PathBuf,
    counts: Mutex<Counts>,
}

#[derive(Default)]
struct Counts {
    active: usize,
    max_active: usize,
}

#[derive(Deserialize)]
struct Args {
    nonce: u64,
    #[serde(default)]
    delay_ms: u64,
    #[serde(default)]
    progress: u32,
    #[serde(default)]
    hold: bool,
    #[serde(default)]
    tool_error: bool,
}

impl Echo {
    pub fn tool() -> Tool {
        serde_json::from_value(json!({
            "name":"echo_delay", "description":"Echo a nonce after a delay",
            "inputSchema":{"type":"object", "required":["nonce"], "properties":{
                "nonce":{"type":"integer"}, "delay_ms":{"type":"integer"},
                "progress":{"type":"integer"}, "hold":{"type":"boolean"},
                "tool_error":{"type":"boolean"}
            }}
        }))
        .unwrap()
    }

    pub fn from_env() -> Option<Self> {
        Some(Self {
            journal: std::env::var_os("MOCK_ECHO_JOURNAL")?.into(),
            gates: std::env::var_os("MOCK_ECHO_GATE_DIR").unwrap().into(),
            counts: Mutex::default(),
        })
    }

    pub async fn call(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args: Args = serde_json::from_value(json!(request.arguments))
            .map_err(|_| ErrorData::invalid_params("Invalid echo arguments", None))?;
        self.record("start", args.nonce, &context.id, true);
        let _activity = EchoCall {
            echo: self,
            nonce: args.nonce,
            id: &context.id,
        };
        let token = context.meta.get_progress_token();
        for progress in 1..=args.progress {
            let notification = serde_json::from_value(json!({
                "progressToken": token.as_ref().unwrap(), "progress": progress,
                "total": args.progress, "message": args.nonce.to_string()
            }))
            .unwrap();
            context.peer.notify_progress(notification).await.unwrap();
        }
        let value = tokio::select! {
            _ = self.wait(&args) => json!({"nonce":args.nonce,"upstream_id":context.id}),
            _ = context.ct.cancelled() => json!({"cancelled":true}),
        };
        let result = if args.tool_error {
            CallToolResult::error(vec![ContentBlock::text("echo fixture failed")])
        } else {
            CallToolResult::success(vec![ContentBlock::text(value.to_string())])
        };
        Ok(result.into())
    }

    async fn wait(&self, args: &Args) {
        tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;
        if args.hold {
            let gate = self.gates.join(args.nonce.to_string());
            while !gate.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    fn record(&self, event: &str, nonce: u64, id: &RequestId, starting: bool) {
        // Serialize counts and writes so the journal preserves the observed
        // order. An append-only file also survives a deliberate process exit.
        let mut counts = self.counts.lock().unwrap();
        if starting {
            counts.active += 1;
            counts.max_active = counts.max_active.max(counts.active);
        } else {
            counts.active -= 1;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.journal)
            .unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "event":event, "nonce":nonce, "id":id,
                "active":counts.active, "max_active":counts.max_active
            })
        )
        .unwrap();
    }
}

struct EchoCall<'a> {
    echo: &'a Echo,
    nonce: u64,
    id: &'a RequestId,
}

impl Drop for EchoCall<'_> {
    fn drop(&mut self) {
        self.echo.record("end", self.nonce, self.id, false);
    }
}

#![allow(deprecated)]
#[path = "mock_backend/echo.rs"]
mod echo;

use echo::Echo;
use rmcp::{RoleServer, ServerHandler, ServiceExt, model::*, service::RequestContext};
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
#[derive(Default)]
struct Mock {
    echo: Option<Echo>,
    state: Mutex<Value>,
    changed_tools: AtomicBool,
    changed_core: AtomicBool,
}
impl ServerHandler for Mock {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![ProtocolVersion::V_2025_11_25])
    }
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info.server_info = Implementation::new("mock", "mock-1");
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.capabilities.tools.as_mut().unwrap().list_changed = Some(true);
        if std::env::var_os("MOCK_CORE_CATALOG").is_some() {
            info.capabilities = serde_json::from_value(json!({
                "tools":{"listChanged":true}, "resources":{"subscribe":false,"listChanged":true},
                "prompts":{"listChanged":true}, "experimental":{},
                "extensions":{"io.modelcontextprotocol/ui":{}}
            }))
            .unwrap();
        }
        info
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if std::env::var("MOCK_DISCOVERY_OFFLINE_FILE")
            .is_ok_and(|path| std::path::Path::new(&path).exists())
        {
            return Err(ErrorData::internal_error(
                "Discovery temporarily unavailable (fixture)",
                None,
            ));
        }
        let mut tools: Vec<Tool> = ["state", "pause", "roots", "crash"]
            .into_iter()
            .filter(|name| *name != "roots" || !self.changed_tools.load(Ordering::SeqCst))
            .map(|name| {
                serde_json::from_value(json!({
            "name": name, "description": name, "inputSchema": {"type":"object", "properties": {"output_dir":{"type":"string"}}}
        })).unwrap()
            })
            .collect();
        if self.echo.is_some() {
            tools.push(Echo::tool());
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let second = request.and_then(|r| r.cursor).is_some();
        let changed = self.changed_core.load(Ordering::SeqCst);
        Ok(serde_json::from_value(json!({
            "resources":[{"uri":if second {"fixture://second"} else {"fixture://first"},"name":if changed {"changed"} else {"fixture"}}],
            "nextCursor":if second { None } else { Some("second") }
        })).unwrap())
    }
    async fn list_resource_templates(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(serde_json::from_value(
            json!({"resourceTemplates":[{"uriTemplate":"fixture://{value}","name":"template"}]}),
        )
        .unwrap())
    }
    async fn list_prompts(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(serde_json::from_value(json!({"prompts":[{"name":"echo","description":if self.changed_core.load(Ordering::SeqCst) {"changed"} else {"original"}}]})).unwrap())
    }
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri == "fixture://error" {
            return Err(ErrorData::invalid_params("fixture error", None));
        }
        if request.uri == "fixture://pause" {
            context.ct.cancelled().await;
        }
        if request.uri == "fixture://change" {
            self.changed_core.store(true, Ordering::SeqCst);
            context.peer.notify_resource_list_changed().await.unwrap();
        }
        Ok(serde_json::from_value::<ReadResourceResult>(json!({"contents":[{"uri":request.uri,"text":json!({"pid":std::process::id(),"value":*self.state.lock().unwrap()}).to_string()}]})).unwrap().into())
    }
    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        if request
            .arguments
            .as_ref()
            .is_some_and(|a| a.contains_key("change"))
        {
            self.changed_core.store(true, Ordering::SeqCst);
            context.peer.notify_prompt_list_changed().await.unwrap();
        }
        Ok(serde_json::from_value::<GetPromptResult>(json!({"messages":[{"role":"user","content":{"type":"text","text":json!({"pid":std::process::id(),"arguments":request.arguments}).to_string()}}]})).unwrap().into())
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Ok(path) = std::env::var("MOCK_ACTION_JOURNAL") {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(file, "{}", request.name).unwrap();
        }
        if std::env::var("MOCK_OFFLINE_FILE").is_ok_and(|path| std::path::Path::new(&path).exists())
        {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "Network unavailable (fixture)",
            )])
            .into());
        }
        if request
            .arguments
            .as_ref()
            .is_some_and(|a| a.get("protocol_error") == Some(&json!(true)))
        {
            return Err(ErrorData::internal_error(
                "Network unavailable (fixture)",
                None,
            ));
        }
        if let Some(change) = request
            .arguments
            .as_ref()
            .and_then(|a| a.get("notify_catalog"))
        {
            if change == "changed" {
                self.changed_tools.store(true, Ordering::SeqCst);
            }
            context.peer.notify_tool_list_changed().await.unwrap();
        }
        if request.name == "echo_delay"
            && let Some(echo) = &self.echo
        {
            return echo.call(request, context).await;
        }
        let value = match request.name.as_ref() {
            "state" => {
                let mut state = self.state.lock().unwrap();
                if let Some(v) = request.arguments.as_ref().and_then(|a| a.get("value")) {
                    *state = v.clone();
                }
                if let Ok(path) = std::env::var("MOCK_CLIENT_CALL_FILE") {
                    std::fs::write(path, state.as_str().unwrap_or("fixture")).unwrap();
                }
                json!({"pid": std::process::id(), "value": *state, "arguments":request.arguments})
            }
            "pause" => {
                if let Some(token) = context.meta.get("progressToken") {
                    let param = serde_json::from_value(
                        json!({"progressToken": token, "progress": 1, "total": 2}),
                    )
                    .unwrap();
                    let _ = context.peer.notify_progress(param).await;
                }
                if let Some(delay) = request
                    .arguments
                    .as_ref()
                    .and_then(|a| a.get("delay_ms"))
                    .and_then(Value::as_u64)
                {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => return Ok(CallToolResult::success(vec![ContentBlock::text("done")]).into()),
                        _ = context.ct.cancelled() => {},
                    }
                } else {
                    context.ct.cancelled().await;
                }
                if let Ok(path) = std::env::var("MOCK_CANCEL_FILE") {
                    std::fs::write(path, "cancelled").unwrap();
                }
                json!({"cancelled": true})
            }
            "roots" => serde_json::to_value(context.peer.list_roots().await.unwrap()).unwrap(),
            "crash" => std::process::exit(23),
            _ => return Err(ErrorData::invalid_params("Unknown mock tool", None)),
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(value.to_string())]).into())
    }
}
#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Ok(path) = std::env::var("MOCK_CONTEXT_FILE") {
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "args":std::env::args().skip(1).collect::<Vec<_>>(),
                "cwd":std::env::current_dir().unwrap(),
                "value":std::env::var("MOCK_CONTEXT_VALUE").unwrap_or_default(),
                "relative":std::fs::read_to_string("relative input.txt").ok()
            }))
            .unwrap(),
        )
        .unwrap();
    }
    if std::env::var_os("MOCK_CHILD_SLEEP").is_some() {
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        return;
    }
    if let Ok(path) = std::env::var("MOCK_INIT_HANG_PID_FILE") {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .env("MOCK_CHILD_SLEEP", "1")
            .env_remove("MOCK_INIT_HANG_PID_FILE")
            .spawn()
            .unwrap();
        std::fs::write(path, format!("{} {}", std::process::id(), child.id())).unwrap();
        // Deliberately hang before the MCP handshake; the gateway owns cleanup.
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        let _ = child.wait();
    }
    let _owned_child = std::env::var("MOCK_CHILD_PID_FILE").ok().map(|path| {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .env("MOCK_CHILD_SLEEP", "1")
            .env_remove("MOCK_CHILD_PID_FILE")
            .spawn()
            .unwrap();
        std::fs::write(path, child.id().to_string()).unwrap();
        child
    });
    Mock {
        echo: Echo::from_env(),
        ..Mock::default()
    }
    .serve((mock_stdin(), tokio::io::stdout()))
    .await
    .unwrap()
    .waiting()
    .await
    .unwrap();
}

fn mock_stdin() -> Box<dyn tokio::io::AsyncRead + Unpin + Send> {
    let stdin = tokio::io::stdin();
    let Ok(path) = std::env::var("MOCK_RAW_WIRE_FILE") else {
        return Box::new(stdin);
    };
    let ignore_cancellation = std::env::var_os("MOCK_IGNORE_CANCELLATION").is_some();
    let (input, mut writer) = tokio::io::duplex(8192);
    tokio::spawn(async move {
        use std::io::Write;
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        let mut stdin = tokio::io::BufReader::new(stdin);
        let mut line = Vec::new();
        while stdin.read_until(b'\n', &mut line).await.unwrap() > 0 {
            log.write_all(&line).unwrap();
            // Some backends complete the call after receiving cancellation.
            // rmcp's server suppresses that response, so swallow only the
            // logged notification when testing such a backend.
            let ignored = ignore_cancellation
                && serde_json::from_slice::<Value>(&line).unwrap()["method"]
                    == "notifications/cancelled";
            if !ignored && writer.write_all(&line).await.is_err() {
                break; // The mock MCP service closed its input.
            }
            line.clear();
        }
    });
    Box::new(input)
}

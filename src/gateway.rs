use crate::{
    catalog::Catalog,
    config::{Config, Ownership, SharedClientRoots},
    ownership::BackendSlot,
};
use rmcp::{
    RoleServer, ServerHandler,
    model::*,
    service::{NotificationContext, RequestContext},
};
use std::{
    borrow::Cow,
    sync::{
        Arc, Mutex as StdMutex, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

pub struct Gateway {
    pub config: Config,
    pub catalog: Catalog,
    pub live: Arc<AtomicUsize>,
    pub(crate) capacity: Arc<Semaphore>,
    pub(crate) shared: Option<Arc<BackendSlot>>,
    sessions: StdMutex<Vec<Weak<Session>>>,
    pub(crate) stopping: AtomicBool,
}
impl Gateway {
    pub fn runtime_status(&self) -> serde_json::Value {
        serde_json::json!({
            "ownership": self.config.ownership,
            "shared_client_roots": self.config.shared_client_roots,
            "concurrency_per_backend": 1,
            "max_pending_calls": self.config.max_pending_calls,
            "queue_timeout_seconds": self.config.queue_timeout_seconds,
            "shared_state": self.shared.as_ref().map(|slot| {
                if slot.failed.load(Ordering::SeqCst) { "failed" }
                else if self.live.load(Ordering::SeqCst) > 0 { "running" }
                else { "dormant" }
            }),
        })
    }
    pub fn new(config: Config, catalog: Catalog) -> Arc<Self> {
        Arc::new(Self {
            shared: (config.ownership == Ownership::Shared)
                .then(|| BackendSlot::new(config.max_pending_calls)),
            capacity: Arc::new(Semaphore::new(config.max_workers)),
            config,
            catalog,
            live: Arc::new(AtomicUsize::new(0)),
            sessions: StdMutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
        })
    }
    pub fn handler(self: &Arc<Self>) -> Result<Handler, std::io::Error> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("Gateway stopping"));
        }
        let session = Arc::new(Session {
            namespace: uuid::Uuid::new_v4(),
            gateway: self.clone(),
            backend: self
                .shared
                .clone()
                .unwrap_or_else(|| BackendSlot::new(self.config.max_pending_calls)),
        });
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|s| s.strong_count() > 0);
        sessions.push(Arc::downgrade(&session));
        Ok(Handler { session })
    }
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let sessions: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        futures::future::join_all(sessions.iter().map(|s| async {
            s.backend.shutdown().await;
        }))
        .await;
        if let Some(shared) = &self.shared {
            shared.shutdown().await;
        }
        // Also wait for RAII cleanup of already-dropped session handlers.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
        while self.live.load(Ordering::SeqCst) > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
pub(crate) struct Session {
    namespace: uuid::Uuid,
    pub(crate) gateway: Arc<Gateway>,
    pub(crate) backend: Arc<BackendSlot>,
}
#[derive(Clone)]
pub struct Handler {
    pub(crate) session: Arc<Session>,
}
impl ServerHandler for Handler {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        // HTTP session ids are an essential isolation boundary. Do not negotiate
        // the newer stateless lifecycle until it has an explicit session design.
        Cow::Owned(vec![
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_03_26,
        ])
    }
    fn get_info(&self) -> ServerInfo {
        let mut info = self.session.gateway.catalog.server_info.clone();
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info.server_info = Implementation::new("mcp-gate", env!("CARGO_PKG_VERSION"));
        info.capabilities = crate::discovery::frontend_capabilities(&info.capabilities);
        info.instructions = Some(format!(
            "{}\n{}\n{}",
            info.instructions.unwrap_or_default(),
            self.session.gateway.config.tool_policy.instructions,
            if self.session.gateway.shared.is_some() {
                if self.session.gateway.config.shared_client_roots == SharedClientRoots::Ignore {
                    "One backend is shared by all clients. Calls are serialized. Client roots are explicitly ignored; only the fixed backend configuration defines context. Uncertain actions require gateway restart."
                } else {
                    "One backend is shared by all clients. Calls are serialized. Roots-capable clients are rejected. Uncertain actions require gateway restart."
                }
            } else {
                "The backend is isolated to this MCP session and starts on first executable request. Reinitializing creates fresh state, not restored state."
            }
        ));
        info
    }
    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request.and_then(|p| p.cursor).is_some() {
            return Err(ErrorData::invalid_params("No further catalog pages", None));
        }
        Ok(ListToolsResult {
            tools: self
                .session
                .gateway
                .catalog
                .tools
                .iter()
                .filter_map(|t| self.session.gateway.config.tool_policy.expose(t))
                .collect(),
            ..Default::default()
        })
    }
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        crate::discovery::first_page(request)?;
        Ok(ListResourcesResult {
            resources: self.session.gateway.catalog.resources.clone(),
            ..Default::default()
        })
    }
    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        crate::discovery::first_page(request)?;
        Ok(ListResourceTemplatesResult {
            resource_templates: self.session.gateway.catalog.resource_templates.clone(),
            ..Default::default()
        })
    }
    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        crate::discovery::first_page(request)?;
        Ok(ListPromptsResult {
            prompts: self.session.gateway.catalog.prompts.clone(),
            ..Default::default()
        })
    }
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if self
            .session
            .gateway
            .catalog
            .server_info
            .capabilities
            .resources
            .is_none()
        {
            return Err(ErrorData::method_not_found::<ReadResourceRequestMethod>());
        }
        match self
            .execute(
                ClientRequest::ReadResourceRequest(ReadResourceRequest::new(request)),
                context,
            )
            .await?
        {
            ServerResult::ReadResourceResult(result) => Ok(result.into()),
            _ => Err(ErrorData::internal_error(
                "Unexpected backend response",
                None,
            )),
        }
    }
    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        if !self
            .session
            .gateway
            .catalog
            .prompts
            .iter()
            .any(|p| p.name == request.name)
        {
            return Err(ErrorData::invalid_params("Unknown prompt", None));
        }
        match self
            .execute(
                ClientRequest::GetPromptRequest(GetPromptRequest::new(request)),
                context,
            )
            .await?
        {
            ServerResult::GetPromptResult(result) => Ok(result.into()),
            _ => Err(ErrorData::internal_error(
                "Unexpected backend response",
                None,
            )),
        }
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        if !self.session.gateway.config.tool_policy.permits(name) {
            return None;
        }
        self.session
            .gateway
            .catalog
            .tools
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| self.session.gateway.config.tool_policy.expose(t))
    }
    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.get_tool(&request.name).is_none() {
            return Err(ErrorData::invalid_params("Unknown tool", None));
        }
        self.session
            .gateway
            .config
            .tool_policy
            .apply(&mut request, self.session.namespace)?;
        match self
            .execute(
                ClientRequest::CallToolRequest(CallToolRequest::new(request)),
                context,
            )
            .await?
        {
            ServerResult::CallToolResult(result) => Ok(result.into()),
            _ => Err(ErrorData::internal_error(
                "Unexpected backend response",
                None,
            )),
        }
    }
    #[allow(deprecated)]
    async fn on_roots_list_changed(&self, context: NotificationContext<RoleServer>) {
        if self.session.gateway.shared.is_some() {
            return;
        }
        let Ok(Ok(roots)) =
            tokio::time::timeout(Duration::from_secs(10), context.peer.list_roots()).await
        else {
            return;
        };
        if let Some(worker) = self.session.backend.worker.lock().await.as_ref() {
            worker.set_roots(roots).await;
        }
    }
    async fn on_initialized(&self, _: NotificationContext<RoleServer>) {}
}

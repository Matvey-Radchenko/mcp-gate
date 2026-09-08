//! Owns an upstream process and its protocol connection. No shell is involved.
#![allow(deprecated)] // Required by the session-based MCP revisions we deliberately support.
use crate::{config::Config, process::ProcessOwner};
use anyhow::{Context, Result, anyhow};
use rmcp::{
    ClientHandler, Peer, RoleClient, RoleServer, ServiceExt,
    model::*,
    service::{NotificationContext, RequestContext, RunningService},
};
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::OwnedSemaphorePermit;

#[derive(Clone)]
pub struct UpstreamClient {
    pub downstream: Option<Peer<RoleServer>>,
    roots: Arc<tokio::sync::RwLock<ListRootsResult>>,
    progress: tokio::sync::broadcast::Sender<ProgressNotificationParam>,
    catalog_dirty: Arc<AtomicBool>,
}
impl ClientHandler for UpstreamClient {
    async fn on_tool_list_changed(&self, _: NotificationContext<RoleClient>) {
        self.catalog_dirty.store(true, Ordering::SeqCst);
    }
    async fn on_resource_list_changed(&self, _: NotificationContext<RoleClient>) {
        self.catalog_dirty.store(true, Ordering::SeqCst);
    }
    async fn on_prompt_list_changed(&self, _: NotificationContext<RoleClient>) {
        self.catalog_dirty.store(true, Ordering::SeqCst);
    }
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info.client_info = Implementation::new("mcp-gate", env!("CARGO_PKG_VERSION"));
        if self
            .downstream
            .as_ref()
            .and_then(|p| p.peer_info())
            .is_some_and(|i| i.capabilities.roots.is_some())
        {
            let mut roots = RootsCapabilities::default();
            roots.list_changed = Some(true);
            info.capabilities.roots = Some(roots);
        }
        info
    }
    async fn list_roots(
        &self,
        _: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, ErrorData> {
        Ok(self.roots.read().await.clone())
    }
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _: NotificationContext<RoleClient>,
    ) {
        let _ = self.progress.send(params);
    }
    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _: NotificationContext<RoleClient>,
    ) {
        // Forward to the owning client only. Never persist browser content in gateway logs.
        if let Some(peer) = &self.downstream {
            let _ = peer.notify_logging_message(params).await;
        }
    }
}

pub struct Worker {
    resources: Option<Resources>,
    roots: Arc<tokio::sync::RwLock<ListRootsResult>>,
    progress: tokio::sync::broadcast::Sender<ProgressNotificationParam>,
    catalog_dirty: Arc<AtomicBool>,
}
struct Resources {
    service: RunningService<RoleClient, UpstreamClient>,
    process: ProcessOwner,
}
impl Worker {
    pub async fn start(
        config: &Config,
        downstream: Option<Peer<RoleServer>>,
        permit: Option<OwnedSemaphorePermit>,
        live: Arc<AtomicUsize>,
    ) -> Result<Self> {
        // Ask in the originating tools/call scope: the response can travel on
        // that POST's SSE stream even when the client has no standalone GET.
        let roots = if let Some(peer) = &downstream {
            if peer
                .peer_info()
                .is_some_and(|i| i.capabilities.roots.is_some())
            {
                tokio::time::timeout(
                    Duration::from_secs(config.startup_timeout_seconds),
                    peer.list_roots(),
                )
                .await
                .context("Client roots request timed out")??
            } else {
                ListRootsResult::default()
            }
        } else {
            ListRootsResult::default()
        };
        let roots = Arc::new(tokio::sync::RwLock::new(roots));
        let (progress, _) = tokio::sync::broadcast::channel(32);
        let catalog_dirty = Arc::new(AtomicBool::new(false));
        let mut command = config.backend.command()?;
        command.current_dir(
            config
                .backend
                .working_directory
                .as_deref()
                .unwrap_or(&config.state_dir),
        );
        config
            .backend
            .configure_directories(&mut command, &config.state_dir)?;
        let container = if config.backend.docker {
            Some(crate::platform::docker::Container::prepare(
                &mut command,
                &config.state_dir,
            )?)
        } else {
            None
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut process = ProcessOwner::spawn(&mut command, permit, live, container)
            .context("Cannot start backend")?;
        let child = process.child();
        let stdout = child.stdout.take().context("Missing backend stdout")?;
        let stdin = child.stdin.take().context("Missing backend stdin")?;
        let service = tokio::time::timeout(
            Duration::from_secs(config.startup_timeout_seconds),
            UpstreamClient {
                downstream,
                roots: roots.clone(),
                progress: progress.clone(),
                catalog_dirty: catalog_dirty.clone(),
            }
            .serve((stdout, stdin)),
        )
        .await
        .context("Backend initialization timed out")
        .and_then(|result| result.map_err(anyhow::Error::from));
        let service = match service {
            Ok(service) => service,
            Err(error) => {
                process.close().await;
                return Err(error);
            }
        };
        Ok(Self {
            resources: Some(Resources { service, process }),
            roots,
            progress,
            catalog_dirty,
        })
    }
    pub fn peer(&self) -> &Peer<RoleClient> {
        self.resources.as_ref().expect("live worker").service.peer()
    }
    pub fn closed(&self) -> bool {
        self.resources
            .as_ref()
            .is_none_or(|r| r.service.is_closed())
    }
    pub fn subscribe_progress(
        &self,
    ) -> tokio::sync::broadcast::Receiver<ProgressNotificationParam> {
        self.progress.subscribe()
    }
    pub async fn set_roots(&self, roots: ListRootsResult) {
        *self.roots.write().await = roots;
        let _ = self.peer().notify_roots_list_changed().await;
    }
    pub async fn catalog(&self) -> Result<crate::catalog::Discovery> {
        // Notifications arriving during the query set this back to true; such a
        // moving catalog cannot be accepted as a stable discovery snapshot.
        self.catalog_dirty.store(false, Ordering::SeqCst);
        let peer_info = self
            .peer()
            .peer_info()
            .ok_or_else(|| anyhow!("Backend omitted server info"))?;
        let mut info = ServerInfo::default();
        info.protocol_version = peer_info.protocol_version.clone();
        info.capabilities = peer_info.capabilities.clone();
        info.server_info = peer_info
            .server_info
            .clone()
            .ok_or_else(|| anyhow!("Backend omitted identity"))?;
        info.instructions = peer_info.instructions.clone();
        crate::catalog::validate_capabilities(&info)?;
        let tools = self.peer().list_all_tools().await?;
        let (resources, resource_templates) = if info.capabilities.resources.is_some() {
            (
                self.peer().list_all_resources().await?,
                self.peer().list_all_resource_templates().await?,
            )
        } else {
            (vec![], vec![])
        };
        let prompts = if info.capabilities.prompts.is_some() {
            self.peer().list_all_prompts().await?
        } else {
            vec![]
        };
        Ok(crate::catalog::Discovery {
            info,
            tools,
            resources,
            resource_templates,
            prompts,
        })
    }
    pub async fn shutdown(mut self) {
        if let Some(resources) = self.resources.take() {
            resources.close().await;
        }
    }
    pub fn catalog_changed(&self) -> bool {
        self.catalog_dirty.load(Ordering::SeqCst)
    }
}
impl Resources {
    async fn close(mut self) {
        let _ = self
            .service
            .close_with_timeout(Duration::from_secs(2))
            .await;
        self.process.close().await;
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(resources) = self.resources.take() {
            tokio::spawn(resources.close());
        }
    }
}

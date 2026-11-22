//! Own the bridge session, initialization barriers, and in-flight HTTP tasks.
mod http;
mod rpc;
mod sse;
mod stdio;

use anyhow::{Context, Result, ensure};
use http::{Connection, Incoming, Session};
use mcp_gate::{config::Config, manage::store, platform::ShutdownSignal};
use rpc::Replies;
use serde_json::Value;
use std::{path::PathBuf, time::Duration};
use tokio::{
    sync::mpsc,
    task::{JoinError, JoinSet},
};

pub(crate) async fn run(name: &str, config: Option<PathBuf>) -> Result<()> {
    let config = match config {
        Some(path) => path,
        None => {
            let registry = store::load(&store::root()?)?;
            let record = registry.select(name)?;
            ensure!(
                !record.removing,
                "Gateway removal is pending. Run mcp-gate status."
            );
            record.config()
        }
    };
    let (listen, token) = Config::client_connection(&config)
        .context("Cannot load gateway configuration or private token. Check --config and run mcp-gate status.")?;
    let connection = Connection::new(listen, token).context("Cannot create gateway HTTP client")?;
    let mut signal = ShutdownSignal::new().context("Cannot register shutdown handlers")?;
    let mut input = stdio::Input::start()?;
    let stdio::Output { messages, mut done } = stdio::Output::start()?;
    let mut bridge = Bridge::new(connection, messages);

    let mut writer_stopped = false;
    let result = tokio::select! {
        biased;
        result = signal.wait() => result.context("Cannot receive shutdown signal"),
        result = &mut input.done => {
            result.context("Stdio reader stopped unexpectedly")
                .and_then(|result| result.context("Cannot read stdio MCP input"))
        },
        result = &mut done => {
            writer_stopped = true;
            result.context("Stdio writer stopped unexpectedly").and_then(|result| result)
        },
        () = bridge.drive(&mut input.lines) => Ok(()),
    };
    drop(input);
    bridge.close().await;
    if !writer_stopped {
        match tokio::time::timeout(Duration::from_secs(1), done).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => eprintln!("connect: {error}"),
            Ok(Err(_)) => eprintln!("connect: Stdio writer stopped unexpectedly"),
            Err(_) => eprintln!("connect: Timed out flushing stdio MCP output"),
        }
    }
    result
}

struct Bridge {
    connection: Connection,
    session: Session,
    get_started: bool,
    output: mpsc::Sender<Value>,
    tasks: JoinSet<()>,
}

impl Bridge {
    fn new(connection: Connection, output: mpsc::Sender<Value>) -> Self {
        Self {
            connection,
            session: Session::default(),
            get_started: false,
            output,
            tasks: JoinSet::new(),
        }
    }

    async fn drive(&mut self, lines: &mut mpsc::Receiver<Vec<u8>>) {
        loop {
            tokio::select! {
                joined = self.tasks.join_next(), if !self.tasks.is_empty() => report_task(joined),
                line = lines.recv() => match line {
                    Some(line) => self.forward(&line).await,
                    None => break,
                },
            }
        }
    }

    async fn forward(&mut self, line: &[u8]) {
        let Ok(message) = serde_json::from_slice::<Value>(line) else {
            eprintln!("connect: Invalid JSON on stdio MCP input");
            return;
        };
        if !rpc::valid(&message) {
            eprintln!("connect: Invalid JSON-RPC message on stdio MCP input");
        } else if !self.session.started()
            && message["method"] == "initialize"
            && let Some(id) = rpc::request_id(&message)
        {
            self.initialize(&message, id).await;
        } else if self.session.negotiated()
            && !self.get_started
            && message["method"] == "notifications/initialized"
            && message.get("id").is_none()
        {
            self.acknowledge_initialized(&message).await;
        } else {
            self.spawn_post(message);
        }
    }

    async fn initialize(&mut self, message: &Value, id: &Value) {
        // Keep newer stdin messages queued until the initialize reply arrives.
        // The reply stream can stay open, so a task relays its remaining events.
        let mut replies = Replies::new(message);
        match self.negotiate(message, id).await {
            Ok(Some((response, incoming))) => {
                replies.observe(&response);
                if self.output.send(response).await.is_ok() {
                    self.tasks
                        .spawn(replies.relay(Ok(incoming), self.output.clone()));
                }
            }
            Ok(None) => {} // The writer is gone, and run observes its result.
            Err(error) => replies.fail(&error, &self.output).await,
        }
    }

    async fn negotiate(
        &mut self,
        message: &Value,
        id: &Value,
    ) -> Result<Option<(Value, Incoming)>, String> {
        let response = self.connection.post(&self.session, message).await?;
        // Record ownership as soon as headers arrive, so shutdown can close
        // the session even if it interrupts reading the initialize body.
        self.session.capture(response.headers())?;
        let mut incoming = Incoming::from_response(response)?;
        while let Some(message) = rpc::next(&mut incoming).await? {
            if let Some(reply) = rpc::response_to(&message, id) {
                if reply.get("result").is_some() {
                    self.session.negotiate(
                        reply
                            .pointer("/result/protocolVersion")
                            .and_then(Value::as_str),
                    )?;
                }
                return Ok(Some((message, incoming)));
            }
            if self.output.send(message).await.is_err() {
                return Ok(None);
            }
        }
        Err("Gateway closed its HTTP response before replying to initialize".into())
    }

    async fn acknowledge_initialized(&mut self, message: &Value) {
        // Wait for HTTP acceptance so tools/list cannot overtake the final
        // handshake notification. Normal POSTs are independent after this.
        let response = self.connection.post(&self.session, message).await;
        if response.is_ok() {
            self.get_started = true;
            let (connection, session, output) = self.request_context();
            self.tasks.spawn(async move {
                Replies::default()
                    .relay(connection.listen(&session).await, output)
                    .await;
            });
        }
        self.tasks.spawn(Replies::default().relay(
            response.and_then(Incoming::from_response),
            self.output.clone(),
        ));
    }

    fn spawn_post(&mut self, message: Value) {
        let (connection, session, output) = self.request_context();
        self.tasks.spawn(async move {
            let incoming = connection
                .post(&session, &message)
                .await
                .and_then(Incoming::from_response);
            Replies::new(&message).relay(incoming, output).await;
        });
    }

    fn request_context(&self) -> (Connection, Session, mpsc::Sender<Value>) {
        (
            self.connection.clone(),
            self.session.clone(),
            self.output.clone(),
        )
    }

    async fn close(mut self) {
        self.tasks.abort_all();
        while let Some(joined) = self.tasks.join_next().await {
            report_task(Some(joined));
        }
        drop(self.output);
        if let Err(error) = self.connection.close(&self.session).await {
            eprintln!("connect: Cannot close gateway session. {error}");
        }
    }
}

fn report_task(joined: Option<Result<(), JoinError>>) {
    if let Some(Err(error)) = joined
        && !error.is_cancelled()
    {
        eprintln!("connect: Gateway HTTP task terminated unexpectedly");
    }
}

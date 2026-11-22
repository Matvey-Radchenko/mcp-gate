//! Blocking OS pipes live on dedicated threads so a signal can interrupt the
//! bridge even while the client keeps stdin open or stops reading stdout.
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{self, BufRead, BufWriter, Write};
use tokio::sync::{mpsc, oneshot};

const BUFFER: usize = 32;

pub(super) struct Input {
    pub(super) lines: mpsc::Receiver<Vec<u8>>,
    pub(super) done: oneshot::Receiver<io::Result<()>>,
}

impl Input {
    pub(super) fn start() -> Result<Self> {
        let (send, lines) = mpsc::channel(BUFFER);
        let (finish, done) = oneshot::channel();
        std::thread::Builder::new()
            .name("mcp-stdin".into())
            .spawn(move || {
                // The receiver can disappear during shutdown. The process owns
                // the lifetime of any thread still blocked in an OS pipe read.
                // Publish read errors before dropping the line sender.
                drop(finish.send(read_lines(&send)));
            })
            .context("Cannot start stdio reader")?;
        Ok(Self { lines, done })
    }
}

fn read_lines(send: &mpsc::Sender<Vec<u8>>) -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    loop {
        let mut line = Vec::new();
        if stdin.read_until(b'\n', &mut line)? == 0 || send.blocking_send(line).is_err() {
            return Ok(());
        }
    }
}

pub(super) struct Output {
    pub(super) messages: mpsc::Sender<Value>,
    pub(super) done: oneshot::Receiver<Result<()>>,
}

impl Output {
    pub(super) fn start() -> Result<Self> {
        let (messages, receive) = mpsc::channel(BUFFER);
        let (finish, done) = oneshot::channel();
        std::thread::Builder::new()
            .name("mcp-stdout".into())
            .spawn(move || {
                drop(finish.send(write_lines(receive).context("Cannot write stdio MCP output")));
            })
            .context("Cannot start stdio writer")?;
        Ok(Self { messages, done })
    }
}

fn write_lines(mut receive: mpsc::Receiver<Value>) -> Result<()> {
    let mut stdout = BufWriter::new(io::stdout().lock());
    while let Some(value) = receive.blocking_recv() {
        serde_json::to_writer(&mut stdout, &value)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

//! Opt-in benchmark driver. Each row gets a cold-start pass and a workload pass.
mod binaries;
mod fixture;
mod memory;
mod process;
mod report;
mod rpc;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use fixture::Fixture;
use process::{Processes, Signals, finish};
use reqwest::Client;
use serde::Serialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(
    bin_name = "cargo xtask bench",
    about = "Compare release-mode MCP transports offline"
)]
struct Options {
    /// Timed tool calls per session.
    #[arg(long, default_value = "200", value_parser = clap::value_parser!(u32).range(1..))]
    calls: u32,
    /// Comma-separated numbers of simultaneous clients.
    #[arg(long, default_value = "1,3,5", value_delimiter = ',', value_parser = clap::value_parser!(u32).range(1..=1024))]
    concurrency: Vec<u32>,
    /// Limit the run to one transport mode.
    #[arg(long)]
    mode: Option<Mode>,
    /// Write JSON to stdout instead of the table.
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Direct,
    Shared,
    Session,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Shared => "shared",
            Self::Session => "session",
        }
    }
}

pub(super) fn run(root: &Path, args: &[String]) -> Result<()> {
    let options = match Options::try_parse_from(
        std::iter::once("bench").chain(args.iter().map(String::as_str)),
    ) {
        Ok(options) => options,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            error.print()?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    binaries::use_release_driver(root, args)?;
    let binaries = binaries::build(root)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(benchmark(options, binaries))
}

async fn benchmark(options: Options, binaries: binaries::Binaries) -> Result<()> {
    let mut signals = Signals::new()?;
    let http = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(TIMEOUT)
        .build()?;
    let mut rows = Vec::new();
    let modes = [Mode::Direct, Mode::Shared, Mode::Session];
    let selected = options
        .mode
        .as_ref()
        .map_or(modes.as_slice(), std::slice::from_ref);
    for &concurrency in &options.concurrency {
        for &mode in selected {
            let scenario = async {
                let fixture =
                    Fixture::prepare(&binaries, mode, concurrency as usize, &mut signals).await?;
                let result = async {
                    let cold = process_pass(&fixture, &mut signals, async |processes, started| {
                        cold_start(&fixture, &http, processes, started).await
                    })
                    .await?;
                    let load = process_pass(&fixture, &mut signals, async |processes, _| {
                        workload(&fixture, &http, processes, options.calls).await
                    })
                    .await?;
                    report::Row::new(mode, concurrency, cold, load)
                }
                .await;
                finish(result, fixture.close())
            };
            rows.push(
                scenario
                    .await
                    .with_context(|| format!("{} with {concurrency} clients", mode.name()))?,
            );
        }
    }
    report::write(&rows, options.calls, options.json)
}

struct ColdStart {
    initialize: Duration,
    first_call: Duration,
}

struct Workload {
    idle: memory::Memory,
    loaded: memory::Memory,
    latencies: Vec<Duration>,
}

async fn process_pass<T>(
    fixture: &Fixture<'_>,
    signals: &mut Signals,
    measure: impl AsyncFnOnce(&mut Processes, Instant) -> Result<T>,
) -> Result<T> {
    let mut processes = Processes::default();
    let result = tokio::select! {
        biased;
        () = signals.interrupted() => Err(anyhow::anyhow!("Benchmark interrupted")),
        result = async {
            let started = Instant::now();
            processes.start(fixture)?;
            measure(&mut processes, started).await
        } => result,
    };
    // Process ownership stays outside the cancelled future so cleanup can reap it.
    finish(result, processes.stop().await)
}

async fn cold_start(
    fixture: &Fixture<'_>,
    http: &Client,
    processes: &mut Processes,
    started: Instant,
) -> Result<ColdStart> {
    let mut sessions = processes.connect(fixture, http).await?;
    let initialize = earliest(sessions.iter().map(rpc::Session::initialized_at), started)?;
    let first_calls = collect(sessions.iter_mut().map(rpc::Session::first_call)).await?;
    let first_call = earliest(first_calls.into_iter(), started)?;
    collect(sessions.into_iter().map(rpc::Session::close)).await?;
    Ok(ColdStart {
        initialize,
        first_call,
    })
}

async fn workload(
    fixture: &Fixture<'_>,
    http: &Client,
    processes: &mut Processes,
    calls: u32,
) -> Result<Workload> {
    let pids = processes.pids()?;
    let mut sessions = processes.connect(fixture, http).await?;
    let idle = memory::sample(&pids)?;
    let latencies = collect(sessions.iter_mut().map(|session| session.measure(calls)))
        .await?
        .into_iter()
        .flatten()
        .collect();
    let loaded = memory::sample(&pids)?;
    collect(sessions.into_iter().map(rpc::Session::close)).await?;
    Ok(Workload {
        idle,
        loaded,
        latencies,
    })
}

fn earliest(times: impl Iterator<Item = Instant>, started: Instant) -> Result<Duration> {
    Ok(times.min().context("No client completed the operation")? - started)
}

// Let all admitted requests settle on error. Each session has a per-request timeout.
async fn collect<T>(
    tasks: impl Iterator<Item = impl Future<Output = Result<T>>>,
) -> Result<Vec<T>> {
    futures::future::join_all(tasks).await.into_iter().collect()
}

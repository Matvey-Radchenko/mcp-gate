//! Pooled nearest-rank percentiles and reporting in milliseconds and KiB.
use super::{ColdStart, Mode, Workload, memory::Memory};
use anyhow::Result;
use serde::Serialize;
use std::{io, time::Duration};

#[derive(Serialize)]
pub(super) struct Row {
    mode: Mode,
    concurrency: u32,
    p50_ms: f64,
    p95_ms: f64,
    spawn_to_initialize_ms: f64,
    spawn_to_first_call_ms: f64,
    idle: Memory,
    post_load: Memory,
}

impl Row {
    pub(super) fn new(
        mode: Mode,
        concurrency: u32,
        cold: ColdStart,
        load: Workload,
    ) -> Result<Self> {
        let Workload {
            idle,
            loaded,
            mut latencies,
        } = load;
        latencies.sort_unstable();
        Ok(Self {
            mode,
            concurrency,
            p50_ms: millis(percentile(&latencies, 50)?),
            p95_ms: millis(percentile(&latencies, 95)?),
            spawn_to_initialize_ms: millis(cold.initialize),
            spawn_to_first_call_ms: millis(cold.first_call),
            idle,
            post_load: loaded,
        })
    }
}

fn percentile(sorted: &[Duration], percent: usize) -> Result<Duration> {
    anyhow::ensure!(
        !sorted.is_empty() && (1..=100).contains(&percent),
        "Invalid percentile or empty sample"
    );
    Ok(sorted[(sorted.len() * percent).div_ceil(100) - 1])
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

pub(super) fn write(rows: &[Row], calls: u32, json: bool) -> Result<()> {
    if json {
        let output = serde_json::json!({
            "calls_per_session": calls, "results": rows
        });
        serde_json::to_writer_pretty(io::stdout().lock(), &output)?;
        println!();
    } else {
        println!("{calls} timed calls per session. Times in ms, memory in KiB.");
        println!(
            "Direct memory sums mock backends. Shared and session memory cover the gateway only."
        );
        println!(
            "{:<8} {:>7} {:>8} {:>8} {:>10} {:>11} {:>9} {:>12} {:>10} {:>12}",
            "Mode",
            "Clients",
            "p50",
            "p95",
            "Cold init",
            "Cold call",
            "Idle RSS",
            "Idle private",
            "Loaded RSS",
            "Load private",
        );
        for row in rows {
            println!(
                "{:<8} {:>7} {:>8.3} {:>8.3} {:>10.3} {:>11.3} {:>9} {:>12} {:>10} {:>12}",
                row.mode.name(),
                row.concurrency,
                row.p50_ms,
                row.p95_ms,
                row.spawn_to_initialize_ms,
                row.spawn_to_first_call_ms,
                row.idle.rss_kib,
                optional(row.idle.private_kib),
                row.post_load.rss_kib,
                optional(row.post_load.private_kib),
            );
        }
    }
    Ok(())
}

fn optional(memory: Option<u64>) -> String {
    memory.map_or_else(|| "n/a".to_owned(), |kib| kib.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_handles_small_samples_and_rounds_up() {
        let samples = (1..=21).map(Duration::from_millis).collect::<Vec<_>>();
        assert_eq!(percentile(&samples, 50).unwrap(), Duration::from_millis(11));
        assert_eq!(percentile(&samples, 95).unwrap(), Duration::from_millis(20));
        assert_eq!(
            percentile(&samples[..1], 95).unwrap(),
            Duration::from_millis(1)
        );
        assert!(percentile(&[], 50).is_err());
    }
}

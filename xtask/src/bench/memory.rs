//! Point-in-time process memory, in KiB.
use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
pub(super) struct Memory {
    pub(super) rss_kib: u64,
    pub(super) private_kib: Option<u64>,
}

pub(super) fn sample(pids: &[u32]) -> Result<Memory> {
    let mut measurements = pids.iter().map(|&pid| {
        process_memory(pid).with_context(|| format!("Cannot read memory for PID {pid}"))
    });
    let first = measurements.next().context("No processes to sample")??;
    measurements.try_fold(first, |total, next| {
        let next = next?;
        Ok(Memory {
            rss_kib: total.rss_kib + next.rss_kib,
            private_kib: total.private_kib.zip(next.private_kib).map(|(a, b)| a + b),
        })
    })
}

#[cfg(target_os = "linux")]
fn process_memory(pid: u32) -> Result<Memory> {
    let status =
        std::fs::read_to_string(format!("/proc/{pid}/status")).context("Reading proc status")?;
    let rollup = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
        .context("Reading proc smaps_rollup")?;
    Ok(Memory {
        rss_kib: kib_field(&status, "VmRSS:")?,
        private_kib: Some(private_kib(&rollup)?),
    })
}

#[cfg(not(target_os = "linux"))]
fn process_memory(pid: u32) -> Result<Memory> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .context("Cannot run ps")?;
    anyhow::ensure!(output.status.success(), "ps exited with {}", output.status);
    let rss_kib = std::str::from_utf8(&output.stdout)?.trim().parse()?;
    Ok(Memory {
        rss_kib,
        private_kib: None,
    })
}

#[cfg(any(target_os = "linux", test))]
fn private_kib(rollup: &str) -> Result<u64> {
    Ok(kib_field(rollup, "Private_Clean:")? + kib_field(rollup, "Private_Dirty:")?)
}

#[cfg(any(target_os = "linux", test))]
fn kib_field(text: &str, name: &str) -> Result<u64> {
    let value = text
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .with_context(|| format!("Missing {name}"))?;
    let mut fields = value.split_whitespace();
    let count = fields
        .next()
        .with_context(|| format!("Missing value for {name}"))?
        .parse()?;
    anyhow::ensure!(fields.next() == Some("kB"), "Unexpected unit for {name}");
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_memory_excludes_shared_and_huge_page_fields() {
        let rollup = "Rss: 4100 kB\nShared_Clean: 3280 kB\nPrivate_Clean: 20 kB\nPrivate_Dirty: 800 kB\nPrivate_Hugetlb: 0 kB\n";
        assert_eq!(private_kib(rollup).unwrap(), 820);
        assert_eq!(kib_field("VmRSS:\t4100 kB\n", "VmRSS:").unwrap(), 4100);
        assert!(private_kib("Private_Dirty: 800 kB\n").is_err());
        assert!(kib_field("VmRSS: 4100 B\n", "VmRSS:").is_err());
    }
}

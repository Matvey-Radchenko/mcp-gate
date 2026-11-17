//! Optional setup timing traces under MCP_GATE_DEBUG_SETUP.
use std::{fs::OpenOptions, io::Write, sync::OnceLock, time::Instant};

fn destination() -> Option<String> {
    let dest = std::env::var("MCP_GATE_DEBUG_SETUP").ok()?;
    (!dest.is_empty()).then_some(dest)
}

fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

pub(crate) fn log(message: &str) {
    let Some(dest) = destination() else {
        return;
    };
    let line = format!("+{}ms {message}\n", start().elapsed().as_millis());
    if dest == "1" {
        eprint!("{line}");
        return;
    }
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&dest)
        .and_then(|mut file| file.write_all(line.as_bytes()));
}

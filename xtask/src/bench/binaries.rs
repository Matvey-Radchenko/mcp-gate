//! Build the same release binaries used in every scenario and resolve their Cargo artifact paths.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const RELEASE_DRIVER: &str = "MCP_GATE_BENCH_RELEASE_DRIVER";

pub(super) fn use_release_driver(root: &Path, args: &[String]) -> Result<()> {
    // The xtask alias normally builds a debug driver. Use an optimized client
    // too, to reduce JSON and HTTP client overhead in the measured intervals.
    if cfg!(debug_assertions) && std::env::var_os(RELEASE_DRIVER).is_none() {
        let error = cargo(root)
            .args([
                "run",
                "--offline",
                "--locked",
                "--release",
                "--package",
                "xtask",
                "--",
                "bench",
            ])
            .args(args)
            .env(RELEASE_DRIVER, "1")
            .exec();
        return Err(error).context("Cannot launch the release benchmark driver");
    }
    Ok(())
}

pub(super) struct Binaries {
    pub(super) backend: PathBuf,
    pub(super) gateway: PathBuf,
}

pub(super) fn build(root: &Path) -> Result<Binaries> {
    let output = cargo(root)
        .args([
            "build",
            "--offline",
            "--locked",
            "--release",
            "--package",
            "mcp-gate",
            "--features",
            "test-backend",
            "--bin",
            "mcp-gate",
            "--bin",
            "mock-backend",
            "--message-format=json-render-diagnostics",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .context("Cannot start Cargo to build the release benchmark binaries")?;
    let mut gateway = None;
    let mut backend = None;
    for line in std::str::from_utf8(&output.stdout)?.lines() {
        let message: Value = serde_json::from_str(line).context("Invalid Cargo build message")?;
        if let Some(diagnostic) = message["message"]["rendered"].as_str() {
            eprint!("{diagnostic}");
        }
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        if let Some(path) = message["executable"].as_str() {
            match message["target"]["name"].as_str() {
                Some("mcp-gate") => gateway = Some(PathBuf::from(path)),
                Some("mock-backend") => backend = Some(PathBuf::from(path)),
                _ => {}
            }
        }
    }
    ensure!(
        output.status.success(),
        "Offline release build failed: {}",
        output.status
    );
    Ok(Binaries {
        gateway: gateway.context("Cargo did not report the gateway executable")?,
        backend: backend.context("Cargo did not report the mock backend executable")?,
    })
}

fn cargo(root: &Path) -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(root);
    command
}

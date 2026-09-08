//! Local repository checks. Never installs services or modifies user settings.
mod size;

use anyhow::{Context, Result, bail, ensure};
use std::{path::Path, process::Command};

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask must live directly inside the workspace")?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") if args.len() == 1 => {
            size::check(root)?;
            cargo(root, &["fmt", "--all", "--check"])?;
            cargo(
                root,
                &[
                    "clippy",
                    "--locked",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            )?;
            cargo(root, &["test", "--locked", "--workspace", "--all-features"])?;
            println!("All local checks passed. Real Chrome/Codex smoke tests remain opt-in.");
        }
        Some("size") if args.len() == 1 => size::check(root)?,
        Some("help" | "--help" | "-h") if args.len() == 1 => {
            println!("cargo xtask check  # sizes, formatting, strict Clippy, mock/unit tests");
            println!("cargo xtask size   # Rust file budgets only");
        }
        _ => bail!("Usage: cargo xtask <check|size|help>"),
    }
    Ok(())
}

fn cargo(root: &Path, args: &[&str]) -> Result<()> {
    println!("Checking: cargo {}", args.join(" "));
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(args)
        .current_dir(root)
        .status()
        .context("Cannot start Cargo")?;
    ensure!(
        status.success(),
        "cargo {} failed: {status}",
        args.join(" ")
    );
    Ok(())
}

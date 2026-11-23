//! Optional commit metadata for builds from a Git checkout.
use std::{env, fs, path::Path, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MCP_GATE_RELEASE_BUILD");
    let dot_git = Path::new(".git");
    // A source archive may be unpacked inside a different checkout.
    if env::var("MCP_GATE_RELEASE_BUILD").as_deref() == Ok("1") || !dot_git.exists() {
        return;
    }
    if dot_git.is_file() {
        watch(dot_git);
    }
    if let Some(commit) = git(&["rev-parse", "--short=9", "HEAD"])
        && commit.len() >= 9
        && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        println!("cargo:rustc-env=MCP_GATE_BUILD_COMMIT={commit}");
    }
    watch_git_refs();
}

fn git(args: &[&str]) -> Option<String> {
    // Source-only builds also work when Git cannot run or resolve HEAD.
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim_end().into())
}

fn watch(path: &Path) {
    if let Some(path) = path.to_str().filter(|s| !s.contains(['\r', '\n'])) {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn watch_git_refs() -> Option<()> {
    // Git locates the worktree's own HEAD and the shared branch references.
    let paths = git(&[
        "rev-parse",
        "--git-path",
        "HEAD",
        "--git-path",
        "packed-refs",
    ])?;
    let (head, packed_refs) = paths.split_once('\n')?;
    let head = Path::new(head.trim_end_matches('\r'));
    if !head.is_file() {
        return None;
    }
    watch(head);
    let contents = fs::read_to_string(head).ok()?;
    let reference = contents.trim_end().strip_prefix("ref: ")?;
    let loose_ref = git(&["rev-parse", "--git-path", reference])?;
    let loose_ref = Path::new(&loose_ref);
    if loose_ref.is_file() {
        watch(loose_ref);
    } else {
        let packed_refs = Path::new(packed_refs);
        if packed_refs.is_file() {
            watch(packed_refs);
        }
        // Cargo treats a missing path as always changed. Watch the nearest
        // refs directory so that a later update can create the loose ref.
        let refs = packed_refs.parent()?.join("refs");
        if let Some(parent) = loose_ref
            .ancestors()
            .skip(1)
            .take_while(|path| path.starts_with(&refs))
            .find(|path| path.is_dir())
        {
            watch(parent);
        }
    }
    Some(())
}

//! Exercise real terminal keys, cancellation and secret-free previews without installing services.
#![cfg(all(unix, feature = "test-backend"))]
use serde_json::json;
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Terminal {
    child: Child,
    master: File,
    output: String,
    cursor_requests: usize,
}
impl Terminal {
    fn start(mut command: Command) -> Self {
        let mut master = -1;
        let mut slave = -1;
        let size = libc::winsize {
            ws_row: 40,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: output pointers and the window-size reference remain valid for openpty.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &size,
            )
        };
        assert_eq!(opened, 0);
        // SAFETY: successful openpty returned two fresh, independently owned descriptors.
        let master = unsafe { File::from_raw_fd(master) };
        // SAFETY: transfer sole ownership of the second fresh descriptor to File.
        let slave = unsafe { File::from_raw_fd(slave) };
        // SAFETY: master is live; only nonblocking and close-on-exec flags are changed.
        unsafe {
            assert_ne!(
                libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK),
                -1
            );
            assert_ne!(
                libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC),
                -1
            );
        }
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // SAFETY: the child closure calls only async-signal-safe session/ioctl operations before exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1")
            .spawn()
            .unwrap();
        Self {
            child,
            master,
            output: String::new(),
            cursor_requests: 0,
        }
    }
    fn send(&mut self, text: &str) {
        self.master.write_all(text.as_bytes()).unwrap();
    }
    fn read(&mut self) {
        let mut bytes = [0; 16384];
        if let Ok(count) = self.master.read(&mut bytes) {
            self.output
                .push_str(&String::from_utf8_lossy(&bytes[..count]));
        }
        let count = self.output.matches("\x1b[6n").count();
        for _ in self.cursor_requests..count {
            self.send("\x1b[1;1R");
        }
        self.cursor_requests = count;
        assert!(!self.output.contains("never-print-fixture-secret"));
    }
    fn expect(&mut self, message: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.read();
            if self.output.contains(message) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Terminal did not show {message}: {}",
                self.output
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn next(&mut self, keys: &str, message: &str) {
        self.output.clear();
        self.cursor_requests = 0;
        self.send(keys);
        self.expect(message);
    }
    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.read();
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{}", self.output);
                return;
            }
            assert!(Instant::now() < deadline, "Terminal did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture(fixed: bool) -> (tempfile::TempDir, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    for path in [".codex", "claude", ".config/opencode"] {
        fs::create_dir_all(dir.path().join(path)).unwrap();
    }
    let mut entry = json!({"command":env!("CARGO_BIN_EXE_mock-backend"),"args":[],"env":{"TOKEN":"never-print-fixture-secret"},"disabled_tools":["danger"]});
    if fixed {
        entry["cwd"] = json!(dir.path());
    }
    let original = toml::to_string(&json!({"mcp_servers":{"fixture":entry}}))
        .unwrap()
        .into_bytes();
    fs::write(dir.path().join(".codex/config.toml"), &original).unwrap();
    (dir, original)
}
fn command(root: &std::path::Path, action: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-gate"));
    command
        .arg(action)
        .current_dir(root)
        .env("MCP_GATE_TEST_HOME", root)
        .env("MCP_GATE_TEST_ROOT", root.join("state"))
        .env("CODEX_HOME", root.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env_remove("OPENCODE_CONFIG");
    command
}

#[test]
fn terminal_search_diff_back_and_cancel_preserve_settings() {
    let (dir, original) = fixture(true);
    let mut terminal = Terminal::start(command(dir.path(), "setup"));
    terminal.expect("Where do you want");
    terminal.next("\r", "Choose MCP connections");
    terminal.next("fixture \r", "Review setup:");
    terminal.next("\x1b[B\r", "<hidden>");
    terminal.expect("Review setup:");
    terminal.next("\x1b", "Choose MCP connections");
    terminal.next(" \r", "Review setup:");
    terminal.next("\x1b[B\x1b[B\x1b[B\r", "Canceled. No changes applied.");
    terminal.finish();
    assert_eq!(
        fs::read(dir.path().join(".codex/config.toml")).unwrap(),
        original
    );
    assert!(!dir.path().join("state").exists());
}

#[test]
fn terminal_ctrl_c_is_a_clean_cancellation() {
    let (dir, original) = fixture(true);
    let mut terminal = Terminal::start(command(dir.path(), "setup"));
    terminal.expect("Where do you want");
    terminal.next("\x03", "Canceled. No changes applied.");
    terminal.finish();
    assert_eq!(
        fs::read(dir.path().join(".codex/config.toml")).unwrap(),
        original
    );
    assert!(!dir.path().join("state").exists());
}

#[test]
fn terminal_unknown_directory_requires_explicit_choice_before_preview() {
    let (dir, original) = fixture(false);
    let mut terminal = Terminal::start(command(dir.path(), "setup"));
    terminal.expect("Where do you want");
    terminal.next("\r", "Choose MCP connections");
    terminal.next(" \r", "Backend working directory:");
    terminal.next("\r", "Fixed backend directory");
    terminal.next(&format!("{}\r", dir.path().display()), "Context change:");
    terminal.expect("Review setup:");
    terminal.next("\x1b[B\x1b[B\x1b[B\r", "Canceled. No changes applied.");
    terminal.finish();
    assert_eq!(
        fs::read(dir.path().join(".codex/config.toml")).unwrap(),
        original
    );
    assert!(!dir.path().join("state").exists());
}

#[test]
fn terminal_remove_all_reviews_owned_connections_and_can_cancel() {
    use mcp_gate::{
        clients::{self, Client, document},
        manage::store::{self, Record, Registry},
    };
    let (dir, _) = fixture(true);
    let root = dir.path();
    let mut binding = clients::discover(root, root, &[Client::Codex], false)
        .unwrap()
        .remove(0)
        .binding;
    binding.after = json!({"url":"http://127.0.0.1:49871/mcp","http_headers":{"Authorization":"never-print-fixture-secret"},"disabled_tools":["danger"]});
    let original = fs::read_to_string(&binding.target).unwrap();
    let configured = document::replace(
        &original,
        true,
        &binding.path,
        &binding.before,
        &binding.after,
    )
    .unwrap();
    fs::write(&binding.target, &configured).unwrap();
    let state = root.join("state");
    store::prepare(&state).unwrap();
    let registry = Registry {
        format_version: 1,
        gateways: vec![Record {
            id: "fixture".into(),
            release: state.join("fixture"),
            label: "local.mcp-gate.fixture-preview-only".into(),
            binary_hash: "not-a-real-service".into(),
            binary: env!("CARGO_BIN_EXE_mcp-gate").into(),
            removing: false,
            bindings: vec![binding],
        }],
    };
    store::save(&state, &registry).unwrap();
    let original_registry = fs::read(state.join("registry.json")).unwrap();
    let mut terminal = Terminal::start(command(root, "remove"));
    terminal.expect("Restore direct MCP connections:");
    terminal.next("\x1b[B\r", "Review removal:");
    assert!(terminal.output.contains("restore 1 direct connections"));
    terminal.next("\x1b[B\r", "<hidden>");
    terminal.expect("Review removal:");
    terminal.next("\x1b[B\x1b[B\x1b[B\r", "Canceled. No changes applied.");
    terminal.finish();
    assert_eq!(
        fs::read_to_string(root.join(".codex/config.toml")).unwrap(),
        configured
    );
    assert_eq!(
        fs::read(state.join("registry.json")).unwrap(),
        original_registry
    );
    let dry = command(root, "remove")
        .args(["--all", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(dry.status.success());
    let result: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(result["connections"].as_array().unwrap().len(), 1);
    assert_eq!(
        fs::read(state.join("registry.json")).unwrap(),
        original_registry
    );
}

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
    slave: File,
    output: String,
    cursor_requests: usize,
    last_termios: String,
    debug_log: Option<std::path::PathBuf>,
}
impl Terminal {
    fn start(mut command: Command) -> Self {
        let mut master = -1;
        let mut slave = -1;
        let mut size = libc::winsize {
            ws_row: 40,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: output pointers and the window-size reference remain valid for openpty.
        // addr_of_mut matches macOS (*mut winsize) and Linux (*const winsize).
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::addr_of_mut!(size),
            )
        };
        assert_eq!(opened, 0);
        // SAFETY: successful openpty returned two fresh, independently owned descriptors.
        let master = unsafe { File::from_raw_fd(master) };
        // SAFETY: transfer sole ownership of the second fresh descriptor to File.
        let slave = unsafe { File::from_raw_fd(slave) };
        // SAFETY: master is live; only nonblocking and close-on-exec flags are changed.
        // The slave starts in raw mode so Enter is delivered immediately. A default
        // canonical PTY can hold CR until the child enables raw mode; Ctrl-C still
        // works in that window because ISIG is on, which is what failed on ARM CI.
        unsafe {
            assert_ne!(
                libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK),
                -1
            );
            assert_ne!(
                libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC),
                -1
            );
            let mut termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave.as_raw_fd(), &mut termios), 0);
            libc::cfmakeraw(&mut termios);
            assert_eq!(
                libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios),
                0
            );
        }
        let debug_log = command
            .get_current_dir()
            .map(|path| path.join("tty-debug.log"));
        let slave_keep = slave.try_clone().unwrap();
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        // SAFETY: the child closure calls only async-signal-safe session/ioctl operations before exec.
        unsafe {
            command.pre_exec(|| {
                // ioctl's request type is c_ulong on glibc and macOS but c_int on musl.
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
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
            slave: slave_keep,
            output: String::new(),
            cursor_requests: 0,
            last_termios: String::new(),
            debug_log,
        }
    }
    fn describe_termios(fd: i32, label: &str) -> String {
        // SAFETY: the termios value is zeroed and then fully overwritten by tcgetattr
        // before any field is read.
        let mut termios = unsafe { std::mem::zeroed() };
        // SAFETY: the pointer refers to the local termios above, valid for this call.
        let rc = unsafe { libc::tcgetattr(fd, &mut termios) };
        if rc != 0 {
            return format!(
                "{label} fd={fd} tcgetattr failed: {}",
                std::io::Error::last_os_error()
            );
        }
        format!(
            "{label} fd={fd} ICANON={} ECHO={} ICRNL={} ISIG={} VMIN={} VTIME={}",
            termios.c_lflag & libc::ICANON != 0,
            termios.c_lflag & libc::ECHO != 0,
            termios.c_iflag & libc::ICRNL != 0,
            termios.c_lflag & libc::ISIG != 0,
            termios.c_cc[libc::VMIN],
            termios.c_cc[libc::VTIME]
        )
    }
    fn termios_snapshot(&self) -> String {
        format!(
            "{}; {}",
            Self::describe_termios(self.master.as_raw_fd(), "master"),
            Self::describe_termios(self.slave.as_raw_fd(), "slave")
        )
    }
    fn termios_report(&self) -> String {
        let debug = self
            .debug_log
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .unwrap_or_default();
        let setup = self
            .debug_log
            .as_ref()
            .and_then(|path| fs::read_to_string(path.with_file_name("setup-debug.log")).ok())
            .unwrap_or_default();
        format!(
            "{}; last_at_send={}; MCP_GATE_DEBUG_TTY:\n{debug}\nMCP_GATE_DEBUG_SETUP:\n{setup}\n{}",
            self.termios_snapshot(),
            self.last_termios,
            self.child_diagnostics()
        )
    }
    fn child_diagnostics(&self) -> String {
        let pid = self.child.id();
        #[allow(unused_mut)]
        let mut report = format!("child pid={pid}\n");
        #[cfg(target_os = "linux")]
        {
            report.push_str(&linux_process_tree(pid, 0));
            if let Ok(output) = Command::new("ps")
                .args([
                    "-o",
                    "pid,ppid,pgid,stat,wchan:32,etime,args",
                    "--forest",
                    "--ppid",
                    &pid.to_string(),
                    "-p",
                    &pid.to_string(),
                ])
                .output()
            {
                report.push_str("ps:\n");
                report.push_str(&String::from_utf8_lossy(&output.stdout));
                if !output.stderr.is_empty() {
                    report.push_str(&String::from_utf8_lossy(&output.stderr));
                }
            }
        }
        report
    }
    fn write_master(&mut self, text: &str) {
        self.master.write_all(text.as_bytes()).unwrap();
    }
    fn send(&mut self, text: &str) {
        // Snapshot termios at the moment test keys are written, not later CPR replies.
        self.last_termios = self.termios_snapshot();
        self.write_master(text);
    }
    fn read(&mut self) {
        let mut bytes = [0; 16384];
        if let Ok(count) = self.master.read(&mut bytes) {
            self.output
                .push_str(&String::from_utf8_lossy(&bytes[..count]));
        }
        let count = self.output.matches("\x1b[6n").count();
        for _ in self.cursor_requests..count {
            self.write_master("\x1b[1;1R");
        }
        self.cursor_requests = count;
        assert!(!self.output.contains("never-print-fixture-secret"));
    }
    fn settle(&mut self) {
        // Wait until drawing and cursor-position reports have gone quiet, so
        // the next key is not sent while the UI is blocked on a CPR reply.
        let deadline = Instant::now() + Duration::from_millis(750);
        let mut last_len = self.output.len();
        let mut last_cpr = self.cursor_requests;
        let mut quiet_since = Instant::now();
        loop {
            self.read();
            if self.output.len() != last_len || self.cursor_requests != last_cpr {
                last_len = self.output.len();
                last_cpr = self.cursor_requests;
                quiet_since = Instant::now();
            } else if quiet_since.elapsed() >= Duration::from_millis(120) {
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn expect(&mut self, message: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.read();
            if self.output.contains(message) {
                self.settle();
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Terminal did not show {message}: {}\n{}",
                self.output,
                self.termios_report()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn next(&mut self, keys: &str, message: &str) {
        self.settle();
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
                assert!(
                    status.success(),
                    "{}\n{}",
                    self.output,
                    self.termios_report()
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Terminal did not exit\n{}\n{}",
                self.output,
                self.termios_report()
            );
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

#[cfg(target_os = "linux")]
fn linux_process_tree(pid: u32, depth: usize) -> String {
    use std::collections::BTreeSet;
    let prefix = "  ".repeat(depth);
    let base = format!("/proc/{pid}");
    let cmdline = fs::read(format!("{base}/cmdline"))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .unwrap_or_else(|| "<unavailable>".into());
    let wchan = fs::read_to_string(format!("{base}/wchan"))
        .unwrap_or_else(|_| "<unavailable>".into())
        .trim()
        .to_owned();
    let state = fs::read_to_string(format!("{base}/status"))
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("State:"))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "State: <unavailable>".into());
    let mut report = format!("{prefix}pid={pid} {state} wchan={wchan} cmdline={cmdline}\n");
    if let Ok(tasks) = fs::read_dir(format!("{base}/task")) {
        for task in tasks.flatten() {
            let tid = task.file_name();
            match fs::read_to_string(task.path().join("stack")) {
                Ok(stack) if !stack.trim().is_empty() => {
                    report.push_str(&format!(
                        "{prefix}  task {} stack:\n",
                        tid.to_string_lossy()
                    ));
                    for line in stack.lines() {
                        report.push_str(&format!("{prefix}    {line}\n"));
                    }
                }
                Ok(_) => report.push_str(&format!(
                    "{prefix}  task {} stack: <empty>\n",
                    tid.to_string_lossy()
                )),
                Err(error) => report.push_str(&format!(
                    "{prefix}  task {} stack: {error}\n",
                    tid.to_string_lossy()
                )),
            }
        }
    }
    let mut children = BTreeSet::new();
    if let Ok(tasks) = fs::read_dir(format!("{base}/task")) {
        for task in tasks.flatten() {
            if let Ok(text) = fs::read_to_string(task.path().join("children")) {
                for child in text.split_whitespace() {
                    if let Ok(child) = child.parse::<u32>() {
                        children.insert(child);
                    }
                }
            }
        }
    }
    for child in children {
        report.push_str(&linux_process_tree(child, depth + 1));
    }
    report
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
        .env("XDG_DATA_HOME", root.join(".local/share"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("MCP_GATE_DEBUG_TTY", root.join("tty-debug.log"))
        .env("MCP_GATE_DEBUG_SETUP", root.join("setup-debug.log"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
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
    let mut binding = clients::discover(root, root, &[Client::CODEX], false)
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
    assert!(terminal.output.contains("restore 1 direct connection"));
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

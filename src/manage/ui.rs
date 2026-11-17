//! Terminal interaction stays separate from JSON output and lifecycle operations.
use super::Selection;
use anyhow::{Result, ensure};
use inquire::{InquireError, MultiSelect, Select, Text};
use std::io::IsTerminal;

#[derive(Debug)]
pub struct Canceled;
impl std::fmt::Display for Canceled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Canceled. No changes applied.")
    }
}
impl std::error::Error for Canceled {}

pub fn interactive(options: &Selection) -> bool {
    !options.yes && !options.json && !options.dry_run && terminal()
}
pub fn terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}
pub fn counted(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}
pub fn safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn answer<T>(result: Result<T, InquireError>) -> Result<T> {
    match result {
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            Err(Canceled.into())
        }
        Err(_) => anyhow::bail!(
            "Cannot read terminal input. Retry in a terminal, or use explicit --client/--server with --yes."
        ),
        Ok(value) => Ok(value),
    }
}
pub fn directory(message: &str) -> Result<Option<std::path::PathBuf>> {
    loop {
        let Some(path) = answer(Text::new(message).prompt_skippable())? else {
            return Ok(None);
        };
        let path = if let Some(tail) = path.strip_prefix("~/") {
            super::store::home()?.join(tail)
        } else {
            path.into()
        };
        if path.is_absolute() && path.is_dir() {
            return Ok(Some(crate::platform::project_path(&path)?));
        }
        println!("Enter an existing absolute directory, or press Esc to go back.");
    }
}
pub fn select(message: &str, options: Vec<String>) -> Result<Option<usize>> {
    answer(
        Select::new(message, options)
            .with_page_size(10)
            .raw_prompt_skippable(),
    )
    .map(|v| v.map(|v| v.index))
}
pub fn multiple(message: &str, options: Vec<String>) -> Result<Option<Vec<usize>>> {
    answer(
        MultiSelect::new(message, options)
            .with_page_size(12)
            .with_help_message(
                "Type to search · arrows move · Space selects · Enter reviews · Esc goes back",
            )
            .with_formatter(&|values| format!("{} selected", values.len()))
            .raw_prompt_skippable(),
    )
    .map(|v| v.map(|v| v.into_iter().map(|v| v.index).collect()))
}
pub fn confirm(message: &str) -> Result<bool> {
    ensure!(
        terminal(),
        "No changes applied. Use a terminal for the wizard, --dry-run to preview, or explicit --client/--server with --yes."
    );
    answer(
        inquire::Confirm::new(message)
            .with_default(false)
            .prompt_skippable(),
    )
    .map(|v| v.unwrap_or(false))
}
pub fn scope(options: &mut Selection) -> Result<bool> {
    if options.project.is_some() {
        return Ok(true);
    }
    let current = crate::platform::project_path(&std::env::current_dir()?)?;
    let home = crate::clients::identity(&super::store::home()?);
    let show_current = current != home;
    loop {
        let mut labels = vec!["Personal settings on this computer".into()];
        if show_current {
            labels.push(format!("Project: {}", safe(&current.display().to_string())));
        }
        labels.extend(["Another project directory".into(), "Cancel".into()]);
        match select("Where do you want to set up MCP connections?", labels)? {
            Some(0) => return Ok(true),
            Some(1) if show_current => {
                options.project = Some(current);
                return Ok(true);
            }
            Some(index) if index == 1 + usize::from(show_current) => {
                let Some(path) = directory("Project directory (absolute path):")? else {
                    continue;
                };
                if path == home {
                    println!(
                        "Home uses personal settings; choose a project directory or personal settings."
                    );
                    continue;
                }
                options.project = Some(path);
                return Ok(true);
            }
            _ => return Ok(false),
        }
    }
}

/// Typeahead between prompts is echoed by the terminal and lands on the next
/// status line (`mailFound 3 connections`). Suppress echo until the next prompt
/// reads those bytes itself. Inquire restores the mode it observed, so this
/// stays off across the gap and is restored when the guard drops.
pub struct QuietInput {
    restore_echo: bool,
}

impl QuietInput {
    pub fn suppress() -> Self {
        debug_tty("QuietInput::suppress: start");
        let restore_echo = terminal() && disable_echo();
        debug_tty(&format!(
            "QuietInput::suppress: restore_echo={restore_echo}"
        ));
        Self { restore_echo }
    }
}

impl Drop for QuietInput {
    fn drop(&mut self) {
        if self.restore_echo {
            debug_tty("QuietInput::drop: restoring echo");
            enable_echo();
        } else {
            debug_tty("QuietInput::drop: no echo restore");
        }
    }
}

pub fn fresh_line() {
    if !terminal() {
        return;
    }
    print!("\r\x1b[2K");
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

fn disable_echo() -> bool {
    set_echo(false).unwrap_or(false)
}

fn enable_echo() {
    let _ = set_echo(true);
}

fn debug_tty(message: &str) {
    let Ok(dest) = std::env::var("MCP_GATE_DEBUG_TTY") else {
        return;
    };
    if dest.is_empty() {
        return;
    }
    let line = format!("{message}\n");
    if dest == "1" {
        eprint!("{line}");
        return;
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&dest)
        .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
}

#[cfg(unix)]
fn tty_flags(termios: &libc::termios) -> String {
    format!(
        "ICANON={} ECHO={} ICRNL={} ISIG={} VMIN={} VTIME={}",
        termios.c_lflag & libc::ICANON != 0,
        termios.c_lflag & libc::ECHO != 0,
        termios.c_iflag & libc::ICRNL != 0,
        termios.c_lflag & libc::ISIG != 0,
        termios.c_cc[libc::VMIN],
        termios.c_cc[libc::VTIME]
    )
}

#[cfg(unix)]
fn set_echo(on: bool) -> Option<bool> {
    // SAFETY: the termios value is zeroed and then fully overwritten by tcgetattr
    // before any field is read. Standard input is the process terminal.
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: the pointer refers to the local termios above, valid for this call.
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut termios) } != 0 {
        debug_tty(&format!(
            "set_echo({on}): tcgetattr failed: {}",
            std::io::Error::last_os_error()
        ));
        return None;
    }
    debug_tty(&format!("set_echo({on}): before {}", tty_flags(&termios)));
    let was = termios.c_lflag & libc::ECHO != 0;
    if on {
        termios.c_lflag |= libc::ECHO;
    } else {
        termios.c_lflag &= !libc::ECHO;
    }
    // SAFETY: termios was filled by tcgetattr on this same descriptor.
    // Only the ECHO bit is changed; this is still a full-struct tcsetattr, so
    // it must not run while another layer is switching raw/canonical mode.
    if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &termios) } != 0 {
        debug_tty(&format!(
            "set_echo({on}): tcsetattr failed: {}",
            std::io::Error::last_os_error()
        ));
        return None;
    }
    // SAFETY: the termios value is zeroed and then fully overwritten by tcgetattr
    // before any field is read.
    let mut after = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: the pointer refers to the local termios above, valid for this call.
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut after) } == 0 {
        debug_tty(&format!("set_echo({on}): after  {}", tty_flags(&after)));
    }
    Some(was)
}

#[cfg(windows)]
fn set_echo(on: bool) -> Option<bool> {
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };
    // SAFETY: STD_INPUT_HANDLE is the process input handle. The mode word is
    // written by GetConsoleMode before it is read.
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return None;
        }
        let was = mode & ENABLE_ECHO_INPUT != 0;
        let next = if on {
            mode | ENABLE_ECHO_INPUT
        } else {
            mode & !ENABLE_ECHO_INPUT
        };
        if SetConsoleMode(handle, next) == 0 {
            return None;
        }
        Some(was)
    }
}

pub fn canceled(json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({"format_version":1,"phase":"canceled"})
        );
    } else {
        println!("Canceled. No changes applied.");
    }
}

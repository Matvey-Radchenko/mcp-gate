//! Recognize and format gateway headers-helper commands.
use crate::clients::Client;
use std::path::{Component, Path, PathBuf};

fn quote(path: &Path) -> String {
    if cfg!(windows) {
        double_quote(path)
    } else {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }
}

fn double_quote(path: &Path) -> String {
    let mut quoted = String::from("\"");
    for ch in path.to_string_lossy().chars() {
        if cfg!(not(windows)) && matches!(ch, '"' | '\\' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}
pub fn command(binary: &Path, config: &Path) -> String {
    format!("{} headers --config {}", quote(binary), quote(config))
}

pub(super) fn field(client: Client) -> Option<&'static str> {
    match client {
        Client::Codex => Some("http_headers_helper"),
        Client::ClaudeCode => Some("headersHelper"),
        // OpenCode consumes a file-backed Authorization header.
        Client::Opencode => None,
    }
}

pub(super) struct Invocation<'a> {
    pub executable: PathBuf,
    pub prefix: &'a str,
    pub arguments: &'a str,
    leading: &'a str,
}

impl Invocation<'_> {
    pub fn replacement(&self, path: &Path) -> String {
        // Keep single quotes out of the replacement when the old executable
        // was bare or double-quoted. A TOML literal can then keep its delimiter.
        let executable = if self.prefix.trim_start().starts_with('\'') {
            quote(path)
        } else {
            double_quote(path)
        };
        format!("{}{executable}", self.leading)
    }
}

pub(super) fn invocation(command: &str) -> Option<Invocation<'_>> {
    let text = command.trim_start();
    let leading = &command[..command.len() - text.len()];
    let (executable, end) = executable_word(text)?;
    let arguments = &text[end..];
    if !arguments.starts_with(char::is_whitespace) {
        return None;
    }
    let mut words = arguments.split_whitespace();
    if words.next()? != "headers" || words.next()? != "--config" || words.next().is_none() {
        return None;
    }
    Some(Invocation {
        executable: executable.into(),
        prefix: &command[..leading.len() + end],
        arguments,
        leading,
    })
}

pub(super) fn executable(client: Client, entry: &serde_json::Value) -> Option<Invocation<'_>> {
    entry.get(field(client)?)?.as_str().and_then(invocation)
}

#[cfg(windows)]
fn executable_word(text: &str) -> Option<(String, usize)> {
    let (path, end) = if let Some(quoted) = text.strip_prefix('"') {
        let end = quoted.find('"')?;
        (&quoted[..end], end + 2)
    } else {
        let end = text.find(char::is_whitespace)?;
        (&text[..end], end)
    };
    Some((path.to_owned(), end))
}

#[cfg(not(windows))]
fn executable_word(text: &str) -> Option<(String, usize)> {
    let mut chars = text.char_indices();
    let mut path = String::new();
    let mut quoting = None;
    while let Some((index, ch)) = chars.next() {
        match (quoting, ch) {
            (None, ch) if ch.is_whitespace() => return Some((path, index)),
            (None, '\'' | '"') => quoting = Some(ch),
            (Some(quote), ch) if quote == ch => quoting = None,
            (None | Some('"'), '\\') => {
                let (_, escaped) = chars.next()?;
                if quoting.is_some() && !matches!(escaped, '\\' | '"' | '$' | '`') {
                    path.push('\\');
                }
                path.push(escaped);
            }
            (None | Some('"'), '$' | '`') => return None,
            (None, ';' | '|' | '&' | '<' | '>' | '*' | '?' | '[' | '(' | ')') => return None,
            _ => path.push(ch),
        }
    }
    None
}

/// The store owns these executable names. Historical locations can be missing
/// already, so recognize the managed layout without launching an old helper.
pub(super) fn versioned(root: &Path, executable: &Path) -> bool {
    let store = root.join("binaries");
    let Ok(relative) = executable.strip_prefix(store) else {
        return false;
    };
    let mut parts = relative.components();
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(Component::Normal(version)), Some(Component::Normal(name)), None)
            if version.to_str().is_some_and(version_directory)
                && name == crate::platform::binary_name()
    )
}

fn version_directory(name: &str) -> bool {
    name == "legacy"
        || name
            .split_once("-local-recovery-")
            .is_some_and(|(version, suffix)| !version.is_empty() && !suffix.is_empty())
        || name.rsplit_once('-').is_some_and(|(version, digest)| {
            !version.is_empty()
                && digest.len() == 64
                && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

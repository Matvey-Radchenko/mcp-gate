//! Render private service fragments and read only literal ExecStart identities.
use super::Record;
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

pub(super) fn render(record: &Record) -> Result<String> {
    Ok(format!(
        "[Service]\nType=exec\nExecStart={} serve --config {}\n\
         Restart=always\nRestartSec=10s\nTimeoutStopSec=20s\nUMask=0077\n\
         StandardOutput=journal\nStandardError=journal\nKillMode=mixed\n\n\
         [Install]\nWantedBy=default.target\n",
        quote(&record.binary, true)?,
        quote(&record.config(), false)?,
    ))
}

fn quote(path: &Path, executable: bool) -> Result<String> {
    ensure!(path.is_absolute(), "Service paths must be absolute");
    let path = path.to_str().context("Service paths must be UTF-8")?;
    ensure!(
        !path.chars().any(char::is_control),
        "Service paths cannot contain control characters"
    );
    // ':' disables environment expansion for the whole command, including
    // argv[0]. Doubling '$' in the executable would change the path systemd opens.
    let mut encoded = String::from(if executable { "\":" } else { "\"" });
    for c in path.chars() {
        match c {
            '"' | '\\' => encoded.push('\\'),
            '%' => encoded.push('%'),
            _ => {}
        }
        encoded.push(c);
    }
    encoded.push('"');
    Ok(encoded)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::manage) struct Identity {
    pub binary: PathBuf,
    pub config: PathBuf,
}

impl Identity {
    pub(super) fn matches(&self, record: &Record) -> bool {
        self.binary == record.binary && self.config == record.config()
    }
}

pub(super) fn identity(fragment: &Path, drop_ins: &str) -> Option<Identity> {
    let mut commands = Vec::new();
    let paths = words(drop_ins, true)?;
    for path in std::iter::once(fragment).chain(paths.iter().map(Path::new)) {
        // The manager supplies local files in load order. Never read devices
        // or run a helper to interpret a service definition.
        if !path.is_absolute() || !path.metadata().ok()?.is_file() {
            return None;
        }
        read_commands(&std::fs::read_to_string(path).ok()?, &mut commands)?;
    }
    let [command] = commands.as_slice() else {
        return None;
    };
    literal(command)
}

fn read_commands(text: &str, commands: &mut Vec<String>) -> Option<()> {
    if text.contains('\0') {
        return None;
    }
    let mut service = false;
    let mut pending = String::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for line in text.lines() {
        if line
            .trim_start_matches([' ', '\t', '\r'])
            .starts_with(['#', ';'])
        {
            continue;
        }
        pending.push_str(line);
        let trailing_backslashes = pending.bytes().rev().take_while(|&b| b == b'\\').count();
        if trailing_backslashes % 2 == 1 {
            pending.pop();
            pending.push(' ');
            continue;
        }
        let line = pending.trim_matches([' ', '\t', '\r']);
        if line.starts_with('[') {
            service = line == "[Service]";
        } else if service
            && let Some((key, value)) = line.split_once('=')
            && key.trim_end_matches([' ', '\t', '\r']) == "ExecStart"
        {
            let value = value.trim_matches([' ', '\t', '\r']);
            if value.is_empty() {
                commands.clear();
            } else {
                commands.push(value.to_owned());
            }
        }
        pending.clear();
    }
    pending.is_empty().then_some(())
}

fn literal(command: &str) -> Option<Identity> {
    let words = words(command, false)?;
    let [binary, serve, flag, config] = words.as_slice() else {
        return None;
    };
    let literal_env = binary.starts_with(':');
    let binary = undouble(binary.strip_prefix(':').unwrap_or(binary), '%')?;
    let config = undouble(config, '%')?;
    let config = if literal_env {
        config
    } else {
        undouble(&config, '$')?
    };
    let binary = PathBuf::from(binary);
    let config = PathBuf::from(config);
    (serve == "serve" && flag == "--config" && binary.is_absolute() && config.is_absolute())
        .then_some(Identity { binary, config })
}

fn undouble(value: &str, special: char) -> Option<String> {
    let mut chars = value.chars();
    let mut decoded = String::new();
    while let Some(c) = chars.next() {
        if c == special && chars.next() != Some(special) {
            return None;
        }
        decoded.push(c);
    }
    Some(decoded)
}

/// Parse a conservative subset of systemd.syntax(7), or systemctl's quoted
/// DropInPaths. Reject unfamiliar forms instead of assigning them an identity.
fn words(mut input: &str, property: bool) -> Option<Vec<String>> {
    let mut result = Vec::new();
    loop {
        input = input.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if input.is_empty() {
            return Some(result);
        }
        let (word, rest) = word(input, property)?;
        result.push(word);
        input = rest;
    }
}

fn word(input: &str, property: bool) -> Option<(String, &str)> {
    let mut chars = input.chars();
    let quote = match chars.clone().next()? {
        c @ ('"' | '\'') => {
            chars.next();
            Some(c)
        }
        _ => None,
    };
    let mut value = String::new();
    while let Some(c) = chars.next() {
        match c {
            c if Some(c) == quote => {
                let rest = chars.as_str();
                return (rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace()))
                    .then_some((value, rest));
            }
            c if quote.is_none() && c.is_ascii_whitespace() => break,
            '"' | '\'' if quote.is_none() => return None,
            '\\' => value.push(match chars.next()? {
                c @ ('\\' | '"' | '\'') => c,
                c @ ('$' | '`') if property => c,
                's' => ' ',
                't' => '\t',
                _ => return None,
            }),
            c => value.push(c),
        }
    }
    quote.is_none().then_some((value, chars.as_str()))
}

//! Render private service fragments and read only literal ExecStart identities.
use super::Record;
use anyhow::{Context, Result, ensure};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

pub(super) fn render(record: &Record) -> Result<String> {
    Ok(format!(
        "[Service]\nType=exec\nExecStart={} serve --config {}\n\
         Restart=always\nRestartSec=10s\nTimeoutStopSec=20s\nUMask=0077\n\
         StandardOutput=journal\nStandardError=journal\nKillMode=mixed\n\n\
         [Install]\nWantedBy=default.target\n",
        quote(&record.binary, ":")?,
        quote(&record.config(), "")?,
    ))
}

fn quote(path: &Path, prefix: &str) -> Result<String> {
    ensure!(path.is_absolute(), "Service paths must be absolute");
    let path = path.to_str().context("Service paths must be UTF-8")?;
    ensure!(
        !path.chars().any(char::is_control),
        "Service paths cannot contain control characters"
    );
    // ':' disables environment expansion for the whole command, including
    // argv[0]. Doubling '$' in the executable would change the path systemd opens.
    let mut encoded = String::from("\"");
    encoded.push_str(prefix);
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
    effective(fragment, drop_ins).map(|(identity, _, _)| identity)
}

pub(super) fn effective(fragment: &Path, drop_ins: &str) -> Option<(Identity, PathBuf, String)> {
    let mut commands = Vec::new();
    let paths = words(drop_ins, true)?;
    for path in std::iter::once(fragment).chain(paths.iter().map(Path::new)) {
        // The manager supplies local files in load order. Never read devices
        // or run a helper to interpret a service definition.
        if !path.is_absolute() || !path.metadata().ok()?.is_file() {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        let entries = commands_in(&text)?;
        for entry in entries {
            match entry {
                Some(command) => commands.push((path.to_path_buf(), command, text.clone())),
                None => commands.clear(),
            }
        }
    }
    let [(path, command, text)] = commands.as_slice() else {
        return None;
    };
    Some((literal(&command.value)?, path.clone(), text.clone()))
}

struct Command {
    value: String,
    executable: Option<Range<usize>>,
}

pub(super) fn with_binary(text: &str, old: &Path, target: &Path) -> Result<String> {
    let mut commands = Vec::new();
    for command in commands_in(text).context("Invalid systemd unit; values hidden")? {
        match command {
            Some(command) => commands.push(command),
            None => commands.clear(),
        }
    }
    let [command] = commands.as_slice() else {
        anyhow::bail!("Expected a single effective ExecStart in the gateway unit");
    };
    let identity =
        literal(&command.value).context("Unsupported gateway ExecStart; values hidden")?;
    ensure!(identity.binary == old, "Systemd gateway executable changed");
    let (first, _) = word(&command.value, false).context("Unsupported gateway ExecStart")?;
    let range = command
        .executable
        .clone()
        .context("Systemd executable spans multiple lines")?;
    let mut replacement = text.to_owned();
    let (prefix, _) = executable_parts(&first);
    replacement.replace_range(range, &quote(target, prefix)?);
    Ok(replacement)
}

fn commands_in(text: &str) -> Option<Vec<Option<Command>>> {
    if text.contains('\0') {
        return None;
    }
    let mut commands = Vec::new();
    let mut service = false;
    let mut pending = String::new();
    let mut segments = Vec::new();
    let mut offset = 0;
    for physical in text.split_inclusive('\n') {
        let line_start = offset;
        offset += physical.len();
        let line = physical.trim_end_matches(['\r', '\n']);
        let line = if line_start == 0 {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        if line.trim_start_matches([' ', '\t']).starts_with(['#', ';']) {
            continue;
        }
        let bom = usize::from(line_start == 0 && physical.starts_with('\u{feff}'))
            * '\u{feff}'.len_utf8();
        segments.push((pending.len(), line_start + bom, line.len()));
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
        } else if service && let Some(command) = exec_start(&pending, &segments) {
            commands.push(command);
        }
        pending.clear();
        segments.clear();
    }
    pending.is_empty().then_some(commands)
}

fn exec_start(text: &str, segments: &[(usize, usize, usize)]) -> Option<Option<Command>> {
    let (key, value) = text.split_once('=')?;
    if key.trim_matches([' ', '\t', '\r']) != "ExecStart" {
        return None;
    }
    let value = value.trim_start_matches([' ', '\t', '\r']);
    if value.is_empty() {
        return Some(None);
    }
    let start = text.len() - value.len();
    let executable = word(value, false).and_then(|(_, rest)| {
        let end = text.len() - rest.len();
        segments.iter().find_map(|&(logical, physical, len)| {
            (start >= logical && end <= logical + len)
                .then(|| physical + start - logical..physical + end - logical)
        })
    });
    Some(Some(Command {
        value: value.to_owned(),
        executable,
    }))
}

fn literal(command: &str) -> Option<Identity> {
    let words = words(command, false)?;
    let (binary, arguments) = words.split_first()?;
    let (prefix, binary) = executable_parts(binary);
    let literal_env = prefix.contains(':');
    let binary = undouble(binary, '%')?;
    let skip_argv0 = usize::from(prefix.contains('@'));
    let config =
        super::super::config_argument(arguments.iter().skip(skip_argv0).map(String::as_str))?;
    let config = undouble(config.to_str()?, '%')?;
    let config = if literal_env {
        config
    } else {
        undouble(&config, '$')?
    };
    let binary = PathBuf::from(binary);
    binary.is_absolute().then_some(Identity {
        binary,
        config: PathBuf::from(config),
    })
}

fn executable_parts(executable: &str) -> (&str, &str) {
    let path = executable.trim_start_matches([':', '-', '@', '+', '!']);
    executable.split_at(executable.len() - path.len())
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
            c if quote.is_none() && c.is_ascii_whitespace() => {
                return Some((
                    value,
                    &input[input.len() - chars.as_str().len() - c.len_utf8()..],
                ));
            }
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

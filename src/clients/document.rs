//! Minimal semantic edits: preserve all bytes outside the selected MCP entry.
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::ops::Range;

#[derive(Clone)]
struct Token {
    range: Range<usize>,
    text: String,
}

fn tokens(source: &str) -> Result<(Vec<Token>, Vec<String>)> {
    let bytes = source.as_bytes();
    let mut at = 0;
    let mut out = Vec::new();
    let mut comments = Vec::new();
    while at < bytes.len() {
        let start = at;
        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if source[at..].starts_with("//") {
            at = source[at..].find('\n').map_or(bytes.len(), |n| at + n);
            comments.push(source[start..at].into());
            continue;
        }
        if source[at..].starts_with("/*") {
            at += source[at + 2..]
                .find("*/")
                .context("Unclosed JSON comment")?
                + 4;
            comments.push(source[start..at].into());
            continue;
        }
        if bytes[at] == b'"' {
            at += 1;
            while at < bytes.len() && bytes[at] != b'"' {
                if bytes[at] == b'\\' {
                    at += 1;
                }
                at += 1;
            }
            ensure!(at < bytes.len(), "Unclosed JSON string");
            at += 1;
        } else if b"{}[]:,".contains(&bytes[at]) {
            at += 1;
        } else {
            while at < bytes.len()
                && !bytes[at].is_ascii_whitespace()
                && !b"{}[]:,".contains(&bytes[at])
            {
                at += 1;
            }
        }
        let text = source
            .get(start..at)
            .context("Invalid JSON encoding")?
            .to_string();
        out.push(Token {
            range: start..at,
            text,
        });
    }
    Ok((out, comments))
}

fn value_end(tokens: &[Token], start: usize) -> Result<usize> {
    let first = tokens.get(start).context("Missing JSON value")?;
    if first.text != "{" && first.text != "[" {
        return Ok(start + 1);
    }
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate().skip(start) {
        match token.text.as_str() {
            "{" | "[" => depth += 1,
            "}" | "]" => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i + 1);
                }
            }
            _ => {}
        }
    }
    bail!("Unclosed JSON container")
}

fn locate(tokens: &[Token], object: usize, path: &[String]) -> Result<(usize, usize)> {
    ensure!(
        tokens[object].text == "{",
        "Configuration path is not an object"
    );
    let mut at = object + 1;
    while at < tokens.len() && tokens[at].text != "}" {
        let key: String =
            serde_json::from_str(&tokens[at].text).context("JSONC keys must be quoted")?;
        ensure!(
            tokens.get(at + 1).is_some_and(|t| t.text == ":"),
            "Missing JSON colon"
        );
        let start = at + 2;
        let end = value_end(tokens, start)?;
        if key == path[0] {
            return if path.len() == 1 {
                Ok((start, end))
            } else {
                locate(tokens, start, &path[1..])
            };
        }
        at = end;
        if tokens.get(at).is_some_and(|t| t.text == ",") {
            at += 1;
        }
    }
    bail!("Configuration entry no longer exists")
}

pub fn parse(source: &str, toml: bool) -> Result<Value> {
    if toml {
        let value: toml::Value = toml::from_str(source)
            .map_err(|_| anyhow::anyhow!("Invalid TOML configuration; values hidden"))?;
        Ok(serde_json::to_value(value)?)
    } else {
        json5::from_str(source)
            .map_err(|_| anyhow::anyhow!("Invalid JSON/JSONC configuration; values hidden"))
    }
}

pub fn entry(source: &str, toml: bool, path: &[String]) -> Result<Value> {
    let mut value = parse(source, toml)?;
    for key in path {
        value = value.get(key).cloned().unwrap_or(Value::Null);
    }
    Ok(value)
}

pub fn replace(
    source: &str,
    toml: bool,
    path: &[String],
    expected: &Value,
    replacement: &Value,
) -> Result<String> {
    ensure!(!path.is_empty(), "Empty configuration path");
    ensure!(
        &entry(source, toml, path)? == expected,
        "MCP settings changed; rerun setup/status before editing"
    );
    if toml {
        return replace_toml(source, path, expected, replacement);
    }
    if expected.is_null() {
        return insert(source, path, replacement);
    }
    let (ts, _) = tokens(source)?;
    let (a, b) = locate(&ts, 0, path)?;
    if replacement.is_null() {
        let mut start = a - 2;
        let mut end = b;
        if ts.get(end).is_some_and(|t| t.text == ",") {
            end += 1;
        } else if start > 0 && ts[start - 1].text == "," {
            start -= 1;
        }
        let range = ts[start].range.start..ts[end - 1].range.end;
        let (_, comments) = tokens(&source[range.clone()])?;
        let mut result = source.to_owned();
        result.replace_range(range, &format!("{}\n", comments.join("\n")));
        parse(&result, false)?;
        return Ok(result);
    }
    let range = ts[a].range.start..ts[b - 1].range.end;
    let (_, comments) = tokens(&source[range.clone()])?;
    let mut text = String::new();
    for comment in comments {
        text.push_str(&comment);
        text.push('\n');
    }
    text.push_str(&serde_json::to_string_pretty(replacement)?);
    let mut result = source.to_owned();
    result.replace_range(range, &text);
    parse(&result, false)?;
    Ok(result)
}

fn replace_toml(source: &str, path: &[String], before: &Value, after: &Value) -> Result<String> {
    let mut doc = source.parse::<toml_edit::DocumentMut>()?;
    let mut table = doc.as_item_mut();
    for key in path {
        table = table.get_mut(key).context("TOML MCP table missing")?;
    }
    let table = table
        .as_table_like_mut()
        .context("MCP entry must be a table")?;
    let old = before.as_object().context("MCP entry must be an object")?;
    let new = after.as_object().context("MCP entry must be an object")?;
    for key in old.keys().filter(|k| !new.contains_key(*k)) {
        table.remove(key);
    }
    for (key, value) in new {
        if old.get(key) == Some(value) {
            continue;
        }
        let encoded: toml::Value = serde_json::from_value(value.clone())?;
        let fragment = toml::to_string(&std::collections::BTreeMap::from([(key, encoded)]))?;
        let mut parsed = fragment.parse::<toml_edit::DocumentMut>()?;
        table.insert(key, parsed.remove(key).context("Cannot encode TOML field")?);
    }
    let result = doc.to_string();
    parse(&result, true)?;
    Ok(result)
}

fn insert(source: &str, path: &[String], replacement: &Value) -> Result<String> {
    let (ts, _) = tokens(source)?;
    let mut existing = path.len() - 1;
    while existing > 0 && entry(source, false, &path[..existing])?.is_null() {
        existing -= 1;
    }
    let (start, end) = if existing == 0 {
        (0, ts.len())
    } else {
        locate(&ts, 0, &path[..existing])?
    };
    ensure!(
        ts[start].text == "{" && ts[end - 1].text == "}",
        "Override parent must be an object"
    );
    let mut value = replacement.clone();
    for key in path[existing + 1..].iter().rev() {
        value = serde_json::json!({key: value});
    }
    let comma = if end - start > 2 && ts[end - 2].text != "," {
        ","
    } else {
        ""
    };
    let field = format!(
        "{}\n{}: {}\n",
        comma,
        serde_json::to_string(&path[existing])?,
        serde_json::to_string_pretty(&value)?
    );
    let mut result = source.to_owned();
    result.insert_str(ts[end - 1].range.start, &field);
    parse(&result, false)?;
    Ok(result)
}

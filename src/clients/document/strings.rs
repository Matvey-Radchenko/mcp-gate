//! Patch one string prefix by source span, retaining the original suffix bytes.
use super::{entry, locate, tokens};
use anyhow::{Context, Result, ensure};
use std::ops::Range;

pub(crate) fn replace_string_prefix(
    source: &str,
    toml: bool,
    path: &[String],
    before: &str,
    after: &str,
) -> Result<String> {
    ensure!(!path.is_empty(), "Empty configuration path");
    let value = entry(source, toml, path)?;
    let suffix = value
        .as_str()
        .and_then(|value| value.strip_prefix(before))
        .context("Helper changed; rerun setup before editing")?;
    let range = string_span(source, toml, path)?;
    let encoded = &source[range.clone()];
    let replacement = patch_scalar(encoded, toml, before, after, suffix)?;
    let mut result = source.to_owned();
    result.replace_range(range, &replacement);
    ensure!(
        entry(&result, toml, path)?.as_str() == Some(format!("{after}{suffix}").as_str()),
        "Cannot preserve the helper arguments"
    );
    Ok(result)
}

fn string_span(source: &str, toml: bool, path: &[String]) -> Result<Range<usize>> {
    if toml {
        let doc = toml_edit::Document::parse(source)
            .map_err(|_| anyhow::anyhow!("Invalid TOML configuration; values hidden"))?;
        let mut item = doc.as_item();
        for key in path {
            item = item.get(key).context("TOML configuration path missing")?;
        }
        return item.span().context("Missing TOML string span");
    }
    let (tokens, _) = tokens(source)?;
    let (start, end) = locate(&tokens, 0, path)?;
    ensure!(end == start + 1, "Expected a JSON string");
    Ok(tokens[start].range.clone())
}

fn patch_scalar(
    encoded: &str,
    toml: bool,
    before: &str,
    after: &str,
    suffix: &str,
) -> Result<String> {
    let delimiter = ["\"\"\"", "'''", "\"", "'"]
        .into_iter()
        .find(|quote| encoded.starts_with(quote) && (toml || quote.len() == 1))
        .context("Expected a quoted helper string")?;
    let content = &encoded[delimiter.len()..encoded.len() - delimiter.len()];
    // A multiline TOML delimiter consumes its first line break.
    let line_break = if delimiter.len() == 3 && content.starts_with("\r\n") {
        "\r\n"
    } else if delimiter.len() == 3 && content.starts_with('\n') {
        "\n"
    } else {
        ""
    };
    let body = &content[line_break.len()..];
    let canonical = encode(before, delimiter, toml)?;
    let end = if body.starts_with(&canonical) {
        canonical.len()
    } else {
        // Alternate escaping (such as JSON Unicode escapes) can change the
        // encoded length. Find the boundary using the format's own decoder.
        body.char_indices()
            .filter(|(at, ch)| *at >= before.len() && (ch.is_whitespace() || *ch == '\\'))
            .map(|(at, _)| at)
            .find(|at| {
                decodes_to(
                    &format!("{delimiter}{line_break}{}{delimiter}", &body[..*at]),
                    toml,
                    before,
                )
            })
            .context("Cannot locate the encoded helper executable")?
    };
    let arguments = &body[end..];
    if toml && delimiter.starts_with('\'') && after.contains('\'') {
        // A single-line literal can widen to a multiline literal without
        // re-encoding its arguments. A triple apostrophe needs a basic string.
        if !after.contains("'''") && !arguments.contains("'''") {
            return Ok(format!("'''{line_break}{after}{arguments}'''"));
        }
        return Ok(serde_json::to_string(&format!("{after}{suffix}"))?);
    }
    Ok(format!(
        "{delimiter}{line_break}{}{arguments}{delimiter}",
        encode(after, delimiter, toml)?
    ))
}

fn encode(text: &str, delimiter: &str, toml: bool) -> Result<String> {
    if toml && delimiter.starts_with('\'') {
        return Ok(text.to_owned());
    }
    let basic = serde_json::to_string(text)?;
    let body = &basic[1..basic.len() - 1];
    if delimiter == "'" {
        Ok(body.replace('\'', "\\'"))
    } else {
        Ok(body.to_owned())
    }
}

fn decodes_to(encoded: &str, toml: bool, expected: &str) -> bool {
    if toml {
        encoded
            .parse::<toml_edit::Value>()
            .is_ok_and(|value| value.as_str() == Some(expected))
    } else {
        json5::from_str::<String>(encoded).is_ok_and(|value| value == expected)
    }
}

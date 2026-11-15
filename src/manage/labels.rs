//! Readable service labels for new gateways. Existing labels are never rewritten.
use super::store::Registry;
use crate::clients::Candidate;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// `local.mcp-gate.<name>` with a numeric suffix when that label is already taken.
pub fn allocate(name: &str, taken: &BTreeSet<String>) -> String {
    // Published labels stay `local.mcp-gate.<name>`. Test builds add a short hash of
    // the private state directory so parallel service tests do not share one login label.
    let base = format!("{}{}", sanitize(name), test_scope());
    for n in 1..10_000 {
        let suffix = if n == 1 {
            String::new()
        } else {
            format!("-{n}")
        };
        let stem = stem_for(&base, suffix.len());
        let service = format!("{stem}{suffix}");
        if !super::agents::valid_name(&service) {
            continue;
        }
        let label = format!("local.mcp-gate.{service}");
        if !taken.contains(&label) {
            return label;
        }
    }
    // The numeric search above covers every practical collision.
    format!(
        "local.mcp-gate.{}",
        sanitize(&format!("server-{}", taken.len()))
    )
}

pub fn sanitize(name: &str) -> String {
    let mut out = String::new();
    let mut separated = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            separated = false;
        } else if matches!(c, '_' | '.') {
            out.push(c);
            separated = false;
        } else if !separated && !out.is_empty() {
            out.push('-');
            separated = true;
        }
    }
    while out.ends_with(['-', '_', '.']) {
        out.pop();
    }
    if out.is_empty() {
        return "server".into();
    }
    if !out.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        out.insert(0, 's');
    }
    stem_for(&out, 0)
}

fn test_scope() -> String {
    // Only the service tests set this. Dry-run and the setup demo keep the
    // published `local.mcp-gate.<name>` label.
    #[cfg(feature = "test-backend")]
    if let Ok(root) = std::env::var("MCP_GATE_TEST_LABEL_SCOPE") {
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(root.as_bytes()));
        return format!("-{}", &hash[..6]);
    }
    String::new()
}

fn stem_for(base: &str, suffix_len: usize) -> String {
    let room = 64usize.saturating_sub(suffix_len).max(1);
    let mut stem: String = base.chars().take(room).collect();
    while stem.ends_with(['-', '_', '.']) {
        stem.pop();
    }
    if stem.is_empty() || !stem.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        "server".into()
    } else {
        stem
    }
}

pub fn taken(registry: &Registry, home: &Path) -> BTreeSet<String> {
    let mut labels: BTreeSet<_> = registry
        .gateways
        .iter()
        .map(|record| record.label.clone())
        .collect();
    labels.extend(super::agents::recorded_labels(home));
    labels
}

/// Assign the label setup would register. Identical backends in this batch share one label.
/// Connections that still need a directory get a preview label only; nothing is registered.
pub fn assign(candidates: &mut [Candidate], registry: &Registry) -> anyhow::Result<()> {
    let home = super::store::home()?;
    let mut reserved = taken(registry, &home);
    let mut planned: Vec<Candidate> = Vec::new();
    for candidate in candidates.iter_mut() {
        if super::context::needs_directory(candidate) {
            // The directory is part of the identity, so the label is chosen after it is.
            candidate.service_label = None;
            continue;
        }
        if let Some(record) = super::runtime::reuse(registry, candidate)? {
            candidate.service_label = Some(record.label);
            continue;
        }
        if let Some(label) = planned
            .iter()
            .find(|earlier| same_definition(earlier, candidate))
            .and_then(|earlier| earlier.service_label.clone())
        {
            candidate.service_label = Some(label);
            continue;
        }
        candidate.service_label = Some(reserve(&candidate.binding.name, &mut reserved));
        planned.push(candidate.clone());
    }
    Ok(())
}

fn reserve(name: &str, taken: &mut BTreeSet<String>) -> String {
    let label = allocate(name, taken);
    taken.insert(label.clone());
    label
}

pub fn same_definition(left: &Candidate, right: &Candidate) -> bool {
    left.command == right.command
        && left.cwd == right.cwd
        && left.env == right.env
        && left.env_files == right.env_files
        && left.inherit_env == right.inherit_env
        && mode(left) == mode(right)
        && disabled(left) == disabled(right)
        && directories(left) == directories(right)
}

fn mode(candidate: &Candidate) -> &str {
    candidate
        .recipe
        .as_ref()
        .map_or("session", |recipe| recipe.mode.as_str())
}

fn disabled(candidate: &Candidate) -> &[String] {
    candidate
        .recipe
        .as_ref()
        .map_or(&[], |recipe| recipe.disabled_tools.as_slice())
}

fn directories(candidate: &Candidate) -> (&[String], Option<&str>) {
    candidate.recipe.as_ref().map_or((&[], None), |recipe| {
        (
            recipe.directory_env.as_slice(),
            recipe.working_directory_env.as_deref(),
        )
    })
}

pub fn note_shared_gateways(servers: &mut [serde_json::Value], registry: &Registry) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for server in servers.iter() {
        if let Some(label) = server["service_label"].as_str() {
            *counts.entry(label.to_string()).or_insert(0) += 1;
        }
    }
    for server in servers.iter_mut() {
        let Some(label) = server["service_label"].as_str() else {
            continue;
        };
        let existing = registry
            .gateways
            .iter()
            .any(|record| record.label == label && !record.removing);
        if existing || counts.get(label).copied().unwrap_or(0) > 1 {
            server["shares_gateway"] = serde_json::json!(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_readable_and_collisions_grow_a_numeric_suffix() {
        assert_eq!(sanitize("File System"), "file-system");
        assert_eq!(sanitize("@@@"), "server");
        assert!(super::super::agents::valid_name(
            "550e8400-e29b-41d4-a716-446655440000"
        ));
        let mut taken = BTreeSet::new();
        taken.insert("local.mcp-gate.550e8400-e29b-41d4-a716-446655440000".into());
        taken.insert("local.mcp-gate.mail".into());
        assert_eq!(allocate("Mail", &taken), "local.mcp-gate.mail-2");
        assert!(taken.contains("local.mcp-gate.550e8400-e29b-41d4-a716-446655440000"));
    }
}

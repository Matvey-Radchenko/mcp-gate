use crate::clients::{Binding, Candidate};
use serde_json::{Value, json};

pub fn display(plan: &Value) {
    eprintln!("Setup preview:");
    for server in plan["servers"].as_array().into_iter().flatten() {
        eprintln!(
            "  {} / {}: direct stdio -> gateway, mode {}",
            server["client"].as_str().unwrap_or("?"),
            server["name"].as_str().unwrap_or("?"),
            server["mode"].as_str().unwrap_or("session")
        );
        eprintln!("    Source: {}", server["source"].as_str().unwrap_or("?"));
        eprintln!(
            "    Local destination: {}",
            server["target"].as_str().unwrap_or("?")
        );
        if let Some(reason) = server["issue"].as_str() {
            eprintln!("    Skipped: {reason}");
        }
        if let Some(note) = server["platform_note"].as_str() {
            eprintln!("    {note}");
        }
        if let Some(conditions) = server["recipe"]["conditions"].as_str() {
            eprintln!("    Recipe conditions: {conditions}");
        }
        if let Some(diff) = server.get("diff") {
            eprintln!("    Redacted diff: {diff}");
        }
    }
    for server in plan["skipped"].as_array().into_iter().flatten() {
        eprintln!(
            "  Skipped {} / {}: {}",
            server["client"], server["name"], server["issue"]
        );
    }
    for gateway in plan["managed"].as_array().into_iter().flatten() {
        eprintln!(
            "  Existing gateway {}: {}",
            gateway["id"].as_str().unwrap_or("?"),
            gateway["action"].as_str().unwrap_or("?")
        );
        for connection in gateway["connections"].as_array().into_iter().flatten() {
            eprintln!(
                "    {} / {} ({})",
                connection["client"], connection["name"], connection["source"]
            );
        }
    }
    eprintln!(
        "Original command, argument order and client permissions are retained. Authentication stays in private files."
    );
    eprintln!(
        "Setup registers user autostart and starts the Rust gateways now. MCP backends stay lazy after discovery."
    );
    eprintln!(
        "Discovery runs the original command; npx, uvx or Docker may download dependencies. Readiness does not prove the application has switched."
    );
}
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, v)| (k.clone(), redact(v))).collect())
        }
        Value::Array(values) => Value::Array(values.iter().map(redact).collect()),
        Value::Null | Value::Bool(_) => value.clone(),
        _ => json!("<hidden>"),
    }
}
pub fn diff(candidate: &Candidate) -> Value {
    let mut after = candidate.binding.direct.clone();
    if let Some(map) = after.as_object_mut() {
        for field in ["command", "args", "env", "environment", "env_vars", "cwd"] {
            map.remove(field);
        }
        map.insert("url".into(), json!("<gateway>"));
        map.insert(
            "authentication".into(),
            json!("<private helper or file reference>"),
        );
    }
    json!({"before":redact(&candidate.binding.before),"after":redact(&after)})
}
pub fn target(binding: &Binding) -> Value {
    json!({"client":binding.client,"name":binding.name,"source":binding.source,"target":binding.target})
}

pub fn managed(record: &super::store::Record, hash: &str) -> Value {
    let config = crate::config::Config::load(&record.config());
    let catalog_changed = config.as_ref().map_or(true, |c| {
        crate::catalog::Catalog::load(&c.catalog_file, &c.backend).is_err()
    });
    json!({"id":record.id,"connections":record.bindings.iter().map(target).collect::<Vec<_>>(),
        "gateway_binary_changed":record.binary_hash != hash,
        "backend_or_catalog_changed":catalog_changed,
        "action":if record.removing { "finish pending remove" } else if catalog_changed {
            "idle gateway: rediscover original command and review catalog changes; busy gateway: defer"
        } else { "keep current healthy service, or replace it when idle" },
        "endpoint_and_credentials":"preserved"})
}

pub fn catalog_change(
    path: &std::path::Path,
    catalog: &crate::catalog::Catalog,
    options: &super::Selection,
) -> anyhow::Result<()> {
    let old = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<crate::catalog::Catalog>(&b).ok());
    let new_names: std::collections::BTreeSet<_> =
        catalog.tools.iter().map(|t| t.name.as_ref()).collect();
    let old_names: std::collections::BTreeSet<_> = old
        .as_ref()
        .map(|c| c.tools.iter().map(|t| t.name.as_ref()).collect())
        .unwrap_or_default();
    eprintln!(
        "Catalog review: {} tools; {} added, {} removed. Backend serverInfo.version changed: {}. Client permissions remain unchanged.",
        catalog.tools.len(),
        new_names.difference(&old_names).count(),
        old_names.difference(&new_names).count(),
        old.as_ref()
            .is_none_or(|c| c.backend_version != catalog.backend_version)
    );
    if options.diff {
        eprintln!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"before":redact(&serde_json::to_value(old)?),"after":redact(&serde_json::to_value(catalog)?)})
            )?
        );
    }
    anyhow::ensure!(
        super::confirm(
            "Accept this discovered catalog and resume the gateway?",
            options.yes
        )?,
        "Catalog changes declined; restoring the previous installation"
    );
    Ok(())
}

use crate::clients::{Binding, Candidate, Client};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::LazyLock};

pub fn display(plan: &Value, dry_run: bool) {
    println!("\nSetup preview");
    let servers: Vec<_> = plan["servers"].as_array().into_iter().flatten().collect();
    for server in &servers {
        if let Some(hint) = server["directory_hint"].as_str() {
            println!(
                "  {} / {}: {}",
                text(&server["client"]),
                text(&server["name"]),
                super::ui::safe(hint)
            );
        } else {
            println!(
                "  {} / {}: direct → gateway ({})",
                text(&server["client"]),
                text(&server["name"]),
                text(&server["mode"])
            );
        }
        println!("    Source: {}", text(&server["source"]));
        if let Some(label) = server["service_label"].as_str() {
            let when = if server["directory_hint"].is_string() {
                ", once a directory is set"
            } else {
                ""
            };
            let shared = if server["shares_gateway"] == true {
                " (one gateway for this identical backend)"
            } else {
                ""
            };
            println!("    Service label{when}: {label}{shared}");
        }
        println!("    Local destination: {}", text(&server["target"]));
        if server["directory_hint"].is_string() {
            println!("    Backend directory: not set");
        } else {
            println!(
                "    Backend directory: {}",
                text(&server["working_directory"])
            );
        }
        if let Some(note) = server["context_note"].as_str() {
            println!("    Context change: {note}");
        }
        if let Some(conditions) = server["recipe"]["conditions"].as_str() {
            println!("    Recipe conditions: {}", super::ui::safe(conditions));
        }
        if let Some(diff) = server.get("diff") {
            println!(
                "    Diff: {}",
                serde_json::to_string(diff).unwrap_or_else(|_| diff.to_string())
            );
        }
    }
    for gateway in plan["managed"].as_array().into_iter().flatten() {
        if gateway["connections"].as_array().is_some_and(Vec::is_empty) {
            println!(
                "  {}: {}",
                text(&gateway["label"]),
                text(&gateway["action"])
            );
            println!("    No saved direct connections.");
        }
        for b in gateway["connections"].as_array().into_iter().flatten() {
            println!(
                "  {} / {}: {}",
                text(&b["client"]),
                text(&b["name"]),
                text(&gateway["action"])
            );
            if let Some(label) = gateway["label"].as_str() {
                println!("    Service label: {label}");
            }
        }
    }
    for helper in plan["helper_migrations"].as_array().into_iter().flatten() {
        println!(
            "  {} / {}: migrate {} to current",
            text(&helper["client"]),
            text(&helper["name"]),
            helper["kind"].as_str().unwrap_or("headers helper")
        );
        println!("    Client file: {}", text(&helper["target"]));
    }
    if plan["refresh_helper_binary"] == true {
        println!("  Update the current client gateway executable.");
    }
    println!(
        "\nCommands, argument order and client permissions are preserved. Credentials stay private."
    );
    let directory_only = !servers.is_empty()
        && servers
            .iter()
            .all(|server| server["directory_hint"].is_string());
    if !servers.is_empty() {
        print_effects(dry_run, directory_only);
    }
}

fn print_effects(dry_run: bool, directory_only: bool) {
    if dry_run && directory_only {
        println!("No gateway would be registered until a working directory is chosen.");
    } else if dry_run {
        println!(
            "User autostart would be registered and gateways would start after setup; backends start on demand after discovery."
        );
    } else {
        println!(
            "User autostart is registered and gateways start now; backends start on demand after discovery."
        );
    }
    if dry_run {
        println!(
            "Discovery would run the original command; npx, uvx or Docker may download dependencies."
        );
    } else {
        println!(
            "Discovery runs the original command; npx, uvx or Docker may download dependencies."
        );
    }
    if cfg!(target_os = "macos") {
        println!(
            "macOS may request separate permission for a background gateway to access protected folders."
        );
    }
    println!(
        "Restart each affected application once after setup. Its new connection remains unconfirmed until used."
    );
}
fn text(value: &Value) -> String {
    super::ui::safe(value.as_str().unwrap_or("?"))
}
pub fn redact(value: &Value) -> Value {
    redact_keyed(value, None)
}

fn redact_keyed(value: &Value, key: Option<&str>) -> Value {
    if key.is_some_and(sensitive_key) {
        return json!("<hidden>");
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(name, child)| (name.clone(), redact_keyed(child, Some(name))))
                .collect(),
        ),
        Value::Array(values) if key.is_some_and(is_word_list) => Value::Array(redact_words(values)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|child| redact_keyed(child, None))
                .collect(),
        ),
        Value::String(text)
            if key.is_none_or(|name| !verbatim(name)) && looks_like_secret(text) =>
        {
            json!("<hidden>")
        }
        other => other.clone(),
    }
}

fn is_word_list(key: &str) -> bool {
    matches!(key, "args" | "command")
}

fn verbatim(key: &str) -> bool {
    matches!(key, "cwd" | "working_directory" | "url" | "command")
}

fn sensitive_key(key: &str) -> bool {
    static AUTH_KEYS: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
        Client::all()
            .iter()
            .filter_map(|client| match &client.descriptor().connection {
                crate::clients::descriptors::Connection::Http { auth, .. } => {
                    Some(normalized_key(auth.key()))
                }
                crate::clients::descriptors::Connection::Connect => None,
            })
            .collect()
    });
    let key = normalized_key(key);
    const EXACT: &[&str] = &[
        "env",
        "environment",
        "env_vars",
        "headers",
        "http_headers",
        "header",
        "authorization",
        "token",
        "password",
        "secret",
        "api_key",
        "apikey",
        "credential",
        "credentials",
        "cookie",
    ];
    EXACT.contains(&key.as_str())
        || AUTH_KEYS.contains(&key)
        || key.contains("token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("authorization")
        || key.contains("credential")
        || key.contains("cookie")
}

fn normalized_key(key: &str) -> String {
    key.to_ascii_lowercase().replace('-', "_")
}

fn redact_words(words: &[Value]) -> Vec<Value> {
    let mut hide_next = false;
    let mut out = Vec::new();
    for word in words {
        let Some(text) = word.as_str() else {
            out.push(json!("<hidden>"));
            hide_next = false;
            continue;
        };
        if hide_next {
            out.push(json!("<hidden>"));
            hide_next = false;
            continue;
        }
        if let Some((flag, _)) = text.split_once('=')
            && flag.starts_with('-')
            && sensitive_flag(flag)
        {
            out.push(json!(format!("{flag}=<hidden>")));
            continue;
        }
        if text.starts_with('-') && sensitive_flag(text) {
            hide_next = true;
            out.push(json!(text));
            continue;
        }
        if looks_like_secret(text) {
            out.push(json!("<hidden>"));
        } else {
            out.push(json!(text));
        }
    }
    out
}

fn sensitive_flag(flag: &str) -> bool {
    let flag = flag
        .trim_start_matches('-')
        .to_ascii_lowercase()
        .replace('-', "_");
    flag.contains("token")
        || flag.contains("secret")
        || flag.contains("password")
        || flag.contains("authorization")
        || flag.contains("credential")
        || flag.contains("api_key")
        || flag == "header"
        || flag.contains("cookie")
        || flag == "key"
        || flag.ends_with("_key")
}

fn looks_like_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    const PREFIXES: &[&str] = &[
        "sk-",
        "sk_",
        "ghp_",
        "github_pat_",
        "xox",
        "glpat-",
        "bearer ",
    ];
    if PREFIXES.iter().any(|prefix| lower.starts_with(prefix)) {
        return true;
    }
    if value.len() >= 8
        && (lower.contains("token") || lower.contains("secret") || lower.contains("password"))
    {
        return true;
    }
    value.len() >= 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn diff(candidate: &Candidate) -> Value {
    let after = super::remote::preview_entry(&candidate.binding, redact(&candidate.binding.direct));
    json!({"before":redact(&candidate.binding.before),"after":after})
}
pub fn target(binding: &Binding) -> Value {
    json!({"client":binding.client,"name":binding.name,"source":binding.source,"target":binding.target})
}

pub fn managed(record: &super::store::Record, root: &std::path::Path, hash: &str) -> Value {
    let target = super::upgrade::binary_path(root, hash);
    let observed = super::service::snapshot(record);
    let observed_binary = observed
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(|s| s.binary.as_path());
    use super::upgrade::BinaryState;
    let state = super::upgrade::binary_state(root, record, &target, hash, observed_binary);
    let binary_changed = state != BinaryState::Current;
    let migration = state == BinaryState::Migration;
    let config = crate::config::Config::load(&record.config());
    let catalog_changed = config.as_ref().map_or(true, |c| {
        crate::catalog::Catalog::load(&c.catalog_file, &c.backend).is_err()
    });
    let action = if record.removing {
        "finish pending remove"
    } else if migration {
        "migrate the gateway binary to the managed store when idle; preserve service settings"
    } else if record.bindings.is_empty() && observed_binary == Some(target.as_path()) {
        "already current; retain the service"
    } else if catalog_changed {
        "idle gateway: rediscover original command and review catalog changes; busy gateway: defer"
    } else if binary_changed {
        "update gateway when idle; retain endpoint and credentials"
    } else {
        "check current service; retain it if healthy"
    };
    json!({"id":record.id,"label":record.label,"connections":record.bindings.iter().map(self::target).collect::<Vec<_>>(),
        "gateway_binary_changed":binary_changed, "managed_store_migration":migration,
        "service_definition_checked":observed.is_ok(),
        "client_settings":if record.bindings.is_empty() { "no saved bindings" } else { "saved direct settings retained" },
        "backend_or_catalog_changed":catalog_changed, "action":action, "endpoint_and_credentials":"preserved"})
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

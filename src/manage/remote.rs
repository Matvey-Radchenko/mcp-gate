//! Render gateway client bindings and their setup previews.
use crate::{
    clients::{
        Binding,
        descriptors::{Authentication, Connection},
    },
    config::Config,
};
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use std::path::Path;

pub(super) fn binding(
    binding: &Binding,
    root: &Path,
    config_path: &Path,
    config: &Config,
) -> Result<Value> {
    let mut value = binding.direct.clone();
    let object = value
        .as_object_mut()
        .context("MCP configuration must be an object")?;
    clear_stdio(object);
    match &binding.client.descriptor().connection {
        Connection::Http {
            url_key,
            fields,
            auth,
        } => {
            object.insert(
                url_key.clone(),
                json!(format!("http://{}/mcp", config.listen)),
            );
            object.extend(fields.clone());
            let value = match auth {
                Authentication::Helper { .. } => {
                    json!(super::helper::command(
                        &super::current::path(root),
                        config_path
                    ))
                }
                Authentication::File { .. } => authorization_file(config)?,
            };
            object.insert(auth.key().into(), value);
        }
        Connection::Connect => {
            let command = std::path::absolute(super::current::path(root))?;
            let config_path = std::path::absolute(config_path)?;
            write_bridge(
                object,
                command
                    .to_str()
                    .context("Client executable path must be UTF-8")?,
                config_path
                    .to_str()
                    .context("Gateway configuration path must be UTF-8")?,
                &binding.name,
            );
        }
    }
    Ok(value)
}

pub(super) fn preview_entry(binding: &Binding, mut after: Value) -> Value {
    let Some(object) = after.as_object_mut() else {
        return after;
    };
    clear_stdio(object);
    match &binding.client.descriptor().connection {
        Connection::Http { url_key, .. } => {
            object.insert(
                url_key.clone(),
                json!("http://127.0.0.1:<assigned-at-setup>/mcp"),
            );
            object.insert(
                "authentication".into(),
                json!("<private helper or file reference>"),
            );
        }
        Connection::Connect => write_bridge(
            object,
            &format!("<state-root>/current/{}", crate::platform::binary_name()),
            "<gateway config>",
            &binding.name,
        ),
    }
    after
}

fn write_bridge(object: &mut Map<String, Value>, command: &str, config: &str, name: &str) {
    let args = if name.starts_with('-') {
        // The argument separator ends option parsing, so --config goes first.
        vec!["connect", "--config", config, "--", name]
    } else {
        vec!["connect", name, "--config", config]
    };
    object.insert("command".into(), json!(command));
    object.insert("args".into(), json!(args));
}

fn clear_stdio(object: &mut Map<String, Value>) {
    for key in ["command", "args", "env", "environment", "env_vars", "cwd"] {
        object.remove(key);
    }
}

fn authorization_file(config: &Config) -> Result<Value> {
    let auth = config.token_file.with_file_name("authorization");
    super::store::atomic(&auth, format!("Bearer {}", config.token()?).as_bytes())?;
    Ok(json!({"Authorization":format!("{{file:{}}}",auth.display())}))
}

#[cfg(test)]
mod tests;

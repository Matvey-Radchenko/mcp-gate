//! Render the client-specific authenticated HTTP binding.
use crate::{
    clients::{Binding, Client},
    config::Config,
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn binding(
    binding: &Binding,
    root: &Path,
    path: &Path,
    config: &Config,
) -> Result<Value> {
    let mut value = binding.direct.clone();
    let object = value
        .as_object_mut()
        .context("MCP configuration must be an object")?;
    for key in ["command", "args", "env", "environment", "env_vars", "cwd"] {
        object.remove(key);
    }
    object.insert("url".into(), json!(format!("http://{}/mcp", config.listen)));
    let helper = super::helper::command(&super::current::path(root), path);
    match binding.client {
        Client::Codex => {
            object.insert("http_headers_helper".into(), json!(helper));
        }
        Client::ClaudeCode => {
            object.insert("type".into(), json!("http"));
            object.insert("headersHelper".into(), json!(helper));
        }
        Client::Opencode => {
            object.insert("type".into(), json!("remote"));
            object.insert("oauth".into(), json!(false));
            let auth = config.token_file.with_file_name("authorization");
            super::store::atomic(&auth, format!("Bearer {}", config.token()?).as_bytes())?;
            object.insert(
                "headers".into(),
                json!({"Authorization":format!("{{file:{}}}",auth.display())}),
            );
        }
    }
    Ok(value)
}

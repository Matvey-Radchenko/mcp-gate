//! Legacy discovery is read-only: inspect bounded service definitions, never execute helpers.
use crate::config::Config;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Legacy {
    pub config: PathBuf,
    pub endpoint: String,
    pub health: Option<Value>,
    pub diagnostic: Option<&'static str>,
}

pub async fn discover(home: &Path, probe: bool) -> Vec<Legacy> {
    use futures::{StreamExt, stream};
    let jobs = configurations(home)
        .into_iter()
        .filter_map(|path| Config::load(&path).ok().map(|config| (path, config)));
    stream::iter(jobs)
        .map(|(path, config)| async move {
            let result = if probe {
                Some(health(&config).await)
            } else {
                None
            };
            let diagnostic = result.as_ref().and_then(|r| r.as_ref().err().copied());
            let health = result.and_then(Result::ok);
            Legacy {
                config: path,
                endpoint: format!("http://{}/mcp", config.listen),
                health,
                diagnostic,
            }
        })
        .buffered(4)
        .collect()
        .await
}

async fn health(config: &Config) -> Result<Value, &'static str> {
    let token = config
        .token()
        .map_err(|_| "Private authentication file is unavailable; health not checked")?;
    let mut response = super::runtime::http()
        .map_err(|_| "Cannot initialize the health check")?
        .get(format!("http://{}/health", config.listen))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Health request failed; check whether the existing service is running")?
        .error_for_status()
        .map_err(
            |_| "Gateway rejected the health request; check its private authentication file",
        )?;
    // Read only a small health response and publish selected, typed fields.
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read the health response")?
    {
        if bytes.len() + chunk.len() > 65536 {
            return Err("Health response exceeds the supported size");
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Unrecognized health response; gateway state not verified")?;
    if !matches!(
        value["service"].as_str(),
        Some("mcp-gate" | "mcp-session-gateway")
    ) {
        return Err("Unrecognized service identity; gateway state not verified");
    }
    Ok(serde_json::json!({"workers":value["workers"].as_u64(),
        "runtime":super::health::sanitized_runtime(&value["runtime"]),
        "sessions":value["sessions"].as_u64(),"maintenance":value["maintenance"].as_bool()}))
}

pub fn same_endpoint(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (reqwest::Url::parse(a), reqwest::Url::parse(b)) else {
        return false;
    };
    let loopback = |url: &reqwest::Url| {
        matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        )
    };
    loopback(&a)
        && loopback(&b)
        && a.scheme() == "http"
        && b.scheme() == "http"
        && a.port_or_known_default() == b.port_or_known_default()
        && a.path() == b.path()
        && a.query().is_none()
        && b.query().is_none()
        && a.username().is_empty()
        && a.password().is_none()
}

#[cfg(target_os = "macos")]
fn configurations(home: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(home.join("Library/LaunchAgents")) else {
        return vec![];
    };
    let mut paths = std::collections::BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "plist") {
            continue;
        }
        let Ok(output) = std::process::Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(&path)
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
            continue;
        };
        if let Some(config) = config_argument(&value) {
            paths.insert(config);
        }
    }
    paths.into_iter().collect()
}

/// Only literal argv for the known executable is evidence. Shell scripts and helpers are opaque.
#[cfg(any(target_os = "macos", test))]
pub fn config_argument(plist: &Value) -> Option<PathBuf> {
    let args = plist["ProgramArguments"].as_array()?;
    let binary = Path::new(args.first()?.as_str()?).file_name()?.to_str()?;
    if !matches!(binary, "mcp-session-gateway" | "mcp-gate") {
        return None;
    }
    let index = args.iter().position(|v| v == "--config")?;
    let path = PathBuf::from(args.get(index + 1)?.as_str()?);
    path.is_absolute().then_some(path)
}

#[cfg(not(target_os = "macos"))]
fn configurations(_home: &Path) -> Vec<PathBuf> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn legacy_evidence_requires_a_literal_executable_and_exact_loopback_endpoint() {
        let config = if cfg!(windows) {
            "C:/fixture/config.toml"
        } else {
            "/fixture/config.toml"
        };
        assert_eq!(
            config_argument(
                &json!({"ProgramArguments":["/bin/mcp-session-gateway","serve","--config",config]})
            ),
            Some(PathBuf::from(config))
        );
        assert!(config_argument(&json!({"ProgramArguments":["sh","-c","mcp-session-gateway --config /fixture/config.toml"]})).is_none());
        assert!(same_endpoint(
            "http://localhost:1234/mcp",
            "http://127.0.0.1:1234/mcp"
        ));
        for endpoint in [
            "http://example.invalid:1234/mcp",
            "http://localhost:1235/mcp",
            "http://localhost:1234/other",
            "http://localhost:1234/mcp?token=hidden",
            "http://user:password@localhost:1234/mcp",
        ] {
            assert!(!same_endpoint(endpoint, "http://127.0.0.1:1234/mcp"));
        }
    }
}

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

pub(super) async fn health(config: &Config) -> Result<Value, &'static str> {
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
    super::agents::plists(home)
        .iter()
        .filter_map(|path| super::service::launchd::invocation(path).ok())
        .filter(|(binary, _)| super::agents::gateway_binary(binary))
        .map(|(_, config)| config)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(target_os = "linux")]
fn configurations(_home: &Path) -> Vec<PathBuf> {
    super::trace::log("legacy: linux definitions start");
    let Ok(services) = super::service::linux::definitions() else {
        // Legacy discovery is optional when the user manager cannot be queried.
        super::trace::log("legacy: linux definitions unavailable");
        return vec![];
    };
    super::trace::log(&format!(
        "legacy: linux definitions done n={}",
        services.len()
    ));
    services
        .into_iter()
        .filter_map(|(_, identity, _)| {
            matches!(
                identity.binary.file_name()?.to_str()?,
                "mcp-session-gateway" | "mcp-gate"
            )
            .then_some(identity.config)
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(windows)]
fn configurations(_home: &Path) -> Vec<PathBuf> {
    super::service::scheduler::definitions()
        .map(|tasks| {
            tasks
                .into_iter()
                .filter(|(_, binary, _)| super::agents::gateway_binary(binary))
                .map(|(_, _, config)| config)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn configurations(_home: &Path) -> Vec<PathBuf> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_evidence_requires_a_literal_executable_and_exact_loopback_endpoint() {
        let config = if cfg!(windows) {
            "C:/fixture/config.toml"
        } else {
            "/fixture/config.toml"
        };
        assert_eq!(
            super::super::service::config_argument(["serve", "--config", config].into_iter()),
            Some(PathBuf::from(config))
        );
        assert!(
            super::super::service::config_argument(
                ["-c", "mcp-session-gateway --config /fixture/config.toml"].into_iter()
            )
            .is_none()
        );
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

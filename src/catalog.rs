use crate::config::Backend;
use anyhow::{Result, ensure};
use rmcp::model::{Prompt, Resource, ResourceTemplate, ServerInfo, Tool};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub format_version: u32,
    pub backend_version: String,
    pub entrypoint_sha256: String,
    pub args: Vec<String>,
    pub server_info: ServerInfo,
    pub tools: Vec<Tool>,
    #[serde(default)]
    pub resources: Vec<Resource>,
    #[serde(default)]
    pub resource_templates: Vec<ResourceTemplate>,
    #[serde(default)]
    pub prompts: Vec<Prompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_sha256: Option<String>,
}
pub fn digest(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}
impl Catalog {
    pub fn load(path: &Path, backend: &Backend) -> Result<Self> {
        let catalog: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            matches!(catalog.format_version, 1..=3),
            "Unsupported catalog format"
        );
        if catalog.format_version >= 2 {
            ensure!(
                catalog.invocation_sha256.as_deref() == Some(&fingerprint(backend)?),
                "Backend changed (invocation); regenerate and review catalog"
            );
        } else {
            ensure!(
                backend.profile == crate::backend::Profile::ChromeDevtools,
                "Generic backend requires catalog format 2"
            );
        }
        ensure!(
            catalog.backend_version == backend.version
                && catalog.server_info.server_info.version == backend.version,
            "Backend version differs from catalog"
        );
        ensure!(
            catalog.args == backend.args
                && catalog.entrypoint_sha256 == digest(backend.artifact())?,
            "Backend changed; regenerate and review catalog"
        );
        ensure!(!catalog.tools.is_empty(), "Empty tool catalog");
        ensure!(
            catalog.format_version >= 3
                || (catalog.server_info.capabilities.resources.is_none()
                    && catalog.server_info.capabilities.prompts.is_none()),
            "Resources/prompts require catalog format 3"
        );
        validate_capabilities(&catalog.server_info)?;
        Ok(catalog)
    }
}

/// Values are hashed, not persisted. Secret-file contents are deliberately excluded:
/// credentials can rotate without turning the discovery artifact into a secret store.
pub fn fingerprint(backend: &Backend) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(backend)?);
    hash.update(digest(&backend.command)?);
    hash.update(digest(backend.artifact())?);
    Ok(format!("{:x}", hash.finalize()))
}

pub fn validate_capabilities(info: &ServerInfo) -> Result<()> {
    ensure!(
        [
            rmcp::model::ProtocolVersion::V_2025_11_25,
            rmcp::model::ProtocolVersion::V_2025_06_18,
            rmcp::model::ProtocolVersion::V_2025_03_26,
            rmcp::model::ProtocolVersion::V_2024_11_05,
        ]
        .contains(&info.protocol_version),
        "Backend negotiated an unsupported protocol revision"
    );
    let caps = &info.capabilities;
    if info.protocol_version == rmcp::model::ProtocolVersion::V_2024_11_05 {
        ensure!(
            caps.resources.is_none() && caps.prompts.is_none() && caps.extensions.is_none(),
            "Legacy stdio adapter currently supports tools only"
        );
    }
    ensure!(caps.tools.is_some(), "Backend must expose tools");
    ensure!(
        caps.resources
            .as_ref()
            .is_none_or(|r| r.subscribe != Some(true)),
        "Resource subscriptions are not supported"
    );
    ensure!(
        caps.completions.is_none()
            && caps.experimental.as_ref().is_none_or(|e| e.is_empty())
            && caps
                .extensions
                .as_ref()
                .is_none_or(|e| e
                    .iter()
                    .all(|(name, settings)| name == "io.modelcontextprotocol/ui"
                        && settings.is_empty())),
        "Backend declares unsupported completion/extension capabilities"
    );
    Ok(())
}

/// A complete discovery snapshot, never resource contents or rendered prompts.
pub struct Discovery {
    pub info: ServerInfo,
    pub tools: Vec<Tool>,
    pub resources: Vec<Resource>,
    pub resource_templates: Vec<ResourceTemplate>,
    pub prompts: Vec<Prompt>,
}

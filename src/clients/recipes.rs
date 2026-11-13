//! Deliberately narrow matches; a familiar server name never selects shared ownership.
use super::Candidate;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
#[derive(Clone, Deserialize, Serialize)]
pub struct Recipe {
    pub platforms: Vec<String>,
    pub id: String,
    pub mode: String,
    pub server_version: String,
    pub package: Option<String>,
    pub package_version: Option<String>,
    pub artifact_sha256: Option<String>,
    pub required_args: Vec<String>,
    pub required_env: BTreeMap<String, String>,
    pub credential_keys: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub conditions: String,
    #[serde(default)]
    pub directory_env: Vec<String>,
    #[serde(default)]
    pub working_directory_env: Option<String>,
    #[serde(default)]
    pub allowed_args: Option<Vec<String>>,
    #[serde(default)]
    pub required_options: BTreeMap<String, String>,
    #[serde(default)]
    pub allowed_env: Option<Vec<String>>,
}
pub fn all() -> Vec<Recipe> {
    serde_json::from_str(include_str!("../../recipes/verified.json"))
        .expect("Checked-in recipe schema")
}
pub fn entrypoint(command: &[String], cwd: &Path) -> Option<PathBuf> {
    let exe = Path::new(command.first()?).file_stem()?.to_str()?;
    let index = if matches!(exe, "node" | "nodejs" | "python" | "python3") {
        (command.len() > 1 && !command[1].starts_with('-')).then_some(1)?
    } else if exe == "java" {
        command.iter().position(|a| a == "-jar")? + 1
    } else {
        0
    };
    let path = cwd.join(command.get(index)?);
    path.is_file().then_some(path)
}
pub fn matching(candidate: &Candidate) -> Option<Recipe> {
    let entry = entrypoint(&candidate.command, &candidate.cwd);
    let hash = entry.as_ref().and_then(|p| crate::catalog::digest(p).ok());
    let package = entry.as_ref().and_then(|p| metadata(p));
    all().into_iter().find(|r| {
        let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        if !r.platforms.contains(&platform) {
            return false;
        }
        if r.artifact_sha256
            .as_ref()
            .is_some_and(|h| Some(h) != hash.as_ref())
        {
            return false;
        }
        if r.artifact_sha256.is_none()
            && !r
                .package
                .as_ref()
                .zip(r.package_version.as_ref())
                .is_some_and(|(name, version)| {
                    package
                        .as_ref()
                        .is_some_and(|p| p == &(name.clone(), version.clone()))
                })
        {
            return false;
        }
        r.required_args
            .iter()
            .all(|a| candidate.command.contains(a))
            && r.allowed_env
                .as_ref()
                .is_none_or(|allowed| candidate.env.keys().all(|key| allowed.contains(key)))
            && r.allowed_args.as_ref().is_none_or(|allowed| {
                candidate
                    .command
                    .iter()
                    .skip(2)
                    .all(|a| allowed.contains(a))
            })
            && r.required_options.iter().all(|(key, value)| {
                candidate
                    .command
                    .windows(2)
                    .any(|args| args[0] == *key && args[1] == *value)
            })
            && r.required_env
                .iter()
                .all(|(k, v)| candidate.env.get(k) == Some(v))
            && r.credential_keys
                .iter()
                .all(|k| candidate.env.get(k).is_some_and(|v| !v.is_empty()))
            && !candidate.command.iter().any(|a| {
                [
                    "--browser-url",
                    "--browserUrl",
                    "--ws-endpoint",
                    "--wsEndpoint",
                    "--autoConnect",
                    "--auto-connect",
                    "--user-data-dir",
                    "--userDataDir",
                    "--cdp-endpoint",
                ]
                .iter()
                .any(|key| a.split('=').next() == Some(key))
            })
    })
}
fn metadata(path: &Path) -> Option<(String, String)> {
    for parent in path.ancestors().skip(1).take(5) {
        let Ok(bytes) = std::fs::read(parent.join("package.json")) else {
            continue;
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        return Some((
            value["name"].as_str()?.into(),
            value["version"].as_str()?.into(),
        ));
    }
    None
}

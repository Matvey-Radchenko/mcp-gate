//! Stages an immutable-by-convention local release, independently of activation.
//! Existing releases, launchd jobs and client configuration are never modified.
use crate::{
    catalog::digest,
    config::Config,
    install::{copy_tree, generate_catalog, xml},
};
use anyhow::{Result, ensure};
use std::{
    fs::{self},
    io::Write,
    path::Path,
};

fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = crate::platform::private_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn validate_destination(prefix: &Path, label: &str) -> Result<()> {
    ensure!(
        prefix.is_absolute() && !prefix.exists(),
        "A fresh absolute release prefix is required"
    );
    ensure!(
        !label.is_empty()
            && label.len() <= 120
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)),
        "Invalid launchd label"
    );
    Ok(())
}

fn snapshot_backend(config: &mut Config, prefix: &Path, root: Option<&Path>) -> Result<()> {
    if let Some(root) = root {
        let root = root.canonicalize()?;
        let parent = prefix
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Missing release parent"))?
            .canonicalize()?;
        ensure!(
            !parent.starts_with(&root),
            "Release cannot be staged inside its artifact source"
        );
        let artifact = config.backend.artifact().canonicalize()?;
        let relative = artifact.strip_prefix(&root)?;
        fs::create_dir(prefix.join("runtime"))?;
        // Directory names can participate in resolution (notably node_modules).
        // Preserve the root basename instead of flattening its contents into runtime.
        let target = prefix.join("runtime").join(
            root.file_name()
                .ok_or_else(|| anyhow::anyhow!("Artifact root needs a basename"))?,
        );
        copy_tree(&root, &target)?;
        if config.backend.entrypoint.is_some() {
            config.backend.entrypoint = Some(target.join(relative));
        } else {
            config.backend.command = target.join(relative);
        }
    }
    fs::create_dir(prefix.join("credentials"))?;
    // Numbered destinations never interpret an environment variable name as a path.
    // Copy legacy credentials privately; leave their original files untouched.
    for (index, path) in config.backend.env_files.values_mut().enumerate() {
        let target = prefix.join(format!("credentials/{index}"));
        private_write(&target, &fs::read(&*path)?)?;
        *path = target;
    }
    Ok(())
}

fn launch_agent(prefix: &Path, label: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array><string>{binary}</string><string>serve</string><string>--config</string><string>{config}</string></array>
<key>WorkingDirectory</key><string>{state}</string>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>10</integer>
<key>ExitTimeOut</key><integer>20</integer>
<key>Umask</key><integer>63</integer>
<key>StandardOutPath</key><string>{stdout}</string>
<key>StandardErrorPath</key><string>{stderr}</string>
</dict></plist>
"#,
        binary = xml(&prefix
            .join("bin")
            .join(crate::platform::binary_name())
            .to_string_lossy()),
        config = xml(&prefix.join("config.toml").to_string_lossy()),
        state = xml(&prefix.join("state").to_string_lossy()),
        stdout = xml(&prefix.join("logs/stdout.log").to_string_lossy()),
        stderr = xml(&prefix.join("logs/stderr.log").to_string_lossy())
    )
}

/// Uses the input config's listener and token; relocates mutable state and secret
/// copies. Catalog discovery starts one temporary backend, but makes no tool calls.
/// Partial failures are retained for inspection, never recursively deleted.
pub async fn stage(
    input: &Path,
    prefix: &Path,
    label: &str,
    artifact_root: Option<&Path>,
) -> Result<()> {
    validate_destination(prefix, label)?;
    let mut config = Config::load(input)?;
    let token = config.token()?;
    fs::create_dir(prefix)?;
    crate::platform::private_permissions(prefix)?;
    for name in ["bin", "state", "logs"] {
        fs::create_dir(prefix.join(name))?;
    }
    let binary = prefix.join("bin").join(crate::platform::binary_name());
    fs::copy(std::env::current_exe()?, &binary)?;
    crate::platform::executable(&binary)?;
    snapshot_backend(&mut config, prefix, artifact_root)?;
    config.token_file = prefix.join("client-token");
    config.catalog_file = prefix.join("catalog.json");
    config.state_dir = prefix.join("state");
    config.validate()?;
    private_write(&config.token_file, token.as_bytes())?;
    private_write(
        &prefix.join("config.toml"),
        toml::to_string_pretty(&config)?.as_bytes(),
    )?;
    let catalog = generate_catalog(&config).await?;
    private_write(&config.catalog_file, &serde_json::to_vec_pretty(&catalog)?)?;
    private_write(
        &prefix.join(format!("{label}.plist")),
        launch_agent(prefix, label).as_bytes(),
    )?;
    let manifest = serde_json::json!({
        "format_version":1, "gateway_version":env!("CARGO_PKG_VERSION"),
        "gateway_sha256":digest(&binary)?, "backend_command_sha256":digest(&config.backend.command)?,
        "catalog_sha256":digest(&config.catalog_file)?, "label":label,
        "tool_count":catalog.tools.len(), "activation":"not performed"
    });
    private_write(
        &prefix.join("release.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    println!(
        "Staged {} tools at {}. Review catalog before activation. No service or client settings changed.",
        catalog.tools.len(),
        prefix.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn private_files_never_overwrite_and_labels_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        private_write(&path, b"fixture").unwrap();
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(private_write(&path, b"replace").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"fixture");
        assert!(validate_destination(&path, "valid.label").is_err());
        assert!(validate_destination(&dir.path().join("fresh"), "../escape").is_err());
        assert!(validate_destination(&dir.path().join("fresh"), "local.gateway").is_ok());
    }
}

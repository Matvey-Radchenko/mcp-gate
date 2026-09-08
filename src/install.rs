//! Explicit, local-only installation. Never modifies a client's configuration,
//! overwrites an existing install, bootstraps launchd, or downloads executable code.
use crate::{
    catalog::{Catalog, digest, fingerprint, validate_capabilities},
    config::{Backend, Config},
    worker::Worker,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    fs::{self},
    io::Write,
    net::SocketAddr,
    path::Path,
    sync::{Arc, atomic::AtomicUsize},
    time::Duration,
};

pub fn init_token(path: &Path) -> Result<()> {
    let mut file = crate::platform::private_file(path)?;
    // UUIDs use OS randomness; concatenation provides 244 random bits.
    writeln!(
        file,
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )?;
    file.sync_all()?;
    Ok(())
}
pub async fn generate_catalog(config: &Config) -> Result<Catalog> {
    assemble(config, discover(config).await?)
}

pub async fn discover_and_pin(config: &mut Config) -> Result<Catalog> {
    let found = discover(config).await?;
    config.backend.version = found.info.server_info.version.clone();
    assemble(config, found)
}

async fn discover(config: &Config) -> Result<crate::catalog::Discovery> {
    let worker = Worker::start(config, None, None, Arc::new(AtomicUsize::new(0))).await?;
    let result = tokio::time::timeout(
        Duration::from_secs(config.startup_timeout_seconds),
        worker.catalog(),
    )
    .await;
    let stable = !worker.catalog_changed();
    worker.shutdown().await;
    ensure!(
        stable,
        "Backend tool catalog changed during discovery; review backend stability"
    );
    result.context("Catalog query timed out")?
}

fn assemble(config: &Config, found: crate::catalog::Discovery) -> Result<Catalog> {
    let crate::catalog::Discovery {
        info,
        tools,
        resources,
        resource_templates,
        prompts,
    } = found;
    ensure!(
        info.server_info.version == config.backend.version,
        "Unexpected backend version"
    );
    validate_capabilities(&info)?;
    config.tool_policy.validate_catalog(&tools)?;
    Ok(Catalog {
        format_version: 3,
        backend_version: config.backend.version.clone(),
        entrypoint_sha256: digest(config.backend.artifact())?,
        invocation_sha256: Some(fingerprint(&config.backend)?),
        args: config.backend.args.clone(),
        server_info: info,
        tools,
        resources,
        resource_templates,
        prompts,
    })
}
pub(crate) fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == ".bin" {
            continue;
        } // npm command shims are not needed.
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "Refusing runtime symlink: {}",
            entry.path().display()
        );
        let target = dest.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            ensure!(kind.is_file(), "Unsupported runtime file");
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
pub(crate) fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub async fn install(prefix: &Path, runtime: &Path, node: &Path, listen: SocketAddr) -> Result<()> {
    ensure!(
        prefix.is_absolute() && runtime.is_absolute() && node.is_absolute(),
        "Install paths must be absolute"
    );
    ensure!(
        !prefix.exists(),
        "Install prefix already exists; refusing to overwrite it"
    );
    ensure!(
        listen.ip().is_loopback(),
        "Only a loopback listener is supported"
    );
    ensure!(node.is_file(), "Node executable is missing");
    let package: serde_json::Value = serde_json::from_slice(&fs::read(
        runtime.join("node_modules/chrome-devtools-mcp/package.json"),
    )?)?;
    ensure!(
        package["version"] == "1.8.0",
        "Expected Chrome DevTools MCP 1.8.0; run npm ci in runtime/"
    );
    // No recursive deletions on failure: a partial install remains available for inspection.
    fs::create_dir(prefix)?;
    crate::platform::private_permissions(prefix)?;
    fs::create_dir(prefix.join("bin"))?;
    let binary = prefix.join("bin").join(crate::platform::binary_name());
    fs::copy(std::env::current_exe()?, &binary)?;
    crate::platform::executable(&binary)?;
    let installed_runtime = prefix.join("runtime");
    fs::create_dir(&installed_runtime)?;
    for file in ["package.json", "package-lock.json"] {
        fs::copy(runtime.join(file), installed_runtime.join(file))?;
    }
    copy_tree(
        &runtime.join("node_modules"),
        &installed_runtime.join("node_modules"),
    )?;
    for dir in ["state", "logs"] {
        fs::create_dir(prefix.join(dir))?;
    }
    let config = Config {
        tool_policy: Default::default(),
        format_version: 2,
        ownership: crate::config::Ownership::Session,
        shared_client_roots: crate::config::SharedClientRoots::Reject,
        max_pending_calls: 16,
        queue_timeout_seconds: 30,
        listen,
        token_file: prefix.join("client-token"),
        catalog_file: prefix.join("catalog.json"),
        state_dir: prefix.join("state"),
        max_workers: 4,
        max_sessions: 128,
        disconnect_grace_seconds: 60,
        startup_timeout_seconds: 30,
        call_timeout_seconds: 300,
        backend: Backend {
            docker: false,
            working_directory: None,
            command_args: Vec::new(),
            directory_env: Vec::new(),
            working_directory_env: None,
            profile: crate::backend::Profile::ChromeDevtools,
            inherit_env: Vec::new(),
            env_files: BTreeMap::new(),
            command: node.into(),
            entrypoint: Some(
                installed_runtime
                    .join("node_modules/chrome-devtools-mcp/build/src/bin/chrome-devtools-mcp.js"),
            ),
            version: "1.8.0".into(),
            args: vec!["--isolated".into(), "--no-usage-statistics".into()],
            env: BTreeMap::new(),
        },
    };
    config.validate()?;
    init_token(&config.token_file)?;
    let config_file = prefix.join("config.toml");
    fs::write(&config_file, toml::to_string_pretty(&config)?)?;
    let catalog = generate_catalog(&config).await?;
    fs::write(&config.catalog_file, serde_json::to_vec_pretty(&catalog)?)?;
    let label = "local.mcp-gate.chrome-devtools";
    let plist = format!(
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
        binary = xml(&binary.to_string_lossy()),
        config = xml(&config_file.to_string_lossy()),
        state = xml(&config.state_dir.to_string_lossy()),
        stdout = xml(&prefix.join("logs/stdout.log").to_string_lossy()),
        stderr = xml(&prefix.join("logs/stderr.log").to_string_lossy())
    );
    fs::write(prefix.join(format!("{label}.plist")), plist)?;
    let shell_quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
    let helper = format!(
        "{} headers --config {}",
        shell_quote(&binary),
        shell_quote(&config_file)
    );
    fs::write(
        prefix.join("codex.toml.example"),
        format!(
            "[mcp_servers.chrome-devtools]\nurl = \"http://{listen}/mcp\"\nhttp_headers_helper = {}\nstartup_timeout_sec = 20\n",
            serde_json::to_string(&helper)?
        ),
    )?;
    println!(
        "Installed locally: {}\nBackend 1.8.0; {} tools.\nLaunchAgent generated but NOT loaded. Client configurations were NOT changed.",
        prefix.display(),
        catalog.tools.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn token_is_private_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        init_token(&path).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(before.len(), 65);
        assert!(init_token(&path).is_err());
        assert_eq!(before, fs::read(&path).unwrap());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn plist_paths_are_escaped() {
        assert_eq!(xml("a&<b>\"'"), "a&amp;&lt;b&gt;&quot;&apos;");
    }
}

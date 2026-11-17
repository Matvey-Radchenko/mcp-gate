//! Adoption records the identity of an existing per-user gateway service.
use super::{agents, store};
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;

pub async fn run(name: String, dry_run: bool, json: bool) -> Result<()> {
    ensure!(
        agents::valid_name(&name),
        "Gateway name must be a short ASCII label"
    );
    let home = store::home()?;
    let root = store::root()?;
    let registry = store::load(&root)?;
    let missing =
        || format!("No local.mcp-gate.{name} user service with a readable definition was found");
    let service = agents::discover(&home, &registry)
        .await
        .with_context(missing)?
        .into_iter()
        .find(|service| service.name == name);
    let Some(service) = service else {
        bail!("{}", missing());
    };
    ensure!(
        service.state != "config unreadable",
        "Gateway {name} configuration cannot be read: {}",
        service.config.display()
    );
    if let Some(existing) = registry
        .gateways
        .iter()
        .find(|record| record.id == name || record.label == service.label)
    {
        ensure!(
            existing.config() == service.config,
            "Gateway name {name} is already used by a different installation"
        );
        #[cfg(target_os = "linux")]
        ensure!(
            existing.label == service.label && existing.binary == service.binary,
            "Gateway name {name} is already used by a different installation"
        );
        report(
            json,
            "unchanged",
            &format!("Gateway {name} is already owned by this installation."),
            &service,
        );
        return Ok(());
    }
    let release = service
        .config
        .parent()
        .context("Gateway configuration has no directory")?
        .to_path_buf();
    ensure!(
        service.binary.is_file(),
        "Gateway binary is missing: {}",
        service.binary.display()
    );
    let record = store::Record {
        removing: false,
        id: name.clone(),
        release,
        label: service.label.clone(),
        binary_hash: crate::catalog::digest(&service.binary)?,
        binary: service.binary.clone(),
        bindings: Vec::new(),
    };
    #[cfg(target_os = "linux")]
    ensure!(
        record.config() == service.config,
        "Adoption requires the gateway configuration to be named config.toml"
    );
    if dry_run {
        report(
            json,
            "preview",
            &format!("Dry run. Gateway {name} stays unmanaged and no files change."),
            &service,
        );
        return Ok(());
    }
    store::prepare(&root)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("setup.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("Another setup/remove operation is running")?;
    #[cfg(target_os = "linux")]
    ensure!(
        super::service::installed(&record)?,
        "The systemd user service changed during adoption"
    );
    let mut registry = store::load(&root)?;
    ensure!(
        !registry
            .gateways
            .iter()
            .any(|existing| existing.id == name || existing.label == service.label),
        "Gateway {name} was adopted while this command was starting"
    );
    registry.gateways.push(record);
    store::save(&root, &registry)?;
    report(
        json,
        "complete",
        &format!("Adopted gateway {name}. Client settings were not changed."),
        &service,
    );
    Ok(())
}

fn report(json: bool, phase: &str, message: &str, service: &agents::Service) {
    if json {
        println!(
            "{}",
            json!({"format_version":1,"phase":phase,"name":service.name,"label":service.label,
                "config":service.config,"binary":service.binary,"listen":service.listen,
                "ownership":if phase == "complete" { "managed" } else { service.ownership },
                "message":message})
        );
        return;
    }
    println!("Gateway {}", service.name);
    println!("  Label: {}", service.label);
    println!("  Config: {}", service.config.display());
    println!("  Binary: {}", service.binary.display());
    if let Some(listen) = &service.listen {
        println!("  Listen: {listen}");
    }
    println!("{message}");
}

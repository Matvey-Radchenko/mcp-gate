//! Adoption records an existing LaunchAgent. It does not rewrite the gateway or its clients.
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
    let service = agents::discover(&home, &registry)
        .await
        .into_iter()
        .find(|service| service.name == name);
    let Some(service) = service else {
        bail!("No LaunchAgent local.mcp-gate.{name} with a readable definition was found");
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

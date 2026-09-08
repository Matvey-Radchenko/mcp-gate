use super::{Status, runtime, service, store};
use crate::{catalog::Catalog, clients::document, config::Config};
use anyhow::Result;
use serde_json::json;

pub async fn run(options: Status) -> Result<()> {
    if let Some(path) = &options.config {
        anyhow::ensure!(
            !options.probe,
            "Active probes require a managed gateway; legacy --config is passive only"
        );
        let c = Config::load(path)?;
        c.token()?;
        Catalog::load(&c.catalog_file, &c.backend)?;
        let response = runtime::http()?
            .get(format!("http://{}/health", c.listen))
            .bearer_auth(c.token()?)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"format_version":1,"gateway":response}))?
        );
        return Ok(());
    }
    let root = store::root()?;
    let registry = store::load(&root)?;
    // Active discovery and setup/remove share the same operation lock. A passive
    // status never creates files or acquires an exclusive maintenance barrier.
    let _probe_lock = if options.probe && root.exists() {
        use fs2::FileExt;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("setup.lock"))?;
        file.try_lock_exclusive()
            .map_err(|_| anyhow::anyhow!("Another setup, remove or active probe is running"))?;
        Some(file)
    } else {
        None
    };
    let operations = super::recovery::diagnostics(&root).await?;
    let mut results = Vec::new();
    let selection = super::Selection {
        client: options.client.clone(),
        project: options.project.clone(),
        server: options.server.clone(),
        ..Default::default()
    };
    for record in &registry.gateways {
        if !record
            .bindings
            .iter()
            .any(|b| super::matches_binding(&selection, b))
        {
            continue;
        }
        let mut issues = Vec::new();
        if record.removing {
            issues.push("Direct connections restored; service removal pending".into());
        }
        let config = Config::load(&record.config());
        match config {
            Ok(c) => {
                if c.token().is_err() {
                    issues.push("Private authentication file is unavailable".to_string());
                }
                if Catalog::load(&c.catalog_file, &c.backend).is_err() {
                    issues.push("Backend invocation/catalog changed; rerun setup".into());
                }
                if options.probe {
                    issues.extend(probe(record, &c).await);
                }
            }
            Err(_) => issues.push("Gateway configuration is invalid or missing".into()),
        }
        for b in &record.bindings {
            let actual = std::fs::read_to_string(&b.target)
                .ok()
                .and_then(|s| document::entry(&s, b.toml, &b.path).ok());
            if actual.as_ref() != Some(&b.after) {
                issues.push(format!(
                    "{} / {} no longer points at this gateway",
                    b.client, b.name
                ));
            }
        }
        let health = runtime::health(record).await.ok();
        if health
            .as_ref()
            .is_some_and(|value| value["maintenance"] == true)
        {
            issues.push("Gateway is paused for maintenance; after an interrupted operation, rerun setup to resume safely".into());
        }
        if health.is_none() {
            issues.push("Gateway is not reachable".into());
        }
        if !service::installed(record).unwrap_or(false) {
            issues.push("Autostart service is missing".into());
        }
        results.push(json!({"id":record.id,"clients":record.bindings.iter().map(|b|json!({"client":b.client,"name":b.name})).collect::<Vec<_>>(),
            "health":health,"issues":issues,"verification":"Reachability does not prove a restarted application has called a tool"}));
    }
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"format_version":1,"gateways":results,"operations":operations})
            )?
        );
    } else if results.is_empty() {
        println!("No managed gateways. Run mcp-gate setup.");
    } else {
        for value in results {
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    }
    if !options.json {
        for operation in operations {
            println!(
                "{}: {}. Journal: {}",
                operation["phase"], operation["reason"], operation["journal"]
            );
        }
    }
    Ok(())
}

async fn probe(record: &store::Record, config: &Config) -> Vec<String> {
    let mut issues = Vec::new();
    eprintln!(
        "Explicit probe: initialization/discovery runs the original command, which may download dependencies. No tools are called."
    );
    if !runtime::maintenance(record, true).await.unwrap_or(false) {
        return vec![
            "Probe deferred: sessions active or gateway unreachable; no backend was started".into(),
        ];
    }
    match runtime::health(record).await {
        Ok(value) if value["workers"] == 0 => {
            if verify_probe(config).await.is_err() {
                issues.push(
                    "Backend discovery failed or catalog changed; rerun setup with review".into(),
                );
            }
        }
        _ => issues.push("Probe deferred: an existing backend may own exclusive resources".into()),
    }
    if runtime::maintenance(record, false).await.is_err() {
        issues.push(
            "Probe finished but maintenance release failed; inspect service before retrying".into(),
        );
    }
    issues
}

async fn verify_probe(config: &Config) -> anyhow::Result<()> {
    let recorded = Catalog::load(&config.catalog_file, &config.backend)?;
    let discovered = crate::install::generate_catalog(config).await?;
    // A stable serverInfo.version does not guarantee unchanged tool schemas.
    // Active diagnostics must detect drift without accepting or saving it.
    anyhow::ensure!(
        serde_json::to_value(recorded)? == serde_json::to_value(discovered)?,
        "Backend catalog changed; rerun setup with review"
    );
    Ok(())
}

//! Compute the ownership edits and retain the TOML decoration of changed values.
use crate::config::{Config, DEFAULT_SESSION_WORKERS, Ownership, OwnershipSource};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item};

#[derive(Serialize)]
pub(super) struct Change {
    pub field: &'static str,
    pub previous: Value,
    pub new: Value,
}

pub(super) struct Plan {
    pub after: String,
    pub changes: Vec<Change>,
}

impl Plan {
    pub fn new(before: &str, config: &Config, ownership: Ownership) -> Result<Self> {
        let mut document: DocumentMut = before
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid gateway configuration; values hidden"))?;
        let mut changes = Vec::new();
        let mode = match ownership {
            Ownership::Shared => "shared",
            Ownership::Session => "session",
        };
        set(
            &mut document,
            &mut changes,
            "ownership",
            config.ownership,
            mode,
        );
        if config.ownership_source != Some(OwnershipSource::Operator) {
            set(
                &mut document,
                &mut changes,
                "ownership_source",
                config.ownership_source,
                "operator",
            );
        }
        let workers = match ownership {
            Ownership::Shared => 1,
            Ownership::Session if config.max_workers == 1 => DEFAULT_SESSION_WORKERS,
            Ownership::Session => config.max_workers,
        };
        if workers != config.max_workers {
            set(
                &mut document,
                &mut changes,
                "max_workers",
                config.max_workers,
                i64::try_from(workers)?,
            );
        }
        if ownership == Ownership::Session {
            for (field, previous) in [
                ("shared_client_roots", json!(config.shared_client_roots)),
                ("shared_recovery", json!(config.shared_recovery)),
            ] {
                if document.remove(field).is_some() {
                    changes.push(Change {
                        field,
                        previous,
                        new: Value::Null,
                    });
                }
            }
        }
        Ok(Self {
            after: document.to_string(),
            changes,
        })
    }
}

pub(super) fn parse(text: &[u8]) -> Result<(&str, Config)> {
    let source = std::str::from_utf8(text)
        .ok()
        .context("Invalid gateway configuration; values hidden")?;
    let config = toml::from_str(source)
        .map_err(|_| anyhow::anyhow!("Invalid gateway configuration; values hidden"))?;
    Ok((source, config))
}

fn set<T: Copy + Serialize + Into<toml_edit::Value>>(
    document: &mut DocumentMut,
    changes: &mut Vec<Change>,
    field: &'static str,
    previous: impl Serialize,
    new: T,
) {
    let mut value = new.into();
    if let Some(old) = document.get(field).and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    document[field] = Item::Value(value);
    changes.push(Change {
        field,
        previous: json!(previous),
        new: json!(new),
    });
}

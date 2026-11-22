//! Schema for the embedded client catalog. Client config files use the separate document editor.
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{collections::BTreeSet, sync::LazyLock};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Descriptor {
    pub id: String,
    pub display_name: String,
    pub format: Format,
    pub section: String,
    pub nested_section: Option<String>,
    pub personal: Vec<PersonalLocation>,
    #[serde(default)]
    pub project: Vec<String>,
    #[serde(default)]
    pub claude_local_projects: bool,
    #[serde(default)]
    pub expansion: Expansion,
    pub connection: Connection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersonalLocation {
    pub os: Option<String>,
    pub env: Option<String>,
    pub path: Option<RelativePath>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum RelativePath {
    Text(String),
    Components(Vec<String>),
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Format {
    Toml,
    Json,
    Jsonc,
}

impl Format {
    pub fn is_toml(&self) -> bool {
        // Both JSON formats retain the existing tolerant parser and editor.
        matches!(self, Self::Toml)
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Expansion {
    #[default]
    None,
    DollarBrace,
    EnvAndFile,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Connection {
    Http {
        url_key: String,
        #[serde(default)]
        fields: Map<String, Value>,
        auth: Authentication,
    },
    Connect,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Authentication {
    Helper { key: String },
    File { key: String },
}

impl Authentication {
    pub fn key(&self) -> &str {
        match self {
            Self::Helper { key } | Self::File { key } => key,
        }
    }
}

pub(super) static CATALOG: LazyLock<Vec<Descriptor>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Catalog {
        client: Vec<Descriptor>,
    }

    // Embedded input ships with this binary. An invalid catalog is a build defect.
    let catalog: Catalog = toml::from_str(include_str!("descriptors.toml"))
        .expect("Embedded client descriptors must match the catalog schema");
    let mut ids = BTreeSet::new();
    for descriptor in &catalog.client {
        let id = &descriptor.id;
        assert!(
            id.split('-').all(|part| !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
                && ids.insert(id),
            "Embedded client IDs must be unique and kebab-case: {id}"
        );
    }
    catalog.client
});

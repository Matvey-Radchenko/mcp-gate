//! Client IDs are stored as strings and ordered by the embedded descriptor catalog.
use super::descriptors::{CATALOG, Descriptor};
use clap::{ValueEnum, builder::PossibleValue};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, fmt, sync::LazyLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Client(&'static str);

impl Client {
    // New descriptors enter the CLI and registry through the catalog
    // without a named Rust constant.
    pub const CODEX: Self = Self("codex");
    pub const OPENCODE: Self = Self("opencode");
    pub const CLAUDE_CODE: Self = Self("claude-code");

    pub(crate) fn all() -> &'static [Self] {
        static CLIENTS: LazyLock<Vec<Client>> =
            LazyLock::new(|| CATALOG.iter().map(|d| Client(&d.id)).collect());
        &CLIENTS
    }

    fn position(self) -> usize {
        // The constructor checks the catalog and the named constants
        // refer to entries pinned by the client ID tests.
        CATALOG
            .iter()
            .position(|descriptor| descriptor.id == self.0)
            .expect("A Client ID must have an embedded descriptor")
    }

    pub(crate) fn descriptor(self) -> &'static Descriptor {
        &CATALOG[self.position()]
    }
}

impl TryFrom<&str> for Client {
    type Error = &'static str;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        Self::all()
            .iter()
            .copied()
            .find(|client| client.0 == id)
            .ok_or("Unknown client ID")
    }
}

impl<'de> Deserialize<'de> for Client {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Self::try_from(id.as_str()).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Ord for Client {
    fn cmp(&self, other: &Self) -> Ordering {
        self.position().cmp(&other.position())
    }
}

impl PartialOrd for Client {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl ValueEnum for Client {
    fn value_variants<'a>() -> &'a [Self] {
        Self::all()
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(PossibleValue::new(self.0))
    }
}

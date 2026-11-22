//! Advisory heuristics for a saved catalog. Neither annotations nor a lack of
//! signals establish that all clients can safely use the same backend process.
use crate::{backend::Backend, catalog::Catalog};
use serde::{Deserialize, Serialize};

const RULES: &str = include_str!("../recipes/shareability.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rules {
    known_servers: Vec<KnownServer>,
    stateful_tool_tokens: Vec<String>,
    roots_tool_tokens: Vec<String>,
    roots_tool_names: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnownServer {
    aliases: Vec<String>,
    reason: String,
}

#[derive(Serialize)]
pub struct Advice {
    pub recommendation: Recommendation,
    pub message: &'static str,
    pub annotation_note: &'static str,
    pub signals: Vec<Signal>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recommendation {
    Session,
    SharedCandidate,
}

#[derive(Serialize)]
pub struct Signal {
    pub kind: SignalKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    KnownServer,
    StatefulTool,
    Roots,
    Capability,
    NoCatalog,
}

/// Use only the saved metadata and invocation tokens. The reported server
/// reasons come from checked-in rules, so command arguments never appear in advice.
pub fn advise(catalog: Option<&Catalog>, backend: &Backend) -> Advice {
    let Some(catalog) = catalog else {
        return Advice {
            recommendation: Recommendation::Session,
            message: "Shareability unknown. Keep session ownership.",
            annotation_note: ANNOTATION_NOTE,
            signals: vec![Signal {
                kind: SignalKind::NoCatalog,
                message: "The saved catalog is missing or unreadable.".into(),
            }],
        };
    };

    // The embedded data is checked by the unit tests and ships with this binary.
    let rules: Rules = serde_json::from_str(RULES).expect("Checked-in shareability rules schema");
    let mut signals = known_server_signals(&rules, catalog, backend);
    signals.extend(tool_signals(&rules, catalog));
    if catalog
        .server_info
        .capabilities
        .resources
        .as_ref()
        .is_some_and(|resources| resources.subscribe == Some(true))
    {
        signals.push(Signal {
            kind: SignalKind::Capability,
            message: "resources.subscribe signals per-client subscription state.".into(),
        });
    }
    let (recommendation, message) = if signals.is_empty() {
        (
            Recommendation::SharedCandidate,
            "No stateful signals found; review before sharing.",
        )
    } else {
        (Recommendation::Session, "Session ownership is recommended.")
    };
    Advice {
        recommendation,
        message,
        annotation_note: ANNOTATION_NOTE,
        signals,
    }
}

const ANNOTATION_NOTE: &str =
    "readOnlyHint and other tool annotations are not trusted as evidence of statelessness.";

fn known_server_signals(rules: &Rules, catalog: &Catalog, backend: &Backend) -> Vec<Signal> {
    let server_name = &catalog.server_info.server_info.name;
    let executable_names = std::iter::once(&backend.command)
        .chain(backend.entrypoint.iter())
        .filter_map(|path| path.file_stem()?.to_str());
    let package_tokens = backend
        .command_args
        .iter()
        .chain(&backend.args)
        .map(|arg| package_name(arg));
    let names = std::iter::once(server_name.as_str())
        .chain(executable_names)
        .chain(package_tokens)
        .collect::<Vec<_>>();
    rules
        .known_servers
        .iter()
        .filter(|server| {
            server
                .aliases
                .iter()
                .any(|alias| names.iter().any(|name| name.eq_ignore_ascii_case(alias)))
        })
        .map(|server| Signal {
            kind: SignalKind::KnownServer,
            message: server.reason.clone(),
        })
        .collect()
}

fn package_name(arg: &str) -> &str {
    let specifier = arg.strip_prefix("--package=").unwrap_or(arg);
    // Scoped npm names may also end in @<version-or-tag>.
    specifier
        .rsplit_once('@')
        .filter(|(name, tag)| !name.is_empty() && !tag.is_empty())
        .map_or(specifier, |(name, _)| name)
}

fn tool_signals<'a>(rules: &'a Rules, catalog: &'a Catalog) -> impl Iterator<Item = Signal> + 'a {
    catalog.tools.iter().filter_map(|tool| {
        let name = tool.name.to_lowercase();
        let tokens = || name.split(|c: char| !c.is_alphanumeric());
        if rules.roots_tool_names.contains(&name)
            || tokens().any(|token| rules.roots_tool_tokens.iter().any(|rule| rule == token))
        {
            return Some(Signal {
                kind: SignalKind::Roots,
                message: format!("Tool {} suggests use of client roots.", tool.name),
            });
        }
        tokens()
            .any(|token| rules.stateful_tool_tokens.iter().any(|rule| rule == token))
            .then(|| Signal {
                kind: SignalKind::StatefulTool,
                message: format!("Tool {} suggests client-specific state.", tool.name),
            })
    })
}

#[cfg(test)]
mod tests;

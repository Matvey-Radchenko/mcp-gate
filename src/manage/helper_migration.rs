//! Plan and journal client executable path edits, including external entries.
use super::{
    Selection, current, helper,
    store::{self, Journal, Registry},
    upgrade::Outcome,
};
use crate::clients::{self, Client, document};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(super) struct Rewrite {
    client: Client,
    name: String,
    file: PathBuf,
    keys: Vec<String>,
    before: String,
    old_prefix: String,
    new_prefix: String,
}

impl Rewrite {
    pub(super) fn summary(&self) -> Value {
        let mut summary = json!({"client":self.client,"name":self.name,"target":self.file});
        if matches!(
            self.client.descriptor().connection,
            clients::descriptors::Connection::Connect
        ) {
            summary["kind"] = json!(self.launcher());
        }
        summary
    }

    fn launcher(&self) -> &'static str {
        match self.client.descriptor().connection {
            clients::descriptors::Connection::Http { .. } => "headers helper",
            clients::descriptors::Connection::Connect => "stdio bridge",
        }
    }

    fn entry_keys(&self) -> (&str, &[String]) {
        let (field, keys) = self
            .keys
            .split_last()
            .expect("A client executable edit always ends in a field key");
        (field, keys)
    }

    fn edit(&self, source: &str) -> Result<String> {
        let toml = self.client.descriptor().format.is_toml();
        let (field, keys) = self.entry_keys();
        let entry = document::entry(source, toml, keys)?;
        ensure!(
            entry.get(field).and_then(Value::as_str) == Some(self.before.as_str())
                && helper::executable(self.client, &entry).is_some(),
            "Client executable changed after preview; rerun setup"
        );
        document::replace_string_prefix(
            source,
            toml,
            &self.keys,
            &self.old_prefix,
            &self.new_prefix,
        )
    }

    fn update_registry(&self, root: &Path, registry: &mut Registry) -> bool {
        let (field, keys) = self.entry_keys();
        let file = clients::identity(&self.file);
        let mut changed = false;
        for binding in registry
            .gateways
            .iter_mut()
            .flat_map(|record| &mut record.bindings)
        {
            if binding.client == self.client
                && binding.path == keys
                && clients::identity(&binding.target) == file
                && let Some(command) = helper::executable(self.client, &binding.after)
                && helper::versioned(root, &command.executable)
            {
                // Keep any unrelated drift visible, including edited launcher arguments.
                binding.after[field] = json!(format!(
                    "{}{}",
                    command.replacement(&current::path(root)),
                    command.arguments
                ));
                changed = true;
            }
        }
        changed
    }
}

#[derive(Default)]
pub(super) struct Plan {
    pub rewrites: Vec<Rewrite>,
    pub matching_current: bool,
    pub issues: Vec<clients::SourceIssue>,
}

impl Plan {
    fn review(
        &mut self,
        root: &Path,
        options: &Selection,
        binding: &clients::Binding,
        file: &Path,
        path: &[String],
        value: &Value,
    ) -> Result<()> {
        if helper::executable(binding.client, value)
            .is_some_and(|command| command.executable == current::path(root))
            && super::matches(options, binding.client, &binding.name)
        {
            self.matching_current = true;
        }
        self.rewrites
            .extend(rewrite(root, binding, file, path, value)?);
        Ok(())
    }

    pub(super) fn matches(&self, options: &Selection) -> bool {
        self.rewrites
            .iter()
            .any(|rewrite| super::matches(options, rewrite.client, &rewrite.name))
    }
}

pub(super) fn plan(root: &Path, registry: &Registry, options: &Selection) -> Result<Plan> {
    let home = store::home()?;
    let project = crate::platform::project_path(
        &options.project.clone().unwrap_or(std::env::current_dir()?),
    )?;
    // Service selection does not limit path migration. Stable executable
    // publication is shared by all clients using this state root.
    let scan = clients::inventory_current(&home, &project, &[], options.project.is_some())?;
    let mut seen = BTreeSet::new();
    let mut plan = Plan {
        issues: scan.issues,
        ..Plan::default()
    };
    for candidate in scan.candidates {
        let binding = &candidate.binding;
        if seen.insert((
            clients::identity(&binding.source),
            binding.source_path.clone(),
            binding.client,
        )) {
            plan.review(
                root,
                options,
                binding,
                &binding.source,
                &binding.source_path,
                &binding.direct,
            )?;
        }
    }
    // Saved targets also cover project overrides that the current inventory
    // did not visit. An unreadable client file is reported and left alone.
    for binding in registry.gateways.iter().flat_map(|record| &record.bindings) {
        if helper::field(binding.client).is_none()
            || !seen.insert((
                clients::identity(&binding.target),
                binding.path.clone(),
                binding.client,
            ))
        {
            continue;
        }
        match read_saved_entry(binding) {
            Ok(value) => plan.review(root, options, binding, &binding.target, &binding.path, &value)?,
            Err(_) => plan.issues.push(clients::SourceIssue {
                client: binding.client,
                source: binding.target.clone(),
                reason: "Cannot inspect a saved binding for executable migration. Check the client file's syntax and permissions, then rerun setup.".into(),
            }),
        }
    }
    Ok(plan)
}

fn read_saved_entry(binding: &clients::Binding) -> Result<Value> {
    let bytes = store::read_optional(&binding.target)?;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    document::entry(std::str::from_utf8(&bytes)?, binding.toml, &binding.path)
}

fn rewrite(
    root: &Path,
    binding: &clients::Binding,
    file: &Path,
    path: &[String],
    value: &Value,
) -> Result<Option<Rewrite>> {
    let Some(field) = helper::field(binding.client) else {
        return Ok(None);
    };
    let Some(before) = value.get(field).and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(command) = helper::executable(binding.client, value) else {
        return Ok(None);
    };
    if !helper::versioned(root, &command.executable) || !inside_store(root, &command.executable)? {
        return Ok(None);
    }
    let mut keys = path.to_vec();
    keys.push(field.into());
    Ok(Some(Rewrite {
        client: binding.client,
        name: binding.name.clone(),
        file: file.into(),
        keys,
        before: before.into(),
        old_prefix: command.prefix.into(),
        new_prefix: command.replacement(&current::path(root)),
    }))
}

fn inside_store(root: &Path, executable: &Path) -> Result<bool> {
    let store = root.join("binaries");
    // An old file can be absent, but an existing symlink or junction must
    // not turn a path in the managed namespace into an external executable.
    for path in [
        &store,
        executable.parent().context("Missing version directory")?,
        executable,
    ] {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => return Ok(false),
            Ok(meta) if path == executable => return Ok(meta.is_file()),
            Ok(meta) if !meta.is_dir() => return Ok(false),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(error).context("Cannot inspect the managed executable path"),
        }
    }
    Ok(false)
}

/// Setup owns its state lock. Client files and registry changes share one
/// recovery journal whose compare-before-write checks also guard user edits.
pub(super) fn apply(
    root: &Path,
    registry: &mut Registry,
    rewrites: &[Rewrite],
    outcome: &mut Outcome,
) -> Result<()> {
    if rewrites.is_empty() {
        return Ok(());
    }
    let path = root
        .join("operations")
        .join(format!("helpers-{}.json", uuid::Uuid::new_v4()));
    let mut journal = Journal {
        format_version: 1,
        phase: "preparing".into(),
        services: vec![],
        restart: vec![],
        migration: None,
        changes: vec![],
    };
    journal.save(&path)?;
    if let Err(error) = commit(root, registry, rewrites, &mut journal, &path) {
        let issues = journal.restore();
        journal.phase = if issues.is_empty() {
            "restored"
        } else {
            "needs-attention"
        }
        .into();
        journal.save(&path)?;
        return Err(error).with_context(|| {
            format!(
                "Client executable migration failed. Private journal: {}. {}",
                path.display(),
                issues.join(" ")
            )
        });
    }
    for rewrite in rewrites {
        outcome.messages.push(format!(
            "{} / {}: {} migrated to current in {}",
            rewrite.client,
            super::ui::safe(&rewrite.name),
            rewrite.launcher(),
            super::ui::safe(&rewrite.file.display().to_string())
        ));
        outcome.restart.insert(rewrite.client);
    }
    Ok(())
}

fn commit(
    root: &Path,
    registry: &mut Registry,
    rewrites: &[Rewrite],
    journal: &mut Journal,
    journal_path: &Path,
) -> Result<()> {
    let mut registry_changed = false;
    for rewrite in rewrites {
        let before = fs::read(&rewrite.file)?;
        let after = rewrite
            .edit(std::str::from_utf8(&before)?)
            .with_context(|| format!("Cannot edit {}", rewrite.file.display()))?;
        journal.write(journal_path, &rewrite.file, before, after.into_bytes())?;
        registry_changed |= rewrite.update_registry(root, registry);
    }
    if registry_changed {
        let path = root.join("registry.json");
        journal.write(
            journal_path,
            &path,
            store::read_optional(&path)?,
            serde_json::to_vec_pretty(registry)?,
        )?;
    }
    journal.phase = "complete".into();
    journal.save(journal_path)
}

#[cfg(test)]
mod tests;

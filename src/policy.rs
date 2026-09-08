//! Configured tool restrictions and output-directory namespaces; no server names.
use anyhow::{Result, ensure};
use rmcp::model::{CallToolRequestParams, ErrorData, Tool};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicy {
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub scoped_directories: Vec<ScopedDirectory>,
    #[serde(default)]
    pub instructions: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedDirectory {
    pub tool: String,
    pub argument: String,
    pub root: PathBuf,
}

impl ToolPolicy {
    /// Client-facing descriptions reflect path rewriting; upstream snapshot stays intact.
    pub fn expose(&self, tool: &Tool) -> Option<Tool> {
        if !self.permits(&tool.name) {
            return None;
        }
        let mut tool = tool.clone();
        for scope in self
            .scoped_directories
            .iter()
            .filter(|s| s.tool == tool.name)
        {
            let schema = std::sync::Arc::make_mut(&mut tool.input_schema);
            if let Some(property) = schema
                .get_mut("properties")
                .and_then(|p| p.get_mut(&scope.argument))
                .and_then(|p| p.as_object_mut())
            {
                property.insert("description".into(), serde_json::json!(
                    "Relative output folder inside this MCP session's cache (e.g. images). Do not pass an absolute path or '..'. Use returned file paths to copy assets into your project."
                ));
            }
        }
        Some(tool)
    }
    pub fn permits(&self, name: &str) -> bool {
        !self.disabled.iter().any(|n| n == name)
    }
    pub fn validate(&self) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        for name in &self.disabled {
            ensure!(
                !name.is_empty() && seen.insert(name),
                "Invalid/duplicate disabled tool"
            );
        }
        let mut seen = std::collections::BTreeSet::new();
        for scope in &self.scoped_directories {
            ensure!(
                !scope.tool.is_empty()
                    && !scope.argument.is_empty()
                    && seen.insert((&scope.tool, &scope.argument)),
                "Invalid/duplicate scoped argument"
            );
            ensure!(self.permits(&scope.tool), "Cannot scope a disabled tool");
            ensure!(
                scope.root.is_absolute()
                    && scope.root.components().count() > 2
                    && !scope.root.components().any(|c| c == Component::ParentDir),
                "Scoped output requires a specific absolute root directory"
            );
        }
        Ok(())
    }
    pub fn validate_catalog(&self, tools: &[Tool]) -> Result<()> {
        self.validate()?;
        for name in &self.disabled {
            ensure!(
                tools.iter().any(|t| t.name == *name),
                "Disabled tool missing from catalog"
            );
        }
        for scope in &self.scoped_directories {
            let tool = tools
                .iter()
                .find(|t| t.name == scope.tool)
                .ok_or_else(|| anyhow::anyhow!("Scoped tool missing from catalog"))?;
            ensure!(
                tool.input_schema
                    .get("properties")
                    .and_then(|p| p.get(&scope.argument))
                    .and_then(|p| p.get("type"))
                    == Some(&serde_json::json!("string")),
                "Scoped argument must be a string property"
            );
        }
        Ok(())
    }
    /// Namespace is gateway-generated per MCP session, never supplied by a client.
    /// This is argument containment, not an OS sandbox for the trusted backend.
    pub fn apply(
        &self,
        request: &mut CallToolRequestParams,
        namespace: uuid::Uuid,
    ) -> Result<(), ErrorData> {
        if !self.permits(&request.name) {
            return Err(ErrorData::invalid_params(
                "Tool disabled by gateway policy",
                None,
            ));
        }
        for scope in self
            .scoped_directories
            .iter()
            .filter(|s| s.tool == request.name)
        {
            let args = request.arguments.as_mut().ok_or_else(|| {
                ErrorData::invalid_params("Missing scoped directory argument", None)
            })?;
            let relative = args
                .get(&scope.argument)
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ErrorData::invalid_params("Scoped directory must be a relative string", None)
                })?;
            let path = scoped_path(&scope.root, namespace, relative)?;
            args.insert(scope.argument.clone(), serde_json::json!(path));
        }
        Ok(())
    }
}

fn scoped_path(root: &Path, namespace: uuid::Uuid, relative: &str) -> Result<PathBuf, ErrorData> {
    let invalid = || {
        ErrorData::invalid_params(
            "Output directory must stay within this session's cache; use a relative path without '..'",
            None,
        )
    };
    let input = Path::new(relative);
    if relative.len() > 4096
        || relative.contains(['\\', ':', '\0'])
        || input
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(invalid());
    }
    let path = root.join(namespace.to_string()).join(input);
    // Reject preexisting symlink components, including the configured root. Local
    // filesystem races remain outside our single-user process-isolation boundary.
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(invalid()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(invalid()),
        }
    }
    Ok(path)
}

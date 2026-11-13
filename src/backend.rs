//! Backend invocation policy. Protocol routing never branches on a server/tool name.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use tokio::process::Command;

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    // Existing v1 installations retain their isolation checks.
    #[default]
    ChromeDevtools,
    Stdio,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Backend {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub docker: bool,
    #[serde(default)]
    pub profile: Profile,
    pub command: PathBuf,
    /// Original launch directory; omitted legacy configs retain their state-directory default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<PathBuf>,
    /// Interpreter arguments before the pinned entrypoint (e.g. Java's -jar).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_args: Vec<String>,
    /// Each named variable receives a private, retained directory per worker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directory_env: Vec<String>,
    /// Optionally use one allocated directory as cwd (relative file outputs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory_env: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<PathBuf>,
    pub version: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Additional names inherited by the generic profile, never environment values.
    #[serde(default)]
    pub inherit_env: Vec<String>,
    /// Private UTF-8 files, read only when starting the backend. Not stored in catalogs.
    #[serde(default)]
    pub env_files: BTreeMap<String, PathBuf>,
}

impl Backend {
    pub fn artifact(&self) -> &std::path::Path {
        self.entrypoint.as_deref().unwrap_or(&self.command)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.command.is_absolute(),
            "Backend command must be absolute"
        );
        ensure!(
            self.artifact().is_absolute(),
            "Backend artifact must be absolute"
        );
        ensure!(!self.version.is_empty(), "Backend version must be pinned");
        ensure!(
            self.working_directory
                .as_ref()
                .is_none_or(|p| p.is_absolute() && p.is_dir()),
            "Backend working directory must be an existing absolute directory"
        );
        let mut directories = std::collections::BTreeSet::new();
        for name in &self.directory_env {
            ensure!(
                !name.is_empty()
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && directories.insert(name)
                    && !["HOME", "PATH", "TMPDIR", "LANG", "LC_ALL"].contains(&name.as_str())
                    && !self.env.contains_key(name)
                    && !self.env_files.contains_key(name)
                    && !self.inherit_env.contains(name),
                "Invalid or conflicting worker directory environment variable"
            );
        }
        ensure!(
            self.working_directory_env
                .as_ref()
                .is_none_or(|name| self.directory_env.contains(name)),
            "Working directory must name an allocated worker directory"
        );
        for key in self
            .env
            .keys()
            .chain(self.env_files.keys())
            .chain(&self.inherit_env)
        {
            ensure!(
                !key.is_empty() && !key.contains(['=', '\0']),
                "Invalid environment name"
            );
        }
        for (name, path) in &self.env_files {
            ensure!(
                path.is_absolute(),
                "Secret reference must be an absolute file path"
            );
            ensure!(!self.env.contains_key(name), "Duplicate environment source");
        }
        if self.docker {
            crate::platform::docker::validate(&self.args)?;
        }
        if self.profile == Profile::Stdio {
            return Ok(());
        }
        ensure!(
            self.command_args.is_empty() && self.directory_env.is_empty(),
            "Custom interpreter/directory options require the stdio profile"
        );
        ensure!(
            self.entrypoint.is_some(),
            "Chrome profile requires an entrypoint"
        );
        ensure!(
            self.args.iter().any(|a| a == "--isolated"),
            "Backend must use --isolated"
        );
        for arg in &self.args {
            ensure!(
                arg != "--isolated=false",
                "Backend isolation cannot be disabled"
            );
            let key = arg.split('=').next().unwrap_or(arg);
            ensure!(
                !matches!(
                    key,
                    "--userDataDir"
                        | "--user-data-dir"
                        | "--autoConnect"
                        | "--auto-connect"
                        | "--browserUrl"
                        | "--browser-url"
                        | "--wsEndpoint"
                        | "--ws-endpoint"
                        | "--no-isolated"
                        | "-u"
                        | "-w"
                ),
                "Shared browser arguments are prohibited: {key}"
            );
        }
        Ok(())
    }

    pub fn command(&self) -> Result<Command> {
        let mut command = Command::new(&self.command);
        command.args(&self.command_args);
        if let Some(entrypoint) = &self.entrypoint {
            command.arg(entrypoint);
        }
        command.args(&self.args);
        if self.profile == Profile::Stdio {
            // Generic backends do not accidentally inherit unrelated credentials.
            command.env_clear();
            for key in crate::platform::BASE_ENVIRONMENT
                .iter()
                .copied()
                .chain(self.inherit_env.iter().map(String::as_str))
            {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
        }
        command.envs(&self.env);
        for (name, path) in &self.env_files {
            crate::platform::validate_private(path)?;
            let value = std::fs::read_to_string(path)?;
            ensure!(!value.contains('\0'), "Invalid credential file encoding");
            command.env(name, value.trim_end_matches(['\r', '\n']));
        }
        if self.profile == Profile::ChromeDevtools {
            command
                .env("CHROME_DEVTOOLS_MCP_NO_USAGE_STATISTICS", "1")
                .env("CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS", "1")
                .env_remove("NODE_OPTIONS");
        }
        Ok(command)
    }

    pub fn configure_directories(
        &self,
        command: &mut Command,
        state: &std::path::Path,
    ) -> Result<()> {
        if self.directory_env.is_empty() {
            return Ok(());
        }
        let parent = state.join("worker-data");
        if let Err(error) = crate::platform::private_dir(&parent)
            && error
                .downcast_ref::<std::io::Error>()
                .is_none_or(|e| e.kind() != std::io::ErrorKind::AlreadyExists)
        {
            return Err(error);
        }
        ensure!(
            std::fs::symlink_metadata(&parent)?.file_type().is_dir(),
            "Worker data directory must be a real directory"
        );
        crate::platform::private_permissions(&parent)?;
        let root = parent.join(uuid::Uuid::new_v4().to_string());
        crate::platform::private_dir(&root)?;
        for name in &self.directory_env {
            let path = root.join(name);
            crate::platform::private_dir(&path)?;
            command.env(name, path);
            if self.working_directory_env.as_ref() == Some(name) {
                command.current_dir(root.join(name));
            }
        }
        // Intentionally retained: browser downloads may be needed after a session closes.
        Ok(())
    }
}

//! Terminal interaction stays separate from JSON output and lifecycle operations.
use super::Selection;
use anyhow::{Result, ensure};
use inquire::{InquireError, MultiSelect, Select, Text};
use std::io::IsTerminal;

#[derive(Debug)]
pub struct Canceled;
impl std::fmt::Display for Canceled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Canceled. No changes applied.")
    }
}
impl std::error::Error for Canceled {}

pub fn interactive(options: &Selection) -> bool {
    !options.yes && !options.json && !options.dry_run && terminal()
}
pub fn terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}
pub fn safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn answer<T>(result: Result<T, InquireError>) -> Result<T> {
    match result {
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            Err(Canceled.into())
        }
        Err(_) => anyhow::bail!(
            "Cannot read terminal input. Retry in a terminal, or use explicit --client/--server with --yes."
        ),
        Ok(value) => Ok(value),
    }
}
pub fn directory(message: &str) -> Result<Option<std::path::PathBuf>> {
    loop {
        let Some(path) = answer(Text::new(message).prompt_skippable())? else {
            return Ok(None);
        };
        let path = if let Some(tail) = path.strip_prefix("~/") {
            super::store::home()?.join(tail)
        } else {
            path.into()
        };
        if path.is_absolute() && path.is_dir() {
            return Ok(Some(crate::platform::project_path(&path)?));
        }
        println!("Enter an existing absolute directory, or press Esc to go back.");
    }
}
pub fn select(message: &str, options: Vec<String>) -> Result<Option<usize>> {
    answer(
        Select::new(message, options)
            .with_page_size(10)
            .raw_prompt_skippable(),
    )
    .map(|v| v.map(|v| v.index))
}
pub fn multiple(message: &str, options: Vec<String>) -> Result<Option<Vec<usize>>> {
    answer(
        MultiSelect::new(message, options)
            .with_page_size(12)
            .with_help_message(
                "Type to search · arrows move · Space selects · Enter reviews · Esc goes back",
            )
            .with_formatter(&|values| format!("{} selected", values.len()))
            .raw_prompt_skippable(),
    )
    .map(|v| v.map(|v| v.into_iter().map(|v| v.index).collect()))
}
pub fn confirm(message: &str) -> Result<bool> {
    ensure!(
        terminal(),
        "No changes applied. Use a terminal for the wizard, --dry-run to preview, or explicit --client/--server with --yes."
    );
    answer(
        inquire::Confirm::new(message)
            .with_default(false)
            .prompt_skippable(),
    )
    .map(|v| v.unwrap_or(false))
}
pub fn scope(options: &mut Selection) -> Result<bool> {
    if options.project.is_some() {
        return Ok(true);
    }
    let current = crate::platform::project_path(&std::env::current_dir()?)?;
    let home = crate::clients::identity(&super::store::home()?);
    let show_current = current != home;
    loop {
        let mut labels = vec!["Personal settings on this computer".into()];
        if show_current {
            labels.push(format!("Project: {}", safe(&current.display().to_string())));
        }
        labels.extend(["Another project directory".into(), "Cancel".into()]);
        match select("Where do you want to set up MCP connections?", labels)? {
            Some(0) => return Ok(true),
            Some(1) if show_current => {
                options.project = Some(current);
                return Ok(true);
            }
            Some(index) if index == 1 + usize::from(show_current) => {
                let Some(path) = directory("Project directory (absolute path):")? else {
                    continue;
                };
                if path == home {
                    println!(
                        "Home uses personal settings; choose a project directory or personal settings."
                    );
                    continue;
                }
                options.project = Some(path);
                return Ok(true);
            }
            _ => return Ok(false),
        }
    }
}

pub fn canceled(json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({"format_version":1,"phase":"canceled"})
        );
    } else {
        println!("Canceled. No changes applied.");
    }
}

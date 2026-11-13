//! Bounded private history contains typed lifecycle fields only, never peer text.
use anyhow::Result;
use std::{fs, io::Write, path::Path};

pub(super) fn append(path: &Path, event: &serde_json::Value) -> Result<()> {
    let mut file = if path.try_exists()? {
        crate::platform::validate_private(path)?;
        if fs::metadata(path)?.len() >= 65536 {
            let previous = path.with_extension("previous.jsonl");
            if previous.try_exists()? {
                crate::platform::validate_private(&previous)?;
                fs::remove_file(&previous)?;
            }
            fs::rename(path, previous)?;
            crate::platform::private_file(path)?
        } else {
            fs::OpenOptions::new().append(true).open(path)?
        }
    } else {
        crate::platform::private_file(path)?
    };
    writeln!(file, "{event}")?;
    file.sync_data()?;
    Ok(())
}

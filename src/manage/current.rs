//! Publish the short-lived client helper independently of service identity.
use super::store;
use crate::{catalog, platform};
use anyhow::{Context, Result, ensure};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(crate) fn path(root: &Path) -> PathBuf {
    root.join("current").join(platform::binary_name())
}

/// Setup holds its state lock while publishing the installed versioned source.
pub(crate) fn publish(root: &Path, binary: &Path, hash: &str) -> Result<()> {
    #[cfg(unix)]
    let link: Option<Link> = Some(|source, target| fs::hard_link(source, target));
    // NTFS shares access modes across hardlinks to the same data stream.
    // An independent copy keeps the running service from locking current.
    #[cfg(windows)]
    let link = None;
    publish_with(root, binary, hash, link, replace)
}

type Link = fn(&Path, &Path) -> io::Result<()>;

fn publish_with(
    root: &Path,
    binary: &Path,
    hash: &str,
    link: Option<Link>,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<()> {
    let target = path(root);
    let parent = target
        .parent()
        .context("Missing current binary directory")?;
    match fs::symlink_metadata(parent) {
        Ok(meta) => ensure!(
            meta.is_dir(),
            "Current binary directory must be a real directory"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => platform::private_dir(parent)?,
        Err(error) => return Err(error.into()),
    }
    if digest(&target)?.as_deref() == Some(hash) {
        return Ok(());
    }
    store::with_temporary(|temporary| {
        let linked = link
            .map(|link| temporary.link(binary, parent, link))
            .transpose();
        if !matches!(linked, Ok(Some(()))) {
            let copy: Result<()> = (|| {
                let mut file = temporary.open(parent)?;
                catalog::copy_digest(binary, &mut file)?;
                file.sync_all()?;
                Ok(())
            })();
            copy.with_context(|| match linked {
                Err(error) => format!("Copy fallback failed after hardlink failed: {error}"),
                Ok(_) => "Cannot stage current helper copy".into(),
            })?;
        }
        let staged = temporary.path()?;
        ensure!(
            digest(staged)?.as_deref() == Some(hash),
            "Staged helper binary digest mismatch"
        );
        platform::executable(staged)?;
        temporary.publish_staged(&target, replace)
    })
    .with_context(|| {
        format!(
            "Cannot publish {}. Close any clients running the headers helper and rerun setup",
            target.display()
        )
    })
}

pub(super) fn digest(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_file(),
                "Current helper must be a real executable file"
            );
            Ok(Some(catalog::digest(path)?))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn replace(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    return retry_replace(
        || store::replace(source, target),
        windows_in_use,
        || std::thread::sleep(std::time::Duration::from_millis(100)),
    );
    #[cfg(not(windows))]
    store::replace(source, target)
}

#[cfg(windows)]
fn windows_in_use(error: &io::Error) -> bool {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
    };
    // An executable image mapped by a helper can be reported as access denied.
    matches!(
        error.raw_os_error().map(|code| code as u32),
        Some(ERROR_ACCESS_DENIED | ERROR_LOCK_VIOLATION | ERROR_SHARING_VIOLATION)
    )
}

#[cfg(any(windows, test))]
fn retry_replace(
    mut replace: impl FnMut() -> io::Result<()>,
    retryable: impl Fn(&io::Error) -> bool,
    mut wait: impl FnMut(),
) -> io::Result<()> {
    // Each failed rename leaves the old entry intact. Only the rename is
    // retried, with at most thirty 100 ms waits on Windows.
    for _ in 0..30 {
        match replace() {
            Err(error) if retryable(&error) => wait(),
            result => return result,
        }
    }
    replace()
}

#[cfg(test)]
mod tests;

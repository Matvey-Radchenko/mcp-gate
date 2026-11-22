//! Publish the stable client executable independently of service identity.
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
/// The legacy installer publishes into a newly created prefix it owns.
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
        cleanup(parent, |path| fs::remove_file(path));
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
                Ok(_) => "Cannot stage current binary copy".into(),
            })?;
        }
        let staged = temporary.path()?;
        ensure!(
            digest(staged)?.as_deref() == Some(hash),
            "Staged current binary digest mismatch"
        );
        platform::executable(staged)?;
        temporary.publish_staged(&target, replace)
    })
    .with_context(|| {
        format!(
            "Cannot publish {}. Check the current directory's permissions and rerun setup",
            target.display()
        )
    })?;
    // A failed rollback may leave the only previous copy aside. Clean up only
    // once a verified executable is installed at the stable path.
    cleanup(parent, |path| fs::remove_file(path));
    Ok(())
}

pub(super) fn digest(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_file(),
                "Current binary must be a real executable file"
            );
            Ok(Some(catalog::digest(path)?))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn replace(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(windows)]
    return replace_with_aside(
        source,
        target,
        store::replace,
        store::rename_new,
        windows_in_use,
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
fn replace_with_aside(
    source: &Path,
    target: &Path,
    mut replace: impl FnMut(&Path, &Path) -> io::Result<()>,
    mut rename_new: impl FnMut(&Path, &Path) -> io::Result<()>,
    in_use: impl Fn(&io::Error) -> bool,
) -> io::Result<()> {
    let blocked = match replace(source, target) {
        Err(error) if in_use(&error) => error,
        result => return result,
    };
    let aside = target.with_file_name(format!(
        "{}{}-{}",
        aside_prefix(),
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    // Windows can rename a mapped image to an unused name. Keep the old
    // executable available for rollback until the staged image is in place.
    rename_new(target, &aside).map_err(|error| {
        io::Error::other(format!(
            "Replacement was blocked ({blocked}); cannot move the previous executable aside: {error}"
        ))
    })?;
    if let Err(error) = replace(source, target) {
        return Err(io::Error::other(match rename_new(&aside, target) {
            Ok(()) => format!(
                "Cannot install the staged executable; previous executable restored: {error}"
            ),
            Err(rollback) => format!(
                "Cannot install the staged executable ({error}) or restore the previous one ({rollback}). Previous executable retained at {}",
                aside.display()
            ),
        }));
    }
    Ok(())
}

fn aside_prefix() -> String {
    format!("{}.old-", platform::binary_name())
}

fn cleanup(parent: &Path, mut remove: impl FnMut(&Path) -> io::Result<()>) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let prefix = aside_prefix();
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix)
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            // Live Windows processes can still hold aside files open.
            let _ = remove(&entry.path());
        }
    }
}

#[cfg(test)]
mod tests;

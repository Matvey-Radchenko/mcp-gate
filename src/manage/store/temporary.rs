//! Stage private files and replace them without exposing partial writes.
use anyhow::{Context, Result, ensure};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub(in crate::manage) fn ensure_replaceable(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => ensure!(
            !meta.file_type().is_symlink(),
            "Refusing to replace a symlink"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(in crate::manage) struct Temporary {
    path: Option<PathBuf>,
}

/// Owns the staged path until publication, removing it if the operation aborts.
pub(in crate::manage) fn with_temporary<T>(
    operation: impl FnOnce(&mut Temporary) -> Result<T>,
) -> Result<T> {
    let mut temporary = Temporary { path: None };
    let result = operation(&mut temporary);
    let Some(path) = temporary.path else {
        return result;
    };
    match fs::remove_file(path) {
        Ok(()) => result,
        Err(error) if error.kind() == io::ErrorKind::NotFound => result,
        Err(cleanup) => match result {
            Ok(_) => Err(cleanup).context("Cannot remove staged file"),
            Err(error) => Err(error.context(format!("Cannot remove staged file: {cleanup}"))),
        },
    }
}

impl Temporary {
    pub(in crate::manage) fn path(&self) -> Result<&Path> {
        self.path.as_deref().context("Missing staged file")
    }

    pub(in crate::manage) fn open(&mut self, parent: &Path) -> Result<fs::File> {
        ensure!(self.path.is_none(), "Temporary file already open");
        let path = temporary_path(parent);
        let file = crate::platform::private_file(&path);
        // A Windows ACL failure can leave a file behind. An existing file belongs
        // to another writer and must be left alone.
        let already_exists = file
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<io::Error>())
            .is_some_and(|error| error.kind() == io::ErrorKind::AlreadyExists);
        if !already_exists {
            self.path = Some(path);
        }
        file
    }

    pub(in crate::manage) fn link(
        &mut self,
        source: &Path,
        parent: &Path,
        link: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.path.is_some() {
            return Err(io::Error::other("Temporary file already staged"));
        }
        let path = temporary_path(parent);
        link(source, &path)?;
        self.path = Some(path);
        Ok(())
    }

    pub(in crate::manage) fn publish(&mut self, file: fs::File, target: &Path) -> Result<()> {
        file.sync_all()?;
        drop(file);
        self.publish_staged(target, replace)
    }

    pub(in crate::manage) fn publish_staged(
        &mut self,
        target: &Path,
        publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> Result<()> {
        let parent = target.parent().context("Missing target parent")?;
        let path = self.path.as_mut().context("Missing staged file")?;
        if path.parent() != Some(parent) {
            // A copy hashed during staging only learns its final directory after EOF.
            // Move the staged file into that directory before the atomic replacement.
            let sibling = parent.join(path.file_name().context("Missing temporary file name")?);
            fs::rename(&*path, &sibling)?;
            *path = sibling;
        }
        ensure_replaceable(target)?;
        publish(path, target).context("Cannot replace file")?;
        self.path = None;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

fn temporary_path(parent: &Path) -> PathBuf {
    parent.join(format!(".mcp-gate-{}.tmp", uuid::Uuid::new_v4()))
}

pub(in crate::manage) fn replace(temporary: &Path, path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return fs::rename(temporary, path);
    #[cfg(windows)]
    move_file(
        temporary,
        path,
        windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING,
    )
}

#[cfg(windows)]
pub(in crate::manage) fn rename_new(source: &Path, target: &Path) -> io::Result<()> {
    move_file(source, target, 0)
}

#[cfg(windows)]
fn move_file(source: &Path, target: &Path, flags: u32) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let a: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both paths are nul-terminated, owned UTF-16 buffers.
    let moved = unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), flags | MOVEFILE_WRITE_THROUGH) };
    if moved == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn removes_relocated_temporary_when_replacement_fails() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("version-hash");
        let target = directory.join("occupied");
        fs::create_dir_all(&target).unwrap();
        let result = with_temporary(|temporary| {
            let mut file = temporary.open(root.path())?;
            file.write_all(b"staged bytes")?;
            temporary.publish(file, &target)
        });
        assert!(result.is_err());
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
    }
}

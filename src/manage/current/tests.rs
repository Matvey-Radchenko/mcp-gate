use super::*;
use crate::manage::store::{Record, Registry};
use std::cell::Cell;

fn binary(root: &Path, version: &str, content: &[u8]) -> PathBuf {
    let path = root
        .join("binaries")
        .join(version)
        .join(platform::binary_name());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    platform::executable(&path).unwrap();
    path
}

#[test]
fn publishes_a_real_file_once_and_atomically_switches_versions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let old = binary(root, "old", b"old binary");
    let new = binary(root, "new", b"new binary");
    let old_hash = catalog::digest(&old).unwrap();
    let new_hash = catalog::digest(&new).unwrap();
    let target = path(root);

    publish(root, &old, &old_hash).unwrap();
    assert!(fs::symlink_metadata(&target).unwrap().is_file());
    assert_eq!(catalog::digest(&target).unwrap(), old_hash);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(&target).unwrap().ino(),
            fs::metadata(&old).unwrap().ino()
        );
        assert_ne!(fs::metadata(&target).unwrap().mode() & 0o100, 0);
    }
    publish_with(
        root,
        &old,
        &old_hash,
        Some(|_, _| panic!("identical link")),
        |_, _| panic!("identical replace"),
    )
    .unwrap();

    publish_with(
        root,
        &new,
        &new_hash,
        Some(|_, _| Err(io::Error::other("hardlink unavailable"))),
        |staged, destination| {
            assert_eq!(destination, target);
            assert_eq!(catalog::digest(destination).unwrap(), old_hash);
            assert_eq!(catalog::digest(staged).unwrap(), new_hash);
            store::replace(staged, destination)
        },
    )
    .unwrap();
    assert_eq!(catalog::digest(&target).unwrap(), new_hash);
    assert_eq!(catalog::digest(&old).unwrap(), old_hash);
    assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn invalid_staging_and_failed_replacement_keep_the_previous_helper() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let old = binary(root, "old", b"old binary");
    let new = binary(root, "new", b"new binary");
    let old_hash = catalog::digest(&old).unwrap();
    let new_hash = catalog::digest(&new).unwrap();
    publish(root, &old, &old_hash).unwrap();
    let failure = publish(root, &new, "wrong digest").unwrap_err();
    assert!(format!("{failure:#}").contains("digest mismatch"));
    let locked = publish_with(root, &new, &new_hash, None, |_, _| {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "locked"))
    });
    assert!(locked.unwrap_err().to_string().contains("rerun setup"));
    assert_eq!(catalog::digest(&path(root)).unwrap(), old_hash);
    assert_eq!(fs::read_dir(root.join("current")).unwrap().count(), 1);
}

#[test]
fn replace_retries_only_busy_failures_and_bounds_waits() {
    let calls = Cell::new(0);
    let waits = Cell::new(0);
    let busy_for = |limit, error| {
        retry_replace(
            || {
                calls.set(calls.get() + 1);
                if calls.get() <= limit {
                    return Err(io::Error::from(error));
                }
                Ok(())
            },
            |error| error.kind() == io::ErrorKind::PermissionDenied,
            || waits.set(waits.get() + 1),
        )
    };
    busy_for(2, io::ErrorKind::PermissionDenied).unwrap();
    assert_eq!((calls.replace(0), waits.replace(0)), (3, 2));
    assert!(busy_for(100, io::ErrorKind::PermissionDenied).is_err());
    assert_eq!((calls.replace(0), waits.replace(0)), (31, 30));
    assert!(busy_for(100, io::ErrorKind::NotFound).is_err());
    assert_eq!((calls.get(), waits.get()), (1, 0));
}

#[cfg(windows)]
#[test]
fn windows_sharing_errors_are_retryable() {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
    };
    for code in [
        ERROR_ACCESS_DENIED,
        ERROR_LOCK_VIOLATION,
        ERROR_SHARING_VIOLATION,
    ] {
        assert!(windows_in_use(&io::Error::from_raw_os_error(code as i32)));
    }
    assert!(!windows_in_use(&io::Error::from_raw_os_error(
        ERROR_FILE_NOT_FOUND as i32
    )));
}

#[cfg(windows)]
#[test]
fn versioned_file_sharing_lock_does_not_pin_current() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let old = binary(root, "old", b"old binary");
    let new = binary(root, "new", b"new binary");
    publish(root, &old, &catalog::digest(&old).unwrap()).unwrap();
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&old)
        .unwrap();
    let new_hash = catalog::digest(&new).unwrap();
    publish(root, &new, &new_hash).unwrap();
    assert_eq!(catalog::digest(&path(root)).unwrap(), new_hash);
    assert_eq!(locked.metadata().unwrap().len(), 10);
}

#[test]
fn retains_current_digest_and_registry_references() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let active = binary(root, "active", b"current");
    let duplicate = binary(root, "duplicate", b"current");
    let service = binary(root, "registered", b"service");
    let unreferenced = binary(root, "unused", b"unreferenced");
    publish(root, &active, &catalog::digest(&active).unwrap()).unwrap();
    let registry = Registry {
        format_version: 1,
        gateways: vec![Record {
            removing: true,
            id: "test".into(),
            release: root.join("release"),
            label: "local.mcp-gate.test".into(),
            binary_hash: catalog::digest(&service).unwrap(),
            binary: service.clone(),
            bindings: vec![],
        }],
    };
    for protected in [active, duplicate, service] {
        assert!(store::retain_binary(root, &registry, &protected).unwrap());
    }
    assert!(!store::retain_binary(root, &registry, &unreferenced).unwrap());
}

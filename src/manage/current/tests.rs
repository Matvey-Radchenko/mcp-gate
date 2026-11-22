use super::*;
use crate::manage::store::{Record, Registry};

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
fn busy_target_is_moved_aside_before_installing_the_staged_image() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join(platform::binary_name());
    let source = temp.path().join("staged");
    fs::write(&target, b"old").unwrap();
    fs::write(&source, b"new").unwrap();
    let mut aside = None;
    replace_with_aside(
        &source,
        &target,
        |source, target| {
            if target.exists() {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            store::replace(source, target)
        },
        |from, to| {
            assert_eq!(from, target);
            assert_eq!(to.parent(), target.parent());
            let name = to.file_name().unwrap().to_str().unwrap();
            let prefix = format!("{}{}-", aside_prefix(), std::process::id());
            assert!(uuid::Uuid::parse_str(name.strip_prefix(&prefix).unwrap()).is_ok());
            fs::rename(from, to)?;
            aside = Some(to.to_owned());
            Ok(())
        },
        |error| error.kind() == io::ErrorKind::PermissionDenied,
    )
    .unwrap();
    assert_eq!(fs::read(target).unwrap(), b"new");
    assert_eq!(fs::read(aside.unwrap()).unwrap(), b"old");
}

#[test]
fn failed_install_restores_the_previous_image_or_reports_its_recovery_path() {
    for rollback_fails in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join(platform::binary_name());
        let source = temp.path().join("staged");
        fs::write(&target, b"old").unwrap();
        let mut aside = None;
        let failure = replace_with_aside(
            &source,
            &target,
            |_, to| {
                if to.exists() {
                    Err(io::ErrorKind::PermissionDenied.into())
                } else {
                    Err(io::Error::other("install failed"))
                }
            },
            |from, to| {
                if from == target {
                    aside = Some(to.to_owned());
                } else if rollback_fails {
                    return Err(io::Error::other("rollback failed"));
                }
                fs::rename(from, to)
            },
            |error| error.kind() == io::ErrorKind::PermissionDenied,
        )
        .unwrap_err()
        .to_string();
        assert!(failure.contains("install failed"));
        let aside = aside.unwrap();
        if rollback_fails {
            assert!(failure.contains("rollback failed"));
            assert!(failure.contains(aside.to_str().unwrap()));
            assert_eq!(fs::read(aside).unwrap(), b"old");
        } else {
            assert!(failure.contains("previous executable restored"));
            assert_eq!(fs::read(target).unwrap(), b"old");
            assert!(!aside.exists());
        }
    }
}

#[test]
fn unrelated_replace_error_does_not_move_the_target() {
    let error = replace_with_aside(
        Path::new("staged"),
        Path::new("target"),
        |_, _| Err(io::ErrorKind::NotFound.into()),
        |_, _| panic!("unexpected rename"),
        |error| error.kind() == io::ErrorKind::PermissionDenied,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn cleanup_spares_locked_asides_and_unrelated_entries() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path();
    let target = parent.join(platform::binary_name());
    let stale = parent.join(format!("{}stale", aside_prefix()));
    let locked = parent.join(format!("{}locked", aside_prefix()));
    let directory = parent.join(format!("{}directory", aside_prefix()));
    let other = parent.join("unrelated.old-stale");
    for path in [&target, &stale, &locked, &other] {
        fs::write(path, b"fixture").unwrap();
    }
    fs::create_dir(&directory).unwrap();
    cleanup(parent, |path| {
        assert!(path == locked || path == stale);
        if path == locked {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        fs::remove_file(path)
    });
    assert!(!stale.exists());
    for path in [target, locked, other, directory] {
        assert!(path.exists());
    }
}

#[cfg(windows)]
#[test]
fn windows_sharing_errors_trigger_rename_aside() {
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
struct RunningImage(std::process::Child);

#[cfg(windows)]
impl Drop for RunningImage {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(windows)]
#[test]
fn publishes_over_a_running_windows_image_and_cleans_it_up_after_exit() {
    use std::process::{Command, Stdio};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ping = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
        .join("System32")
        .join("ping.exe");
    let old = binary(root, "old", b"");
    fs::copy(ping, &old).unwrap();
    let old_hash = catalog::digest(&old).unwrap();
    publish(root, &old, &old_hash).unwrap();
    let target = path(root);
    let mut ping = RunningImage(
        Command::new(&target)
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    assert!(ping.0.try_wait().unwrap().is_none());

    let new = binary(root, "new", b"replacement");
    let new_hash = catalog::digest(&new).unwrap();
    publish(root, &new, &new_hash).unwrap();
    assert_eq!(catalog::digest(&target).unwrap(), new_hash);
    assert!(ping.0.try_wait().unwrap().is_none());
    let mut asides = fs::read_dir(root.join("current"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(&aside_prefix())
        });
    let aside = asides.next().unwrap();
    assert_eq!(catalog::digest(&aside).unwrap(), old_hash);
    assert!(asides.next().is_none());

    drop(ping);
    publish(root, &new, &new_hash).unwrap();
    assert!(!aside.exists());
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

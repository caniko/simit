//! Checkpoint updater state across read-only tools, including failed commands.
use super::engine::{save, verify_lock};
use crate::review::digest;
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
};

const MARKER: &str = "generation-checkpoint.json";

#[derive(PartialEq, Eq)]
struct Entry {
    permissions: fs::Permissions,
    kind: Kind,
}

#[derive(PartialEq, Eq)]
enum Kind {
    Directory,
    File(String),
    Symlink(PathBuf, bool),
}

fn symlink(target: &Path, path: &Path, _directory: bool) -> Result<()> {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, path)?;
    #[cfg(windows)]
    if _directory {
        std::os::windows::fs::symlink_dir(target, path)?;
    } else {
        std::os::windows::fs::symlink_file(target, path)?;
    }
    Ok(())
}

pub fn ensure_idle(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root.join(MARKER)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => bail!(
            "interrupted generation: reconcile the persistent updater state checkpoint in {}",
            root.join(MARKER).display()
        ),
    }
}

fn capture(
    root: &Path,
    checkout: &Path,
    backup: &Path,
    copy: bool,
) -> Result<BTreeMap<PathBuf, Entry>> {
    let mut pending = vec![root.to_owned()];
    let mut entries = BTreeMap::new();
    while let Some(path) = pending.pop() {
        if path == checkout || path == backup || path == root.join(MARKER) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file() || metadata.is_dir() || metadata.is_symlink(),
            "unsupported entry in persistent updater state"
        );
        let relative = path.strip_prefix(root)?.to_owned();
        let kind = if metadata.is_symlink() {
            #[cfg(windows)]
            let directory = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x10 != 0
            };
            #[cfg(not(windows))]
            let directory = false;
            let target = fs::read_link(&path)?;
            if copy {
                let destination = backup.join(&relative);
                fs::create_dir_all(destination.parent().context("missing checkpoint parent")?)?;
                symlink(&target, &destination, directory)?;
            }
            Kind::Symlink(target, directory)
        } else if metadata.is_dir() {
            if copy {
                fs::create_dir_all(backup.join(&relative))?;
            }
            for child in fs::read_dir(&path)? {
                pending.push(child?.path());
            }
            Kind::Directory
        } else {
            let hash = digest(&fs::read(&path)?);
            if copy {
                let destination = backup.join(&relative);
                fs::create_dir_all(destination.parent().context("missing checkpoint parent")?)?;
                fs::copy(&path, &destination)?;
                File::open(destination)?.sync_all()?;
            }
            Kind::File(hash)
        };
        entries.insert(
            relative,
            Entry {
                permissions: metadata.permissions(),
                kind,
            },
        );
    }
    if copy {
        for (relative, entry) in entries.iter().rev() {
            if !matches!(entry.kind, Kind::Symlink(..)) {
                fs::set_permissions(backup.join(relative), entry.permissions.clone())?;
                if matches!(entry.kind, Kind::Directory) {
                    File::open(backup.join(relative))?.sync_all()?;
                }
            }
        }
    }
    Ok(entries)
}

fn clear(root: &Path, path: &Path, checkout: &Path, backup: &Path) -> Result<()> {
    if path == checkout || path == backup || path == root.join(MARKER) {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)?;
    // Preserve the live workspace-lock inode, but never follow its replacement.
    if path == root.join("lock") && metadata.is_file() {
        return Ok(());
    }
    if metadata.is_dir() {
        for child in fs::read_dir(path)? {
            clear(root, &child?.path(), checkout, backup)?;
        }
        if path != root && !checkout.starts_with(path) {
            fs::remove_dir(path)?;
        }
    } else {
        ensure!(
            !checkout.starts_with(path),
            "persistent updater state ancestor was redirected"
        );
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn protect<T>(
    root: &Path,
    checkout: &Path,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    ensure_idle(root)?;
    // ponytail: portable verification copies O(total state bytes) to disk. Native
    // child write confinement can replace the checkpoint for larger fleets.
    let backup = tempfile::Builder::new()
        .prefix("simit-generation-state-")
        .tempdir_in(root)?;
    let entries = capture(root, checkout, backup.path(), true)?;
    let held = match File::open(root.join("lock")) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let marker = root.join(MARKER);
    save(&marker, &serde_json::json!({"snapshot": backup.path()}))?;
    // Keep recovery bytes across interruption or an unsuccessful restoration.
    let backup = backup.keep();
    let output = operation();
    let check_lock = || -> Result<()> {
        if let Some(held) = &held {
            verify_lock(root, held).with_context(|| {
                format!(
                    "workspace lock changed; recovery snapshot retained at {}",
                    backup.display()
                )
            })?;
        }
        Ok(())
    };
    // Content equality cannot detect unlink/recreate of an empty lock file.
    // Never restore or continue without the original locked inode.
    check_lock()?;
    let unchanged = capture(root, checkout, &backup, false).is_ok_and(|current| current == entries);
    if !unchanged {
        let restore = || -> Result<()> {
            ensure!(
                fs::symlink_metadata(root)?.is_dir(),
                "persistent updater state root was redirected"
            );
            clear(root, root, checkout, &backup)?;
            for (relative, entry) in &entries {
                let path = root.join(relative);
                match &entry.kind {
                    Kind::File(hash) => {
                        let original = backup.join(relative);
                        ensure!(
                            digest(&fs::read(&original)?) == *hash,
                            "checkpoint contents changed"
                        );
                        fs::copy(original, &path)?;
                    }
                    Kind::Directory => fs::create_dir_all(&path)?,
                    Kind::Symlink(target, directory) => symlink(target, &path, *directory)?,
                }
            }
            for (relative, entry) in entries.iter().rev() {
                if !matches!(entry.kind, Kind::Symlink(..)) {
                    let path = root.join(relative);
                    fs::set_permissions(&path, entry.permissions.clone())?;
                    File::open(path)?.sync_all()?;
                }
            }
            ensure!(
                capture(root, checkout, &backup, false)? == entries,
                "restored persistent updater state differs from checkpoint"
            );
            Ok(())
        };
        restore().with_context(|| {
            format!(
                "persistent updater state restoration failed; recovery snapshot retained at {}",
                backup.display()
            )
        })?;
    }
    check_lock()?;
    fs::remove_file(marker)?;
    // A durable marker must never point at a deleted recovery snapshot.
    File::open(root)?.sync_all()?;
    fs::remove_dir_all(backup)?;
    File::open(root)?.sync_all()?;
    ensure!(
        unchanged,
        "read-only command modified persistent updater state; checkpoint restored"
    );
    output
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn interrupted_generation_keeps_recovery_inside_durable_workspace_state() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        fs::create_dir(&checkout).unwrap();
        fs::write(root.path().join("baseline.json"), "original baseline").unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            protect::<()>(root.path(), &checkout, || panic!("interrupted generation")).unwrap();
        }));
        assert!(result.is_err());
        assert!(ensure_idle(root.path()).is_err());
        let marker: serde_json::Value =
            crate::review::read_json(&root.path().join(MARKER)).unwrap();
        let snapshot = Path::new(marker["snapshot"].as_str().unwrap());
        assert_eq!(snapshot.parent(), Some(root.path()));
        assert_eq!(
            fs::read_to_string(snapshot.join("baseline.json")).unwrap(),
            "original baseline"
        );
        assert!(!snapshot.join("checkout").exists());
        assert_eq!(fs::read_dir(snapshot).unwrap().count(), 1);
    }
}

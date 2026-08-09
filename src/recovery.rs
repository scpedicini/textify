use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::file_io::save_atomic_chunks;

/// Writes a revision-specific crash-recovery copy. Revision-specific names prevent an older,
/// slower background write from replacing a newer snapshot.
pub fn write_snapshot<'a>(
    directory: &Path,
    key: u128,
    revision: u64,
    chunks: impl IntoIterator<Item = &'a str>,
) -> Result<PathBuf> {
    fs::create_dir_all(directory)
        .with_context(|| format!("could not create {}", directory.display()))?;
    let path = directory.join(format!("tab-{key:032x}-{revision:016x}.txt"));
    save_atomic_chunks(&path, chunks)?;
    Ok(path)
}

pub fn load_snapshot(path: &Path) -> Result<String> {
    let bytes = fs::read(path)
        .with_context(|| format!("could not read recovery copy {}", path.display()))?;
    String::from_utf8(bytes)
        .with_context(|| format!("recovery copy {} is not UTF-8", path.display()))
}

pub fn remove_snapshot(path: &Path) -> Result<()> {
    let managed_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let body = name
                .strip_prefix("tab-")
                .and_then(|name| name.strip_suffix(".txt"));
            body.is_some_and(|body| {
                let mut parts = body.split('-');
                matches!(
                    (parts.next(), parts.next(), parts.next()),
                    (Some(key), Some(revision), None)
                        if key.len() == 32
                            && revision.len() == 16
                            && key.chars().all(|character| character.is_ascii_hexdigit())
                            && revision.chars().all(|character| character.is_ascii_hexdigit())
                )
            })
        });
    anyhow::ensure!(
        managed_name,
        "refusing to remove an unmanaged recovery path"
    );
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("could not remove recovery copy {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_durable_utf8_and_revision_specific() {
        let directory = tempfile::tempdir().expect("directory");
        let first = write_snapshot(directory.path(), 7, 1, ["one", " 🦀"]).expect("first snapshot");
        let second = write_snapshot(directory.path(), 7, 2, ["two"]).expect("second snapshot");

        assert_ne!(first, second);
        assert_eq!(load_snapshot(&first).expect("load first"), "one 🦀");
        assert_eq!(load_snapshot(&second).expect("load second"), "two");
    }

    #[test]
    fn removing_a_snapshot_is_idempotent() {
        let directory = tempfile::tempdir().expect("directory");
        let path = write_snapshot(directory.path(), 9, 3, ["draft"]).expect("snapshot");
        remove_snapshot(&path).expect("remove");
        remove_snapshot(&path).expect("remove missing");
        assert!(!path.exists());
    }

    #[test]
    fn removal_rejects_unmanaged_paths() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("notes.txt");
        fs::write(&path, "keep me").expect("fixture");
        assert!(remove_snapshot(&path).is_err());
        assert!(path.exists());
    }
}

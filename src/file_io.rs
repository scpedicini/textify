use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::document::{DocumentMetadata, FileMode, FilePolicy};

#[derive(Debug, Clone)]
pub struct LoadedFile {
    pub text: String,
    pub metadata: DocumentMetadata,
}

pub fn load_utf8(path: &Path, policy: FilePolicy) -> Result<LoadedFile> {
    let file_metadata =
        fs::metadata(path).with_context(|| format!("could not inspect {}", path.display()))?;

    if !file_metadata.is_file() {
        bail!("{} is not a regular file", path.display());
    }

    if file_metadata.len() >= policy.huge_file_bytes {
        bail!(
            "{} is {:.1} MiB; Textify's paged huge-file viewer is not available yet",
            path.display(),
            file_metadata.len() as f64 / (1024.0 * 1024.0)
        );
    }

    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let analysis = crate::document::FileAnalysis::from_bytes(&bytes);
    let metadata = DocumentMetadata::new(Some(path.to_path_buf()), analysis, policy);
    debug_assert_ne!(metadata.mode, FileMode::HugeViewer);

    let text = String::from_utf8(bytes).with_context(|| {
        format!(
            "{} is not valid UTF-8; additional encodings are not supported yet",
            path.display()
        )
    })?;

    Ok(LoadedFile { text, metadata })
}

/// Writes to a temporary file beside the target, flushes it, then atomically replaces the target.
pub fn save_atomic(path: &Path, text: &str) -> Result<()> {
    save_atomic_chunks(path, std::iter::once(text))
}

/// Chunked form used by the rope-backed editor so saving does not build a second full string.
pub fn save_atomic_chunks<'a>(
    path: &Path,
    chunks: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;

    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create a temporary file in {}", parent.display()))?;
    for chunk in chunks {
        temporary
            .write_all(chunk.as_bytes())
            .with_context(|| format!("could not write temporary file for {}", path.display()))?;
    }
    temporary
        .flush()
        .with_context(|| format!("could not flush temporary file for {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("could not sync temporary file for {}", path.display()))?;

    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not replace {}", path.display()))?;
    sync_parent(parent)?;
    Ok(())
}

fn sync_parent(parent: &Path) -> Result<()> {
    match File::open(parent).and_then(|directory| directory.sync_all()) {
        Ok(()) => Ok(()),
        // Some filesystems do not allow syncing a directory. The file itself is already synced.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not sync {}", parent.display())),
    }
}

pub fn suggested_save_path(directory: &Path, untitled_number: usize) -> PathBuf {
    directory.join(format!("Untitled {untitled_number}.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{FileMode, Language, LineEnding};

    #[test]
    fn loads_utf8_and_builds_metadata() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("notes.md");
        fs::write(&path, "# Notes\r\n\r\nHello").expect("fixture");

        let loaded = load_utf8(&path, FilePolicy::default()).expect("load fixture");
        assert_eq!(loaded.text, "# Notes\r\n\r\nHello");
        assert_eq!(loaded.metadata.language, Language::Markdown);
        assert_eq!(loaded.metadata.mode, FileMode::Normal);
        assert_eq!(loaded.metadata.analysis.line_ending, LineEnding::CrLf);
    }

    #[test]
    fn atomic_save_replaces_existing_content() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("draft.txt");
        fs::write(&path, "old").expect("fixture");

        save_atomic(&path, "new text").expect("save");

        assert_eq!(fs::read_to_string(path).expect("read result"), "new text");
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("bytes.dat");
        fs::write(&path, [0xff, 0xfe]).expect("fixture");

        let error = load_utf8(&path, FilePolicy::default()).expect_err("must reject invalid UTF-8");
        assert!(error.to_string().contains("not valid UTF-8"));
    }
}

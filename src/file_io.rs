use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::Hasher,
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};

use crate::document::{DocumentMetadata, FileMode, FilePolicy, TextEncoding};

const CP437_HIGH_CHARS: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕',
    '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐',
    '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±',
    '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{00a0}',
];

#[derive(Debug, Clone)]
pub struct LoadedFile {
    pub text: String,
    pub metadata: DocumentMetadata,
    pub disk_revision: DiskRevision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskRevision {
    pub bytes: u64,
    pub modified_nanos: Option<u128>,
    pub content_hash: u64,
}

#[derive(Debug)]
pub struct ExternalFileChanged {
    pub path: PathBuf,
    pub expected: DiskRevision,
    pub actual: Option<DiskRevision>,
}

impl std::fmt::Display for ExternalFileChanged {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} changed on disk since it was opened",
            self.path.display()
        )
    }
}

impl std::error::Error for ExternalFileChanged {}

impl DiskRevision {
    fn from_bytes(metadata: &fs::Metadata, bytes: &[u8]) -> Self {
        let mut hasher = DefaultHasher::new();
        hasher.write(bytes);
        Self::from_hash(metadata, hasher.finish())
    }

    fn from_hash(metadata: &fs::Metadata, content_hash: u64) -> Self {
        Self {
            bytes: metadata.len(),
            modified_nanos: metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos()),
            content_hash,
        }
    }
}

pub fn load_text(path: &Path, policy: FilePolicy) -> Result<LoadedFile> {
    load_text_inner(path, policy, None)
}

pub fn load_text_as(path: &Path, policy: FilePolicy, encoding: TextEncoding) -> Result<LoadedFile> {
    load_text_inner(path, policy, Some(encoding))
}

fn load_text_inner(
    path: &Path,
    policy: FilePolicy,
    requested_encoding: Option<TextEncoding>,
) -> Result<LoadedFile> {
    let file_metadata =
        fs::metadata(path).with_context(|| format!("could not inspect {}", path.display()))?;

    if !file_metadata.is_file() {
        bail!("{} is not a regular file", path.display());
    }

    if file_metadata.len() >= policy.huge_file_bytes {
        bail!(
            "{} is {:.1} MiB and must be opened with Textify's paged huge-file viewer",
            path.display(),
            file_metadata.len() as f64 / (1024.0 * 1024.0)
        );
    }

    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    let file_metadata = fs::metadata(path)
        .with_context(|| format!("could not re-inspect {} after reading", path.display()))?;
    let disk_revision = DiskRevision::from_bytes(&file_metadata, &bytes);
    let analysis = crate::document::FileAnalysis::from_bytes(&bytes);
    let encoding = requested_encoding.unwrap_or_else(|| detect_encoding(&bytes));
    if requested_encoding.is_none()
        && encoding == TextEncoding::Cp437
        && !looks_like_cp437_text(&bytes)
    {
        bail!(
            "{} is neither valid UTF-8 nor recognizable CP437 text",
            path.display()
        );
    }
    let metadata =
        DocumentMetadata::new_with_encoding(Some(path.to_path_buf()), analysis, policy, encoding);
    debug_assert_ne!(metadata.mode, FileMode::HugeViewer);

    let text = decode_text(bytes, encoding).with_context(|| {
        format!(
            "could not decode {} as {}",
            path.display(),
            encoding.label()
        )
    })?;

    Ok(LoadedFile {
        text,
        metadata,
        disk_revision,
    })
}

fn detect_encoding(bytes: &[u8]) -> TextEncoding {
    if std::str::from_utf8(bytes).is_ok() {
        TextEncoding::Utf8
    } else {
        TextEncoding::Cp437
    }
}

fn looks_like_cp437_text(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    let suspicious_controls = bytes
        .iter()
        .filter(|&&byte| byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r' | 0x0c | 0x1a | 0x1b))
        .count();
    suspicious_controls.saturating_mul(100) <= bytes.len().max(1).saturating_mul(2)
}

fn decode_text(bytes: Vec<u8>, encoding: TextEncoding) -> Result<String> {
    match encoding {
        TextEncoding::Utf8 => String::from_utf8(bytes).context("invalid UTF-8"),
        TextEncoding::Cp437 => Ok(bytes
            .into_iter()
            .map(|byte| {
                if byte < 0x80 {
                    char::from(byte)
                } else {
                    CP437_HIGH_CHARS[usize::from(byte - 0x80)]
                }
            })
            .collect()),
    }
}

fn encode_cp437(character: char) -> Option<u8> {
    if character.is_ascii() {
        return Some(character as u8);
    }
    CP437_HIGH_CHARS
        .iter()
        .position(|&candidate| candidate == character)
        .map(|index| index as u8 + 0x80)
}

/// Reads a bounded-memory content fingerprint and filesystem metadata for conflict detection.
pub fn disk_revision(path: &Path) -> Result<DiskRevision> {
    for _ in 0..2 {
        let before =
            fs::metadata(path).with_context(|| format!("could not inspect {}", path.display()))?;
        let file =
            File::open(path).with_context(|| format!("could not open {}", path.display()))?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut buffer = [0_u8; 64 * 1024];
        let mut hasher = DefaultHasher::new();
        loop {
            let read = reader
                .read(&mut buffer)
                .with_context(|| format!("could not fingerprint {}", path.display()))?;
            if read == 0 {
                break;
            }
            hasher.write(&buffer[..read]);
        }
        let after = fs::metadata(path)
            .with_context(|| format!("could not re-inspect {}", path.display()))?;
        let before_revision = DiskRevision::from_hash(&before, hasher.finish());
        let after_revision = DiskRevision::from_hash(&after, before_revision.content_hash);
        if before_revision.bytes == after_revision.bytes
            && before_revision.modified_nanos == after_revision.modified_nanos
        {
            return Ok(after_revision);
        }
    }
    bail!(
        "{} kept changing while Textify inspected it",
        path.display()
    )
}

pub fn optional_disk_revision(path: &Path) -> Result<Option<DiskRevision>> {
    match disk_revision(path) {
        Ok(revision) => Ok(Some(revision)),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
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
    save_atomic_chunks_checked(path, chunks, None).map(|_| ())
}

/// Saves atomically after verifying that an opened file still matches its disk revision.
pub fn save_atomic_chunks_checked<'a>(
    path: &Path,
    chunks: impl IntoIterator<Item = &'a str>,
    expected: Option<&DiskRevision>,
) -> Result<DiskRevision> {
    save_atomic_with_writer(path, expected, |temporary| {
        for chunk in chunks {
            temporary.write_all(chunk.as_bytes()).with_context(|| {
                format!("could not write temporary file for {}", path.display())
            })?;
        }
        Ok(())
    })
}

pub fn save_atomic_encoded_chunks_checked<'a>(
    path: &Path,
    chunks: impl IntoIterator<Item = &'a str>,
    encoding: TextEncoding,
    expected: Option<&DiskRevision>,
) -> Result<DiskRevision> {
    match encoding {
        TextEncoding::Utf8 => save_atomic_chunks_checked(path, chunks, expected),
        TextEncoding::Cp437 => save_atomic_with_writer(path, expected, |temporary| {
            let mut buffer = Vec::with_capacity(8192);
            for chunk in chunks {
                for character in chunk.chars() {
                    let Some(byte) = encode_cp437(character) else {
                        bail!(
                            "character {character:?} cannot be represented in CP437; remove or replace it before saving"
                        );
                    };
                    buffer.push(byte);
                    if buffer.len() == buffer.capacity() {
                        temporary.write_all(&buffer).with_context(|| {
                            format!("could not write temporary file for {}", path.display())
                        })?;
                        buffer.clear();
                    }
                }
            }
            temporary.write_all(&buffer).with_context(|| {
                format!("could not write temporary file for {}", path.display())
            })?;
            Ok(())
        }),
    }
}

fn save_atomic_with_writer(
    path: &Path,
    expected: Option<&DiskRevision>,
    write_contents: impl FnOnce(&mut tempfile::NamedTempFile) -> Result<()>,
) -> Result<DiskRevision> {
    if let Some(expected) = expected {
        let actual = optional_disk_revision(path)?;
        if actual.as_ref() != Some(expected) {
            return Err(ExternalFileChanged {
                path: path.to_path_buf(),
                expected: expected.clone(),
                actual,
            }
            .into());
        }
    }

    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;

    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create a temporary file in {}", parent.display()))?;
    write_contents(&mut temporary)?;
    temporary
        .flush()
        .with_context(|| format!("could not flush temporary file for {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("could not sync temporary file for {}", path.display()))?;
    if let Some(permissions) = permissions {
        temporary
            .as_file()
            .set_permissions(permissions)
            .with_context(|| format!("could not preserve permissions for {}", path.display()))?;
    }

    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not replace {}", path.display()))?;
    sync_parent(parent)?;
    disk_revision(path)
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

        let loaded = load_text(&path, FilePolicy::default()).expect("load fixture");
        assert_eq!(loaded.text, "# Notes\r\n\r\nHello");
        assert_eq!(loaded.metadata.language, Language::Markdown);
        assert_eq!(loaded.metadata.mode, FileMode::Normal);
        assert_eq!(loaded.metadata.analysis.line_ending, LineEnding::CrLf);
        assert_eq!(loaded.disk_revision.bytes, loaded.text.len() as u64);
    }

    #[test]
    fn utf8_decode_reuses_the_loaded_byte_buffer() {
        let bytes = Vec::from("Textify keeps one file buffer");
        let allocation = bytes.as_ptr();

        let decoded = decode_text(bytes, TextEncoding::Utf8).expect("decode UTF-8");

        assert_eq!(decoded.as_ptr(), allocation);
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
    fn binary_data_is_not_misidentified_as_cp437_text() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("bytes.dat");
        fs::write(&path, [0x89, b'P', b'N', b'G', 0, 0xff]).expect("fixture");

        let error = load_text(&path, FilePolicy::default()).expect_err("must reject binary data");
        assert!(error.to_string().contains("recognizable CP437 text"));
    }

    #[test]
    fn cp437_is_detected_decoded_and_saved_without_conversion() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("dos.txt");
        let bytes = b"Caf\x82\r\nBox: \xda\xc4\xbf\r\n";
        fs::write(&path, bytes).expect("fixture");

        let loaded = load_text(&path, FilePolicy::default()).expect("load CP437");
        assert_eq!(loaded.metadata.encoding, TextEncoding::Cp437);
        assert_eq!(loaded.text, "Café\r\nBox: ┌─┐\r\n");
        assert_eq!(loaded.metadata.analysis.line_ending, LineEnding::CrLf);

        save_atomic_encoded_chunks_checked(
            &path,
            [&loaded.text[..]],
            loaded.metadata.encoding,
            Some(&loaded.disk_revision),
        )
        .expect("save CP437");
        assert_eq!(fs::read(path).expect("saved bytes"), bytes);
    }

    #[test]
    fn explicit_cp437_reopen_overrides_a_valid_utf8_guess() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("ambiguous.txt");
        fs::write(&path, [0xc2, 0xa3]).expect("fixture");

        let automatic = load_text(&path, FilePolicy::default()).expect("automatic load");
        assert_eq!(automatic.metadata.encoding, TextEncoding::Utf8);
        assert_eq!(automatic.text, "£");
        let cp437 = load_text_as(&path, FilePolicy::default(), TextEncoding::Cp437)
            .expect("explicit CP437 load");
        assert_eq!(cp437.metadata.encoding, TextEncoding::Cp437);
        assert_eq!(cp437.text, "┬ú");
    }

    #[test]
    fn cp437_save_rejects_unrepresentable_edits_without_replacing_the_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("dos.txt");
        fs::write(&path, b"original\r\n").expect("fixture");
        let expected = disk_revision(&path).expect("revision");

        let error = save_atomic_encoded_chunks_checked(
            &path,
            ["snowman ☃\r\n"],
            TextEncoding::Cp437,
            Some(&expected),
        )
        .expect_err("unrepresentable text must fail");
        assert!(error.to_string().contains("cannot be represented in CP437"));
        assert_eq!(fs::read(&path).expect("original bytes"), b"original\r\n");
    }

    #[test]
    fn checked_save_rejects_external_changes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("draft.txt");
        fs::write(&path, "opened").expect("fixture");
        let expected = disk_revision(&path).expect("revision");
        fs::write(&path, "external").expect("external edit");

        let error = save_atomic_chunks_checked(&path, ["mine"], Some(&expected))
            .expect_err("must detect conflict");

        assert!(error.downcast_ref::<ExternalFileChanged>().is_some());
        assert_eq!(fs::read_to_string(path).expect("disk text"), "external");
    }

    #[test]
    fn missing_file_has_no_optional_revision() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("missing.txt");
        assert_eq!(optional_disk_revision(&path).expect("revision"), None);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_save_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("script.sh");
        fs::write(&path, "old").expect("fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o744)).expect("permissions");

        save_atomic(&path, "new").expect("save");

        assert_eq!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
            0o744
        );
    }
}

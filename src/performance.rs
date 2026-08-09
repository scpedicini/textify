use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

use crate::{
    document::{FileMode, FilePolicy},
    file_io::{load_utf8, save_atomic_chunks},
    session::{SessionState, save_session},
};

const MIB: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorpusSpec {
    pub json_bytes: [usize; 3],
    pub line_count: usize,
    pub long_line_bytes: usize,
    pub tab_count: usize,
}

impl CorpusSpec {
    pub const fn production() -> Self {
        Self {
            json_bytes: [MIB, 25 * MIB, 100 * MIB],
            line_count: 200_000,
            long_line_bytes: 5 * MIB,
            tab_count: 100,
        }
    }

    #[cfg(test)]
    const fn tiny() -> Self {
        Self {
            json_bytes: [128, 256, 512],
            line_count: 20,
            long_line_bytes: 1_024,
            tab_count: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedCorpus {
    pub files: Vec<PathBuf>,
    pub session_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct FileMeasurement {
    pub name: String,
    pub bytes: u64,
    pub mode: FileMode,
    pub open: Duration,
    pub save: Duration,
}

pub fn peak_rss_bytes() -> Option<u64> {
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: `getrusage` initializes the provided `rusage` when it returns zero.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0 {
            // SAFETY: The successful call above initialized the value.
            let usage = unsafe { usage.assume_init() };
            #[cfg(target_os = "macos")]
            return Some(usage.ru_maxrss as u64);
            #[cfg(not(target_os = "macos"))]
            return Some((usage.ru_maxrss as u64) * 1024);
        }
    }
    None
}

pub fn generate_corpus(root: &Path, spec: CorpusSpec) -> Result<GeneratedCorpus> {
    fs::create_dir_all(root).with_context(|| format!("could not create {}", root.display()))?;
    let mut files = Vec::new();

    for bytes in spec.json_bytes {
        let path = root.join(format!("json-{}-bytes.json", bytes));
        write_sized_json(&path, bytes)?;
        files.push(path);
    }

    let lines_path = root.join(format!("{}-lines.txt", spec.line_count));
    let mut lines = BufWriter::new(File::create(&lines_path)?);
    for index in 0..spec.line_count {
        writeln!(lines, "{index:08} textify performance line")?;
    }
    lines.flush()?;
    files.push(lines_path);

    let long_line_path = root.join(format!("{}-byte-line.txt", spec.long_line_bytes));
    let mut long_line = BufWriter::new(File::create(&long_line_path)?);
    write_repeated(&mut long_line, b'x', spec.long_line_bytes)?;
    long_line.flush()?;
    files.push(long_line_path);

    let unicode_path = root.join("unicode-ime.txt");
    fs::write(
        &unicode_path,
        "ASCII\n中文输入法\n日本語入力\n👩🏽‍💻 family: 👨‍👩‍👧‍👦\ne\u{301} café\n",
    )?;
    files.push(unicode_path);

    let tabs_dir = root.join("tabs");
    fs::create_dir_all(&tabs_dir)?;
    let mut tab_paths = Vec::with_capacity(spec.tab_count);
    for index in 0..spec.tab_count {
        let path = tabs_dir.join(format!("tab-{index:03}.txt"));
        fs::write(&path, format!("Textify tab {index}\n"))?;
        tab_paths.push(path);
    }
    let session_path = root.join("100-tabs-session.json");
    save_session(&session_path, &SessionState::new(0, tab_paths))?;

    Ok(GeneratedCorpus {
        files,
        session_path,
    })
}

pub fn measure_corpus(corpus: &GeneratedCorpus) -> Result<Vec<FileMeasurement>> {
    let policy = FilePolicy::default();
    corpus
        .files
        .iter()
        .map(|path| {
            let open_started = Instant::now();
            let loaded = load_utf8(path, policy)?;
            let open = open_started.elapsed();

            let save_path = path.with_extension("textify-save.tmp");
            let save_started = Instant::now();
            save_atomic_chunks(&save_path, [loaded.text.as_str()])?;
            let save = save_started.elapsed();
            fs::remove_file(&save_path)
                .with_context(|| format!("could not remove {}", save_path.display()))?;

            Ok(FileMeasurement {
                name: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("fixture")
                    .to_owned(),
                bytes: loaded.metadata.analysis.bytes,
                mode: loaded.metadata.mode,
                open,
                save,
            })
        })
        .collect()
}

fn write_sized_json(path: &Path, bytes: usize) -> Result<()> {
    let bytes = bytes.max(3);
    let mut writer = BufWriter::new(File::create(path)?);
    writer.write_all(b"[")?;
    let body_bytes = bytes - 2;
    let elements = body_bytes.div_ceil(2);
    write_json_pairs(&mut writer, elements.saturating_sub(1))?;
    writer.write_all(b"0")?;
    write_repeated(&mut writer, b' ', body_bytes - (elements * 2 - 1))?;
    writer.write_all(b"]")?;
    writer.flush()?;
    Ok(())
}

fn write_json_pairs(writer: &mut impl Write, pair_count: usize) -> Result<()> {
    let mut block = [b'0'; 64 * 1024];
    for index in (1..block.len()).step_by(2) {
        block[index] = b',';
    }
    let mut remaining_bytes = pair_count * 2;
    while remaining_bytes > 0 {
        let length = remaining_bytes.min(block.len());
        writer.write_all(&block[..length])?;
        remaining_bytes -= length;
    }
    Ok(())
}

fn write_repeated(writer: &mut impl Write, byte: u8, count: usize) -> Result<()> {
    let block = [byte; 64 * 1024];
    let mut remaining = count;
    while remaining > 0 {
        let length = remaining.min(block.len());
        writer.write_all(&block[..length])?;
        remaining -= length;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Language;

    #[test]
    fn production_corpus_covers_research_thresholds() {
        let spec = CorpusSpec::production();
        assert_eq!(spec.json_bytes, [MIB, 25 * MIB, 100 * MIB]);
        assert_eq!(spec.line_count, 200_000);
        assert_eq!(spec.long_line_bytes, 5 * MIB);
        assert_eq!(spec.tab_count, 100);
    }

    #[test]
    fn generated_corpus_exercises_parser_and_large_file_policy() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let corpus = generate_corpus(directory.path(), CorpusSpec::tiny()).expect("corpus");
        assert_eq!(corpus.files.len(), 6);

        let json = load_utf8(&corpus.files[0], FilePolicy::default()).expect("json");
        assert_eq!(json.metadata.language, Language::Json);
        let long_line = load_utf8(&corpus.files[4], FilePolicy::default()).expect("line");
        assert_eq!(long_line.metadata.analysis.longest_line_bytes, 1_024);
        assert!(corpus.session_path.is_file());
    }
}

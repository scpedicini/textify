use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use gpui_component::{
    highlighter::{HighlightTheme, SyntaxHighlighter},
    input::Rope,
};

use crate::{
    document::{FileMode, FilePolicy},
    file_io::{load_text, save_atomic_chunks},
    session::{SessionState, save_session},
};

const MIB: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorpusSpec {
    pub json_bytes: [usize; 3],
    pub line_count: usize,
    pub long_line_bytes: usize,
    pub nested_json_line_bytes: usize,
    pub tab_count: usize,
}

impl CorpusSpec {
    pub const fn production() -> Self {
        Self {
            json_bytes: [MIB, 25 * MIB, 100 * MIB],
            line_count: 200_000,
            long_line_bytes: 5 * MIB,
            nested_json_line_bytes: 30_000,
            tab_count: 100,
        }
    }

    #[cfg(test)]
    const fn tiny() -> Self {
        Self {
            json_bytes: [128, 256, 512],
            line_count: 20,
            long_line_bytes: 1_024,
            nested_json_line_bytes: 2_000,
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
    pub syntax_parse: Option<Duration>,
    pub syntax_style_20x: Option<Duration>,
    pub syntax_style_count: Option<usize>,
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

    let nested_json_path = root.join(format!(
        "30-row-lorem-{}-byte-json-line.json",
        spec.nested_json_line_bytes
    ));
    write_long_line_json(&nested_json_path, 30, spec.nested_json_line_bytes)?;
    files.push(nested_json_path);

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
            let loaded = load_text(path, policy)?;
            let open = open_started.elapsed();

            let (syntax_parse, syntax_style_20x, syntax_style_count) =
                if loaded.metadata.analysis.longest_line_bytes <= 64 * 1024 {
                    if let Some(parser) = loaded.metadata.parser_name(policy) {
                        let rope = Rope::from(loaded.text.as_str());
                        let mut highlighter = SyntaxHighlighter::new(parser);
                        let parse_started = Instant::now();
                        highlighter.update(None, &rope);
                        let syntax_parse = parse_started.elapsed();
                        let theme = HighlightTheme::default_dark();
                        let style_started = Instant::now();
                        let mut style_count = 0;
                        for _ in 0..20 {
                            style_count = highlighter.styles(&(0..rope.len()), &theme).len();
                        }
                        (
                            Some(syntax_parse),
                            Some(style_started.elapsed()),
                            Some(style_count),
                        )
                    } else {
                        (None, None, None)
                    }
                } else {
                    (None, None, None)
                };

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
                syntax_parse,
                syntax_style_20x,
                syntax_style_count,
            })
        })
        .collect()
}

const LOREM_WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
];

#[derive(Default)]
struct LoremGenerator {
    cursor: usize,
}

impl LoremGenerator {
    fn fill(&mut self, bytes: usize) -> String {
        let mut output = String::with_capacity(bytes);
        while output.len() < bytes {
            if !output.is_empty() {
                output.push(' ');
            }
            let word = LOREM_WORDS[self.cursor % LOREM_WORDS.len()];
            self.cursor += 1;
            output.push_str(word);
        }
        output.truncate(bytes);
        output
    }
}

fn write_long_line_json(path: &Path, lines: usize, target_line_bytes: usize) -> Result<()> {
    let rows = lines.saturating_sub(2).max(1);
    let nested_row = rows / 2;
    let long_key = "k".repeat(50);
    let mut lorem = LoremGenerator::default();
    let mut output = String::from("{\n");

    for row in 0..rows {
        let line = if row == nested_row {
            let prefix = format!("  \"{long_key}\": ");
            let trailing_bytes = usize::from(row + 1 != rows);
            let value_bytes = target_line_bytes.saturating_sub(prefix.len() + trailing_bytes);
            format!("{prefix}{}", lorem_json_object(value_bytes, &mut lorem))
        } else {
            format!("  \"row_{row:02}\": \"{}\"", lorem.fill(48))
        };
        output.push_str(&line);
        output.push_str(if row + 1 == rows { "\n" } else { ",\n" });
    }
    output.push('}');
    fs::write(path, output).with_context(|| format!("could not write {}", path.display()))
}

fn lorem_json_object(target_bytes: usize, lorem: &mut LoremGenerator) -> String {
    const SUFFIX_OVERHEAD: usize = r#","remainder":""}"#.len();
    let target_bytes = target_bytes.max(2 + SUFFIX_OVERHEAD);
    let mut value = String::from("{");
    let mut item = 0usize;

    loop {
        let separator = if item == 0 { "" } else { "," };
        let entry = format!(
            "{separator}\"item_{item:04}\":{{\"text\":\"{}\",\"index\":{item},\"active\":true}}",
            lorem.fill(72)
        );
        if value.len() + entry.len() + SUFFIX_OVERHEAD > target_bytes {
            break;
        }
        value.push_str(&entry);
        item += 1;
    }

    let separator = if item == 0 { "" } else { "," };
    let remainder_overhead = separator.len() + r#""remainder":""}"#.len();
    let remainder_bytes = target_bytes.saturating_sub(value.len() + remainder_overhead);
    value.push_str(separator);
    value.push_str(r#""remainder":""#);
    value.push_str(&lorem.fill(remainder_bytes));
    value.push_str("\"}");
    value
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
        assert_eq!(spec.nested_json_line_bytes, 30_000);
        assert_eq!(spec.tab_count, 100);
    }

    #[test]
    fn generated_corpus_exercises_parser_and_large_file_policy() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let corpus = generate_corpus(directory.path(), CorpusSpec::tiny()).expect("corpus");
        assert_eq!(corpus.files.len(), 7);

        let json = load_text(&corpus.files[0], FilePolicy::default()).expect("json");
        assert_eq!(json.metadata.language, Language::Json);
        let long_line = load_text(&corpus.files[4], FilePolicy::default()).expect("line");
        assert_eq!(long_line.metadata.analysis.longest_line_bytes, 1_024);
        let nested_json = load_text(&corpus.files[5], FilePolicy::default()).expect("nested JSON");
        assert_eq!(nested_json.metadata.language, Language::Json);
        assert_eq!(nested_json.metadata.analysis.lines, 30);
        assert_eq!(nested_json.metadata.analysis.longest_line_bytes, 2_000);
        serde_json::from_str::<serde_json::Value>(&nested_json.text).expect("valid JSON fixture");
        assert!(corpus.session_path.is_file());
    }
}

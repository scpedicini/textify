use std::path::{Path, PathBuf};

/// The initial editor intentionally ships only the grammars we can continuously benchmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    PlainText,
    Json,
    Html,
    Markdown,
}

impl Language {
    pub fn detect(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::PlainText;
        };

        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        match extension.as_str() {
            "json" | "jsonc" => Self::Json,
            "html" | "htm" => Self::Html,
            "md" | "markdown" | "mdown" | "mkd" => Self::Markdown,
            _ => Self::PlainText,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::PlainText => "Plain Text",
            Self::Json => "JSON",
            Self::Html => "HTML",
            Self::Markdown => "Markdown",
        }
    }

    pub const fn parser_name(self) -> Option<&'static str> {
        match self {
            Self::PlainText => None,
            Self::Json => Some("json"),
            Self::Html => Some("html"),
            Self::Markdown => Some("markdown"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Normal,
    Large,
    HugeViewer,
}

impl FileMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Large => "LARGE FILE",
            Self::HugeViewer => "HUGE FILE VIEWER",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
    None,
    Mixed,
}

impl LineEnding {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::CrLf => "CRLF",
            Self::Cr => "CR",
            Self::None => "No EOL",
            Self::Mixed => "Mixed EOL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileAnalysis {
    pub bytes: u64,
    pub lines: usize,
    pub longest_line_bytes: usize,
    pub line_ending: LineEnding,
}

impl FileAnalysis {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::from_byte_chunks(std::iter::once(bytes))
    }

    /// Analyses rope chunks without flattening the entire document into a `String`.
    pub fn from_str_chunks<'a>(chunks: impl IntoIterator<Item = &'a str>) -> Self {
        Self::from_byte_chunks(chunks.into_iter().map(str::as_bytes))
    }

    fn from_byte_chunks<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut lf = 0usize;
        let mut crlf = 0usize;
        let mut cr = 0usize;
        let mut lines = 1usize;
        let mut longest_line_bytes = 0usize;
        let mut current_line_bytes = 0usize;
        let mut bytes = 0u64;
        let mut pending_cr = false;

        for chunk in chunks {
            for &byte in chunk {
                bytes += 1;

                if pending_cr {
                    pending_cr = false;
                    if byte == b'\n' {
                        crlf += 1;
                        lines += 1;
                        longest_line_bytes = longest_line_bytes.max(current_line_bytes);
                        current_line_bytes = 0;
                        continue;
                    }

                    cr += 1;
                    lines += 1;
                    longest_line_bytes = longest_line_bytes.max(current_line_bytes);
                    current_line_bytes = 0;
                }

                match byte {
                    b'\r' => pending_cr = true,
                    b'\n' => {
                        lf += 1;
                        lines += 1;
                        longest_line_bytes = longest_line_bytes.max(current_line_bytes);
                        current_line_bytes = 0;
                    }
                    _ => current_line_bytes += 1,
                }
            }
        }

        if pending_cr {
            cr += 1;
            lines += 1;
            longest_line_bytes = longest_line_bytes.max(current_line_bytes);
            current_line_bytes = 0;
        }

        longest_line_bytes = longest_line_bytes.max(current_line_bytes);
        if bytes == 0 {
            lines = 0;
        }

        let line_ending = match (lf > 0, crlf > 0, cr > 0) {
            (false, false, false) => LineEnding::None,
            (true, false, false) => LineEnding::Lf,
            (false, true, false) => LineEnding::CrLf,
            (false, false, true) => LineEnding::Cr,
            _ => LineEnding::Mixed,
        };

        Self {
            bytes,
            lines,
            longest_line_bytes,
            line_ending,
        }
    }
}

/*
The scanner above intentionally delays classifying `\r` until the next byte. Rope chunks may
split a CRLF pair, and treating each chunk independently would incorrectly report mixed endings.
*/

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePolicy {
    pub large_file_bytes: u64,
    pub huge_file_bytes: u64,
    pub large_file_lines: usize,
    pub long_line_bytes: usize,
    pub json_parser_bytes: u64,
}

impl Default for FilePolicy {
    fn default() -> Self {
        Self {
            large_file_bytes: 64 * 1024 * 1024,
            huge_file_bytes: 512 * 1024 * 1024,
            large_file_lines: 200_000,
            long_line_bytes: 1024 * 1024,
            json_parser_bytes: 24 * 1024 * 1024,
        }
    }
}

impl FilePolicy {
    pub fn mode_for(self, analysis: FileAnalysis) -> FileMode {
        if analysis.bytes >= self.huge_file_bytes {
            FileMode::HugeViewer
        } else if analysis.bytes > self.large_file_bytes
            || analysis.lines > self.large_file_lines
            || analysis.longest_line_bytes > self.long_line_bytes
        {
            FileMode::Large
        } else {
            FileMode::Normal
        }
    }

    /// Returns a grammar only when parsing is inside Textify's explicit budget.
    pub fn parser_for(self, language: Language, analysis: FileAnalysis) -> Option<&'static str> {
        if self.mode_for(analysis) != FileMode::Normal {
            return None;
        }

        if language == Language::Json && analysis.bytes > self.json_parser_bytes {
            return None;
        }

        language.parser_name()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentMetadata {
    pub path: Option<PathBuf>,
    pub language: Language,
    pub mode: FileMode,
    pub analysis: FileAnalysis,
}

impl DocumentMetadata {
    pub fn new(path: Option<PathBuf>, analysis: FileAnalysis, policy: FilePolicy) -> Self {
        let language = Language::detect(path.as_deref());
        let mode = policy.mode_for(analysis);
        Self {
            path,
            language,
            mode,
            analysis,
        }
    }

    pub fn display_name(&self, untitled_number: usize) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("Untitled {untitled_number}"))
    }

    pub fn parser_name(&self, policy: FilePolicy) -> Option<&'static str> {
        policy.parser_for(self.language, self.analysis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_extensions_select_a_language() {
        assert_eq!(
            Language::detect(Some(Path::new("data.json"))),
            Language::Json
        );
        assert_eq!(
            Language::detect(Some(Path::new("README.MD"))),
            Language::Markdown
        );
        assert_eq!(
            Language::detect(Some(Path::new("index.HTML"))),
            Language::Html
        );
        assert_eq!(Language::Html.parser_name(), Some("html"));
        assert_eq!(
            Language::detect(Some(Path::new("unknown.payload"))),
            Language::PlainText
        );
        assert_eq!(Language::detect(None), Language::PlainText);
    }

    #[test]
    fn analyses_line_endings_and_longest_line() {
        let analysis = FileAnalysis::from_bytes(b"one\r\ntwo\nthree");
        assert_eq!(analysis.lines, 3);
        assert_eq!(analysis.longest_line_bytes, 5);
        assert_eq!(analysis.line_ending, LineEnding::Mixed);
    }

    #[test]
    fn recognises_crlf_split_across_rope_chunks() {
        let analysis = FileAnalysis::from_str_chunks(["one\r", "\ntwo"]);
        assert_eq!(analysis.bytes, 8);
        assert_eq!(analysis.lines, 2);
        assert_eq!(analysis.longest_line_bytes, 3);
        assert_eq!(analysis.line_ending, LineEnding::CrLf);
    }

    #[test]
    fn empty_document_has_no_lines_or_line_ending() {
        let analysis = FileAnalysis::from_bytes(b"");
        assert_eq!(analysis.lines, 0);
        assert_eq!(analysis.longest_line_bytes, 0);
        assert_eq!(analysis.line_ending, LineEnding::None);
    }

    #[test]
    fn large_files_never_get_a_parser() {
        let policy = FilePolicy::default();
        let analysis = FileAnalysis {
            bytes: policy.large_file_bytes + 1,
            lines: 1,
            longest_line_bytes: 20,
            line_ending: LineEnding::None,
        };

        assert_eq!(policy.mode_for(analysis), FileMode::Large);
        assert_eq!(policy.parser_for(Language::Json, analysis), None);
    }

    #[test]
    fn json_has_a_stricter_parser_budget() {
        let policy = FilePolicy::default();
        let analysis = FileAnalysis {
            bytes: policy.json_parser_bytes + 1,
            lines: 10,
            longest_line_bytes: 100,
            line_ending: LineEnding::Lf,
        };

        assert_eq!(policy.mode_for(analysis), FileMode::Normal);
        assert_eq!(policy.parser_for(Language::Json, analysis), None);
        assert_eq!(
            policy.parser_for(Language::Markdown, analysis),
            Some("markdown")
        );
        assert_eq!(policy.parser_for(Language::Html, analysis), Some("html"));
    }
}

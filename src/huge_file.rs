use std::{
    fs::{self, File},
    ops::Range,
    os::unix::fs::FileExt as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, bail};

const READ_BUFFER_BYTES: usize = 1024 * 1024;
pub const VIEW_PAGE_BYTES: usize = 256 * 1024;
pub const VIEW_PAGE_LINES: usize = 512;
pub const MAX_COPY_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_EDIT_RANGE_BYTES: u64 = 64 * 1024 * 1024;
const LINE_INDEX_STRIDE: u64 = 4096;

#[derive(Debug, Clone)]
pub struct HugeFile {
    path: PathBuf,
    file: Arc<File>,
    len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLine {
    pub byte_range: Range<u64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPage {
    pub byte_range: Range<u64>,
    pub lines: Vec<PageLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    checkpoints: Vec<LineCheckpoint>,
    total_lines: u64,
    file_len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineCheckpoint {
    line: u64,
    byte: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub byte_range: Range<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchSummary {
    pub matches: usize,
    pub completed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl HugeFile {
    pub fn open(path: &Path) -> Result<Self> {
        let metadata =
            fs::metadata(path).with_context(|| format!("could not inspect {}", path.display()))?;
        if !metadata.is_file() {
            bail!("{} is not a regular file", path.display());
        }
        let file =
            File::open(path).with_context(|| format!("could not open {}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Arc::new(file),
            len: metadata.len(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn read_page(&self, offset: u64) -> Result<TextPage> {
        let offset = offset.min(self.len);
        let raw = self.read_bytes(offset, VIEW_PAGE_BYTES)?;
        let (leading, valid) = valid_utf8_window(&raw, offset > 0)?;
        let page_start = offset + leading as u64;
        let mut consumed = 0usize;
        let mut lines = Vec::new();

        for part in valid.split_inclusive('\n').take(VIEW_PAGE_LINES) {
            let line_start = page_start + consumed as u64;
            consumed += part.len();
            let text = part.strip_suffix('\n').unwrap_or(part);
            let text = text.strip_suffix('\r').unwrap_or(text).to_owned();
            lines.push(PageLine {
                byte_range: line_start..page_start + consumed as u64,
                text,
            });
        }

        if !valid.is_empty() && lines.is_empty() {
            consumed = valid.len();
            lines.push(PageLine {
                byte_range: page_start..page_start + consumed as u64,
                text: valid.to_owned(),
            });
        } else if consumed == 0 {
            consumed = valid.len();
        }

        Ok(TextPage {
            byte_range: page_start..page_start + consumed as u64,
            lines,
        })
    }

    pub fn read_utf8_range(&self, range: Range<u64>, max_bytes: u64) -> Result<String> {
        if range.start > range.end || range.end > self.len {
            bail!("byte range is outside {}", self.path.display());
        }
        let length = range.end - range.start;
        if length > max_bytes {
            bail!(
                "selected range is {:.1} MiB; the limit is {:.1} MiB",
                length as f64 / (1024.0 * 1024.0),
                max_bytes as f64 / (1024.0 * 1024.0)
            );
        }
        let raw = self.read_bytes(range.start, length as usize)?;
        let (leading, valid) = valid_utf8_window(&raw, range.start > 0)?;
        if leading > 0 {
            tracing::debug!(leading, "trimmed partial UTF-8 prefix from selected range");
        }
        Ok(valid.to_owned())
    }

    pub fn build_line_index(&self, cancel: &CancellationToken) -> Result<Option<LineIndex>> {
        let mut checkpoints = vec![LineCheckpoint { line: 1, byte: 0 }];
        let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
        let mut offset = 0u64;
        let mut line: u64 = if self.len == 0 { 0 } else { 1 };

        while offset < self.len {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            let read = read_at(&self.file, &mut buffer, offset)?;
            if read == 0 {
                break;
            }
            for (index, byte) in buffer[..read].iter().enumerate() {
                if *byte == b'\n' {
                    line += 1;
                    if (line - 1).is_multiple_of(LINE_INDEX_STRIDE) {
                        checkpoints.push(LineCheckpoint {
                            line,
                            byte: offset + index as u64 + 1,
                        });
                    }
                }
            }
            offset += read as u64;
        }

        Ok(Some(LineIndex {
            checkpoints,
            total_lines: line,
            file_len: self.len,
        }))
    }

    pub fn byte_for_line(&self, index: &LineIndex, target_line: u64) -> Result<Option<u64>> {
        self.byte_for_line_cancellable(index, target_line, &CancellationToken::default())
    }

    pub fn byte_for_line_cancellable(
        &self,
        index: &LineIndex,
        target_line: u64,
        cancel: &CancellationToken,
    ) -> Result<Option<u64>> {
        if target_line == 0 || target_line > index.total_lines {
            return Ok(None);
        }
        let checkpoint = index.checkpoint_for_line(target_line);
        if checkpoint.line == target_line {
            return Ok(Some(checkpoint.byte));
        }
        let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
        let mut offset = checkpoint.byte;
        let mut line = checkpoint.line;
        while offset < self.len {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            let read = read_at(&self.file, &mut buffer, offset)?;
            if read == 0 {
                break;
            }
            for (byte_index, byte) in buffer[..read].iter().enumerate() {
                if *byte == b'\n' {
                    line += 1;
                    if line == target_line {
                        return Ok(Some(offset + byte_index as u64 + 1));
                    }
                }
            }
            offset += read as u64;
        }
        Ok(None)
    }

    pub fn line_for_byte(&self, index: &LineIndex, target_byte: u64) -> Result<Option<u64>> {
        if target_byte > index.file_len || index.total_lines == 0 {
            return Ok(None);
        }
        let checkpoint = index.checkpoint_for_byte(target_byte);
        let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
        let mut offset = checkpoint.byte;
        let mut line = checkpoint.line;
        while offset < target_byte {
            let capacity = (target_byte - offset).min(buffer.len() as u64) as usize;
            let read = read_at(&self.file, &mut buffer[..capacity], offset)?;
            if read == 0 {
                break;
            }
            line += buffer[..read].iter().filter(|byte| **byte == b'\n').count() as u64;
            offset += read as u64;
        }
        Ok(Some(line))
    }

    pub fn stream_find(
        &self,
        query: &str,
        start_byte: u64,
        cancel: &CancellationToken,
        mut on_match: impl FnMut(SearchMatch) -> bool,
    ) -> Result<SearchSummary> {
        if query.is_empty() {
            bail!("search query cannot be empty");
        }
        let needle = query.as_bytes();
        let mut buffer = vec![0_u8; READ_BUFFER_BYTES + needle.len().saturating_sub(1)];
        let mut overlap = 0usize;
        let mut offset = start_byte.min(self.len);
        let mut matches = 0usize;

        while offset < self.len {
            if cancel.is_cancelled() {
                return Ok(SearchSummary {
                    matches,
                    completed: false,
                });
            }
            let read = read_at(&self.file, &mut buffer[overlap..], offset)?;
            if read == 0 {
                break;
            }
            let haystack_len = overlap + read;
            let base = offset.saturating_sub(overlap as u64);
            let mut local = 0usize;
            while let Some(found) = find_subslice(&buffer[local..haystack_len], needle) {
                let start = local + found;
                let end = start + needle.len();
                let global_start = base + start as u64;
                let global_end = base + end as u64;
                if global_start >= start_byte && (offset == start_byte || global_end > offset) {
                    matches += 1;
                    if !on_match(SearchMatch {
                        byte_range: global_start..global_end,
                    }) {
                        return Ok(SearchSummary {
                            matches,
                            completed: false,
                        });
                    }
                }
                local = end.max(local + 1);
            }

            offset += read as u64;
            overlap = needle.len().saturating_sub(1).min(haystack_len);
            buffer.copy_within(haystack_len - overlap..haystack_len, 0);
        }

        Ok(SearchSummary {
            matches,
            completed: true,
        })
    }

    fn read_bytes(&self, offset: u64, max_bytes: usize) -> Result<Vec<u8>> {
        let available = self.len.saturating_sub(offset).min(max_bytes as u64) as usize;
        let mut bytes = vec![0_u8; available];
        let mut read_total = 0usize;
        while read_total < bytes.len() {
            let read = read_at(
                &self.file,
                &mut bytes[read_total..],
                offset + read_total as u64,
            )?;
            if read == 0 {
                break;
            }
            read_total += read;
        }
        bytes.truncate(read_total);
        Ok(bytes)
    }
}

impl LineIndex {
    pub fn total_lines(&self) -> u64 {
        self.total_lines
    }

    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    fn checkpoint_for_line(&self, line: u64) -> LineCheckpoint {
        let index = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.line <= line)
            .saturating_sub(1);
        self.checkpoints[index]
    }

    fn checkpoint_for_byte(&self, byte: u64) -> LineCheckpoint {
        let index = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.byte <= byte)
            .saturating_sub(1);
        self.checkpoints[index]
    }
}

fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> Result<usize> {
    file.read_at(buffer, offset)
        .with_context(|| format!("could not read huge file at byte {offset}"))
}

fn valid_utf8_window(bytes: &[u8], allow_partial_prefix: bool) -> Result<(usize, &str)> {
    let leading = if allow_partial_prefix {
        bytes
            .iter()
            .take(3)
            .take_while(|byte| **byte & 0b1100_0000 == 0b1000_0000)
            .count()
    } else {
        0
    };
    let candidate = &bytes[leading..];
    match std::str::from_utf8(candidate) {
        Ok(text) => Ok((leading, text)),
        Err(error) if error.error_len().is_none() => {
            let valid = &candidate[..error.valid_up_to()];
            Ok((
                leading,
                std::str::from_utf8(valid).expect("validated UTF-8 prefix"),
            ))
        }
        Err(error) => bail!(
            "huge file contains invalid UTF-8 near byte {}",
            leading + error.valid_up_to()
        ),
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(text: &str) -> (tempfile::TempDir, HugeFile) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("huge.log");
        fs::write(&path, text).expect("fixture");
        let file = HugeFile::open(&path).expect("open");
        (directory, file)
    }

    #[test]
    fn pages_have_global_byte_ranges_and_bounded_lines() {
        let (_directory, file) = fixture("one\ntwo\nthree\n");
        let page = file.read_page(0).expect("page");
        assert_eq!(page.byte_range, 0..14);
        assert_eq!(page.lines[1].byte_range, 4..8);
        assert_eq!(page.lines[1].text, "two");
        assert!(page.lines.len() <= VIEW_PAGE_LINES);
    }

    #[test]
    fn sparse_index_supports_line_and_byte_navigation() {
        let text = (0..10_000)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let (_directory, file) = fixture(&text);
        let index = file
            .build_line_index(&CancellationToken::default())
            .expect("index")
            .expect("not cancelled");

        let byte = file
            .byte_for_line(&index, 9_000)
            .expect("lookup")
            .expect("line");
        assert_eq!(
            file.line_for_byte(&index, byte).expect("lookup"),
            Some(9_000)
        );
        assert_eq!(index.total_lines(), 10_001);
        assert!(index.checkpoint_count() <= 4);
    }

    #[test]
    fn streaming_search_finds_cross_buffer_matches_and_cancels() {
        let mut text = "x".repeat(READ_BUFFER_BYTES - 2);
        text.push_str("Textify");
        text.push_str(&"x".repeat(32));
        let (_directory, file) = fixture(&text);
        let mut found = Vec::new();
        let summary = file
            .stream_find("Textify", 0, &CancellationToken::default(), |item| {
                found.push(item.byte_range);
                true
            })
            .expect("search");
        assert_eq!(
            found,
            vec![(READ_BUFFER_BYTES as u64 - 2)..(READ_BUFFER_BYTES as u64 + 5)]
        );
        assert!(summary.completed);

        let cancelled = CancellationToken::default();
        cancelled.cancel();
        let summary = file
            .stream_find("x", 0, &cancelled, |_| true)
            .expect("cancelled search");
        assert!(!summary.completed);
    }

    #[test]
    fn copy_and_edit_ranges_are_bounded_and_utf8_safe() {
        let (_directory, file) = fixture("a💝b");
        assert_eq!(
            file.read_utf8_range(1..5, MAX_COPY_BYTES).expect("copy"),
            "💝"
        );
        assert!(file.read_utf8_range(0..file.len(), 2).is_err());
    }

    #[test]
    fn line_index_build_is_cancellable() {
        let (_directory, file) = fixture("one\ntwo\n");
        let token = CancellationToken::default();
        token.cancel();
        assert_eq!(file.build_line_index(&token).expect("index"), None);
    }
}

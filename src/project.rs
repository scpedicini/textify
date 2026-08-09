use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result};

use crate::huge_file::CancellationToken;

const IGNORED_DIRECTORIES: &[&str] = &[".git", "target", "node_modules", ".textify"];
const SUPPORTED_TEXT_EXTENSIONS: &[&str] = &[
    "bash",
    "c",
    "cc",
    "cfg",
    "cjs",
    "conf",
    "cpp",
    "css",
    "csv",
    "cs",
    "cxx",
    "dart",
    "ejs",
    "env",
    "erl",
    "ex",
    "exs",
    "fish",
    "fs",
    "fsx",
    "go",
    "gradle",
    "h",
    "hpp",
    "hrl",
    "htm",
    "html",
    "hxx",
    "ini",
    "java",
    "js",
    "json",
    "jsonc",
    "jsx",
    "kt",
    "kts",
    "lock",
    "log",
    "lua",
    "m",
    "markdown",
    "md",
    "mdown",
    "mkd",
    "mjs",
    "mm",
    "php",
    "properties",
    "py",
    "pyi",
    "r",
    "rb",
    "rs",
    "sh",
    "sql",
    "svelte",
    "swift",
    "text",
    "toml",
    "ts",
    "tsv",
    "tsx",
    "txt",
    "vue",
    "xml",
    "yaml",
    "yml",
    "zsh",
];
const SUPPORTED_TEXT_FILENAMES: &[&str] = &[
    ".bashrc",
    ".editorconfig",
    ".env",
    ".gitattributes",
    ".gitignore",
    ".profile",
    ".zshrc",
    "changelog",
    "dockerfile",
    "gemfile",
    "license",
    "makefile",
    "procfile",
    "readme",
];

fn is_supported_text_file(path: &Path) -> bool {
    let extension_supported = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_TEXT_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        });
    if extension_supported {
        return true;
    }

    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            SUPPORTED_TEXT_FILENAMES
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(name))
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub depth: usize,
    pub is_directory: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectIndex {
    pub root: PathBuf,
    pub entries: Vec<ProjectEntry>,
    pub files: Vec<PathBuf>,
    pub truncated: bool,
}

impl ProjectIndex {
    pub fn build(root: &Path, max_entries: usize) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("could not open folder {}", root.display()))?;
        let mut entries = Vec::new();
        let mut files = Vec::new();
        let mut pending = vec![(root.clone(), 0usize)];
        let mut truncated = false;

        while let Some((directory, depth)) = pending.pop() {
            let read_directory = match fs::read_dir(&directory) {
                Ok(read_directory) => read_directory,
                Err(error) if directory == root => {
                    return Err(error)
                        .with_context(|| format!("could not read {}", directory.display()));
                }
                Err(_) => continue,
            };
            let mut children = read_directory.filter_map(Result::ok).collect::<Vec<_>>();
            children.sort_by_key(|entry| {
                let directory_first = entry.file_type().map(|kind| !kind.is_dir()).unwrap_or(true);
                (
                    directory_first,
                    entry.file_name().to_string_lossy().to_lowercase(),
                )
            });

            let mut child_directories = Vec::new();
            for child in children {
                let name = child.file_name();
                let name = name.to_string_lossy();
                let file_type = match child.file_type() {
                    Ok(file_type) => file_type,
                    Err(_) => continue,
                };
                if file_type.is_symlink()
                    || (file_type.is_dir() && IGNORED_DIRECTORIES.contains(&name.as_ref()))
                {
                    continue;
                }
                let path = child.path();
                let supported = file_type.is_dir()
                    || (file_type.is_file() && is_supported_text_file(path.as_path()));
                if !supported {
                    continue;
                }
                if entries.len() >= max_entries {
                    truncated = true;
                    break;
                }
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(path.as_path())
                    .to_path_buf();
                entries.push(ProjectEntry {
                    path: path.clone(),
                    relative,
                    depth,
                    is_directory: file_type.is_dir(),
                });
                if file_type.is_dir() {
                    child_directories.push(path);
                } else if file_type.is_file() {
                    files.push(path);
                }
            }
            if truncated {
                break;
            }
            for child in child_directories.into_iter().rev() {
                pending.push((child, depth + 1));
            }
        }

        Ok(Self {
            root,
            entries,
            files,
            truncated,
        })
    }

    pub fn quick_open(&self, query: &str, limit: usize) -> Vec<PathBuf> {
        let mut matches = self
            .files
            .iter()
            .filter_map(|path| {
                let relative = path.strip_prefix(&self.root).unwrap_or(path);
                fuzzy_score(&relative.to_string_lossy(), query).map(|score| (score, path.clone()))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.as_os_str().len().cmp(&right.as_os_str().len()))
                .then_with(|| left.cmp(right))
        });
        matches
            .into_iter()
            .take(limit)
            .map(|(_, path)| path)
            .collect()
    }
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i64> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let candidate = candidate.to_lowercase();
    let mut score = 0i64;
    let mut query_chars = query.chars();
    let mut wanted = query_chars.next()?;
    let mut previous_match = None;
    for (index, character) in candidate.char_indices() {
        if character != wanted {
            continue;
        }
        score += 10;
        if previous_match.is_some_and(|previous| previous + character.len_utf8() == index) {
            score += 8;
        }
        if index == 0
            || candidate[..index]
                .chars()
                .last()
                .is_some_and(|previous| matches!(previous, '/' | '_' | '-' | '.'))
        {
            score += 12;
        }
        previous_match = Some(index);
        let Some(next) = query_chars.next() else {
            return Some(score - candidate.len() as i64 / 8);
        };
        wanted = next;
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchSummary {
    pub files_scanned: usize,
    pub matches: usize,
    pub completed: bool,
}

pub fn stream_workspace_search(
    files: &[PathBuf],
    query: &str,
    max_file_bytes: u64,
    max_matches: usize,
    cancel: &CancellationToken,
    mut on_match: impl FnMut(WorkspaceMatch) -> bool,
) -> SearchSummary {
    if query.is_empty() {
        return SearchSummary {
            files_scanned: 0,
            matches: 0,
            completed: true,
        };
    }
    let query_lower = query.to_ascii_lowercase();
    let mut summary = SearchSummary {
        files_scanned: 0,
        matches: 0,
        completed: true,
    };
    if cancel.is_cancelled() {
        summary.completed = false;
        return summary;
    }

    for path in files {
        if cancel.is_cancelled() {
            summary.completed = false;
            break;
        }
        let Ok(metadata) = fs::metadata(path) else {
            continue;
        };
        if metadata.len() > max_file_bytes {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        summary.files_scanned += 1;
        if bytes.iter().take(8 * 1024).any(|byte| *byte == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (line_index, line) in text.lines().enumerate() {
            if cancel.is_cancelled() {
                summary.completed = false;
                return summary;
            }
            let line_lower = line.to_ascii_lowercase();
            let mut search_from = 0usize;
            while let Some(found) = line_lower[search_from..].find(&query_lower) {
                let byte_column = search_from + found;
                let column = line[..byte_column].chars().count();
                let preview = line.chars().take(240).collect::<String>();
                summary.matches += 1;
                if !on_match(WorkspaceMatch {
                    path: path.clone(),
                    line: line_index,
                    column,
                    preview,
                }) || summary.matches >= max_matches
                {
                    summary.completed = summary.matches < max_matches;
                    return summary;
                }
                search_from = byte_column + query_lower.len();
            }
        }
    }
    summary
}

pub fn load_git_status(root: &Path) -> Result<HashMap<PathBuf, String>> {
    if !root.join(".git").exists() {
        return Ok(HashMap::new());
    }
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=normal"])
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .context("could not run git status")?;
    if !output.status.success() {
        anyhow::bail!("git status exited with {}", output.status);
    }
    Ok(parse_git_status(&output.stdout))
}

fn parse_git_status(output: &[u8]) -> HashMap<PathBuf, String> {
    let mut statuses = HashMap::new();
    let mut items = output
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty());
    while let Some(item) = items.next() {
        if item.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&item[..2]).to_string();
        let path = PathBuf::from(String::from_utf8_lossy(&item[3..]).into_owned());
        statuses.insert(path, status.clone());
        if (status.contains('R') || status.contains('C'))
            && let Some(destination) = items.next()
        {
            statuses.insert(
                PathBuf::from(String::from_utf8_lossy(destination).into_owned()),
                status,
            );
        }
    }
    statuses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_index_ignores_heavy_directories_and_quick_opens_fuzzily() {
        let directory = tempfile::tempdir().expect("directory");
        fs::create_dir_all(directory.path().join("src/nested")).expect("src");
        fs::create_dir_all(directory.path().join("target/debug")).expect("target");
        fs::write(directory.path().join("src/main.rs"), "fn main() {}\n").expect("main");
        fs::write(
            directory.path().join("src/nested/model.rs"),
            "struct Model;\n",
        )
        .expect("model");
        fs::write(directory.path().join("target/debug/binary"), "ignored").expect("ignored");
        fs::write(directory.path().join(".DS_Store"), "metadata").expect("finder metadata");
        fs::write(directory.path().join("cover.png"), b"not really a png").expect("image");
        fs::write(directory.path().join("bundle.zip"), b"not really a zip").expect("archive");
        fs::write(directory.path().join("README"), "project notes\n").expect("readme");

        let index = ProjectIndex::build(directory.path(), 100).expect("index");
        assert_eq!(index.files.len(), 3);
        assert!(
            index
                .files
                .iter()
                .all(|path| !path.to_string_lossy().contains("target"))
        );
        assert!(index.files.iter().any(|path| path.ends_with("README")));
        assert!(index.files.iter().all(|path| {
            !matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".DS_Store" | "cover.png" | "bundle.zip")
            )
        }));
        assert!(index.quick_open("smod", 10)[0].ends_with("model.rs"));
    }

    #[test]
    fn explorer_supports_known_text_and_source_files_only() {
        for path in [
            "notes.txt",
            "data.JSON",
            "page.html",
            "README",
            ".gitignore",
            ".bashrc",
            ".zshrc",
            "src/main.rs",
        ] {
            assert!(is_supported_text_file(Path::new(path)), "{path}");
        }
        for path in ["photo.jpg", "sound.mp3", "archive.zip", ".DS_Store"] {
            assert!(!is_supported_text_file(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn workspace_search_streams_results_and_honors_cancellation() {
        let directory = tempfile::tempdir().expect("directory");
        let first = directory.path().join("first.txt");
        let second = directory.path().join("second.txt");
        fs::write(&first, "needle one\nnope\nneedle two\n").expect("first");
        fs::write(&second, "needle three\n").expect("second");
        let cancel = CancellationToken::default();
        let mut found = Vec::new();
        let summary =
            stream_workspace_search(&[first, second], "needle", 1024, 10, &cancel, |item| {
                found.push(item);
                true
            });
        assert_eq!(summary.matches, 3);
        assert_eq!(found[1].line, 2);

        cancel.cancel();
        let cancelled = stream_workspace_search(&[], "needle", 1024, 10, &cancel, |_| true);
        assert!(!cancelled.completed);
    }

    #[test]
    fn git_porcelain_parser_handles_modified_untracked_and_renamed_paths() {
        let statuses = parse_git_status(b" M src/lib.rs\0?? notes.txt\0R  old.rs\0new.rs\0");
        assert_eq!(statuses[Path::new("src/lib.rs")], " M");
        assert_eq!(statuses[Path::new("notes.txt")], "??");
        assert_eq!(statuses[Path::new("new.rs")], "R ");
    }
}

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::file_io::save_atomic;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentFiles {
    pub paths: Vec<PathBuf>,
}

impl RecentFiles {
    pub fn load(path: &Path, limit: usize) -> Result<Self> {
        let mut history = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("could not parse {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", path.display()));
            }
        };
        history.normalize(limit);
        Ok(history)
    }

    pub fn record(&mut self, path: PathBuf, limit: usize) {
        self.paths.retain(|existing| existing != &path);
        if limit == 0 {
            self.paths.clear();
            return;
        }
        self.paths.insert(0, path);
        self.paths.truncate(limit);
    }

    pub fn normalize(&mut self, limit: usize) {
        if limit == 0 {
            self.paths.clear();
            return;
        }
        let mut unique = Vec::with_capacity(self.paths.len().min(limit));
        for path in self.paths.drain(..) {
            if !unique.contains(&path) {
                unique.push(path);
            }
            if unique.len() == limit {
                break;
            }
        }
        self.paths = unique;
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json =
            serde_json::to_string_pretty(self).context("could not serialize recent files")?;
        save_atomic(path, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_are_newest_first_unique_and_bounded() {
        let mut history = RecentFiles::default();
        history.record(PathBuf::from("one.txt"), 3);
        history.record(PathBuf::from("two.txt"), 3);
        history.record(PathBuf::from("three.txt"), 3);
        history.record(PathBuf::from("one.txt"), 3);
        history.record(PathBuf::from("four.txt"), 3);

        assert_eq!(
            history.paths,
            ["four.txt", "one.txt", "three.txt"].map(PathBuf::from)
        );
        history.record(PathBuf::from("ignored.txt"), 0);
        assert!(history.paths.is_empty());
    }

    #[test]
    fn recent_files_round_trip_and_clear() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("recent-files.json");
        let mut history = RecentFiles {
            paths: vec![
                PathBuf::from("one.txt"),
                PathBuf::from("one.txt"),
                PathBuf::from("two.txt"),
            ],
        };
        history.save(&path).expect("save");
        assert_eq!(
            RecentFiles::load(&path, 10).expect("load").paths,
            ["one.txt", "two.txt"].map(PathBuf::from)
        );

        history.clear();
        history.save(&path).expect("clear save");
        assert!(
            RecentFiles::load(&path, 10)
                .expect("cleared")
                .paths
                .is_empty()
        );
    }
}

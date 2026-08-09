use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

pub struct FileWatcher {
    watcher: RecommendedWatcher,
    receiver: Receiver<PathBuf>,
    directories: HashSet<PathBuf>,
}

impl FileWatcher {
    pub fn new() -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                for path in event_paths(&event.paths) {
                    let _ = sender.send(path);
                }
            }
        })
        .context("could not create filesystem watcher")?;

        Ok(Self {
            watcher,
            receiver,
            directories: HashSet::new(),
        })
    }

    pub fn watch_file(&mut self, path: &Path) -> Result<()> {
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let directory = directory.to_path_buf();
        if self.directories.insert(directory.clone()) {
            self.watcher
                .watch(&directory, RecursiveMode::NonRecursive)
                .with_context(|| format!("could not watch {}", directory.display()))?;
        }
        Ok(())
    }

    pub fn drain_changed_paths(&self) -> HashSet<PathBuf> {
        self.receiver.try_iter().collect()
    }
}

fn event_paths(paths: &[PathBuf]) -> HashSet<PathBuf> {
    paths.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_paths_are_deduplicated_without_losing_the_changed_file() {
        let paths = vec![
            PathBuf::from("/tmp/project/a.txt"),
            PathBuf::from("/tmp/project/a.txt"),
            PathBuf::from("/tmp/other/c.txt"),
        ];
        let paths = event_paths(&paths);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(Path::new("/tmp/project/a.txt")));
        assert!(paths.contains(Path::new("/tmp/other/c.txt")));
    }
}

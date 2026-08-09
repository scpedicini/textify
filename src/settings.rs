use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::document::FileMode;

pub fn textify_data_dir() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("TEXTIFY_DATA_DIR") {
        return path.into();
    }

    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("Library/Application Support/Textify")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorBudgets {
    pub normal_undo_bytes: usize,
    pub large_undo_bytes: usize,
    pub normal_search_matches: usize,
    pub large_search_matches: usize,
}

impl Default for EditorBudgets {
    fn default() -> Self {
        Self {
            normal_undo_bytes: 64 * 1024 * 1024,
            large_undo_bytes: 8 * 1024 * 1024,
            normal_search_matches: 20_000,
            large_search_matches: 2_000,
        }
    }
}

impl EditorBudgets {
    pub fn for_mode(self, mode: FileMode) -> (usize, usize) {
        match mode {
            FileMode::Normal => (self.normal_undo_bytes, self.normal_search_matches),
            FileMode::Large | FileMode::HugeViewer => {
                (self.large_undo_bytes, self.large_search_matches)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TextifySettings {
    pub editor: EditorBudgets,
}

impl TextifySettings {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("could not parse {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_settings_use_safe_defaults() {
        let settings: TextifySettings = serde_json::from_str(
            r#"{"editor":{"large_undo_bytes":1024,"large_search_matches":12}}"#,
        )
        .expect("settings");

        assert_eq!(settings.editor.large_undo_bytes, 1024);
        assert_eq!(settings.editor.large_search_matches, 12);
        assert_eq!(settings.editor.normal_search_matches, 20_000);
    }

    #[test]
    fn large_and_huge_modes_use_bounded_budgets() {
        let budgets = EditorBudgets::default();
        assert_eq!(budgets.for_mode(FileMode::Large), (8 * 1024 * 1024, 2_000));
        assert_eq!(
            budgets.for_mode(FileMode::HugeViewer),
            budgets.for_mode(FileMode::Large)
        );
    }
}

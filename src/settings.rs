use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{document::FileMode, file_io::save_atomic};

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
    pub appearance: AppearanceSettings,
    pub recovery: RecoverySettings,
    pub editor: EditorBudgets,
    pub workspace: WorkspaceSettings,
    pub lsp: LspSettings,
}

impl TextifySettings {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let mut settings: Self = serde_json::from_slice(&bytes)
                    .with_context(|| format!("could not parse {}", path.display()))?;
                settings.appearance.normalize();
                Ok(settings)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).context("could not serialize settings")?;
        save_atomic(path, &json)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub font_family: String,
    pub font_size: u16,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            font_family: "SFMono-Regular".to_owned(),
            font_size: 14,
        }
    }
}

impl AppearanceSettings {
    pub const MIN_FONT_SIZE: u16 = 8;
    pub const MAX_FONT_SIZE: u16 = 40;

    pub fn normalize(&mut self) {
        self.font_family = self.font_family.trim().to_owned();
        if self.font_family.is_empty() {
            self.font_family = Self::default().font_family;
        }
        self.font_size = self
            .font_size
            .clamp(Self::MIN_FONT_SIZE, Self::MAX_FONT_SIZE);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecoverySettings {
    pub save_temporary_files: bool,
    pub keep_unsaved_changes: bool,
    pub temporary_files_location: Option<PathBuf>,
}

impl Default for RecoverySettings {
    fn default() -> Self {
        Self {
            save_temporary_files: true,
            keep_unsaved_changes: true,
            temporary_files_location: None,
        }
    }
}

impl RecoverySettings {
    pub fn directory(&self, data_dir: &Path) -> PathBuf {
        self.temporary_files_location
            .clone()
            .unwrap_or_else(|| data_dir.join("Backups"))
    }

    pub const fn enabled_for(&self, has_file_path: bool) -> bool {
        if has_file_path {
            self.keep_unsaved_changes
        } else {
            self.save_temporary_files
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceSettings {
    pub max_entries: usize,
    pub quick_open_results: usize,
    pub search_max_file_bytes: u64,
    pub search_max_matches: usize,
    pub git_enabled: bool,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            quick_open_results: 100,
            search_max_file_bytes: 8 * 1024 * 1024,
            search_max_matches: 2_000,
            git_enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LspSettings {
    pub enabled: bool,
    pub command: Vec<String>,
    pub file_extensions: Vec<String>,
    pub max_document_bytes: u64,
}

impl Default for LspSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            command: Vec::new(),
            file_extensions: vec!["rs".to_owned()],
            max_document_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextifyKeymap {
    pub command_palette: String,
    pub quick_open: String,
    pub workspace_search: String,
    pub open_folder: String,
    pub toggle_sidebar: String,
    pub go_to_definition: String,
}

impl Default for TextifyKeymap {
    fn default() -> Self {
        Self {
            command_palette: "cmd-shift-p".to_owned(),
            quick_open: "cmd-p".to_owned(),
            workspace_search: "cmd-shift-f".to_owned(),
            open_folder: "cmd-shift-o".to_owned(),
            toggle_sidebar: "cmd-b".to_owned(),
            go_to_definition: "f12".to_owned(),
        }
    }
}

impl TextifyKeymap {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("could not parse {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
        }
    }
}

pub fn ensure_config_files(data_dir: &Path) -> Result<()> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("could not create {}", data_dir.display()))?;
    write_default_if_missing(&data_dir.join("settings.json"), &TextifySettings::default())?;
    write_default_if_missing(&data_dir.join("keymap.json"), &TextifyKeymap::default())?;
    Ok(())
}

fn write_default_if_missing(path: &Path, value: &impl Serialize) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
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
        assert_eq!(settings.workspace.max_entries, 100_000);
        assert!(!settings.lsp.enabled);
        assert_eq!(settings.appearance, AppearanceSettings::default());
        assert_eq!(settings.recovery, RecoverySettings::default());
    }

    #[test]
    fn recovery_location_defaults_to_private_application_storage() {
        let root = Path::new("/tmp/textify-data");
        let recovery = RecoverySettings::default();
        assert_eq!(recovery.directory(root), root.join("Backups"));
        assert!(recovery.enabled_for(false));
        assert!(recovery.enabled_for(true));

        let custom = RecoverySettings {
            temporary_files_location: Some(PathBuf::from("/tmp/custom-textify")),
            ..RecoverySettings::default()
        };
        assert_eq!(custom.directory(root), PathBuf::from("/tmp/custom-textify"));
    }

    #[test]
    fn appearance_normalization_keeps_rendering_values_safe() {
        let mut appearance = AppearanceSettings {
            font_family: "   ".to_owned(),
            font_size: 200,
        };
        appearance.normalize();
        assert_eq!(appearance.font_family, "SFMono-Regular");
        assert_eq!(appearance.font_size, AppearanceSettings::MAX_FONT_SIZE);
    }

    #[test]
    fn settings_save_round_trips_atomically() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("settings.json");
        let mut settings = TextifySettings::default();
        settings.appearance.font_size = 18;
        settings.recovery.keep_unsaved_changes = false;

        settings.save(&path).expect("save");
        assert_eq!(TextifySettings::load(&path).expect("load"), settings);
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

    #[test]
    fn default_config_files_are_created_without_overwriting_user_content() {
        let directory = tempfile::tempdir().expect("directory");
        ensure_config_files(directory.path()).expect("defaults");
        assert!(directory.path().join("settings.json").exists());
        assert_eq!(
            TextifyKeymap::load(&directory.path().join("keymap.json")).expect("keymap"),
            TextifyKeymap::default()
        );

        fs::write(directory.path().join("keymap.json"), b"{}\n").expect("custom keymap");
        ensure_config_files(directory.path()).expect("second ensure");
        assert_eq!(
            fs::read(directory.path().join("keymap.json")).unwrap(),
            b"{}\n"
        );
    }
}

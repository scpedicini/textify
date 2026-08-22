use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{document::FileMode, file_io::save_atomic};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPlatform {
    Macos,
    Windows,
    Unix,
}

fn host_platform() -> HostPlatform {
    if cfg!(target_os = "macos") {
        HostPlatform::Macos
    } else if cfg!(target_os = "windows") {
        HostPlatform::Windows
    } else {
        HostPlatform::Unix
    }
}

fn textify_data_dir_from(
    platform: HostPlatform,
    environment: impl Fn(&str) -> Option<OsString>,
) -> PathBuf {
    if let Some(path) = environment("TEXTIFY_DATA_DIR") {
        return path.into();
    }

    match platform {
        HostPlatform::Macos => environment("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Textify"),
        HostPlatform::Windows => environment("APPDATA")
            .or_else(|| environment("LOCALAPPDATA"))
            .map(PathBuf::from)
            .or_else(|| {
                environment("USERPROFILE")
                    .map(PathBuf::from)
                    .map(|path| path.join("AppData/Roaming"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Textify"),
        HostPlatform::Unix => environment("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                environment("HOME")
                    .map(PathBuf::from)
                    .map(|path| path.join(".config"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join("textify"),
    }
}

pub fn textify_data_dir() -> PathBuf {
    textify_data_dir_from(host_platform(), |name| std::env::var_os(name))
}

fn default_editor_font(platform: HostPlatform) -> &'static str {
    match platform {
        HostPlatform::Macos => "Menlo",
        HostPlatform::Windows => "Consolas",
        HostPlatform::Unix => "DejaVu Sans Mono",
    }
}

fn editor_font_candidates(platform: HostPlatform) -> &'static [&'static str] {
    match platform {
        HostPlatform::Macos => &["Menlo", ".SF NS Mono", "Andale Mono"],
        HostPlatform::Windows => &["Consolas", "Cascadia Mono", "Courier New"],
        HostPlatform::Unix => &[
            "DejaVu Sans Mono",
            "Liberation Mono",
            "Noto Sans Mono",
            "Ubuntu Mono",
            "Cascadia Mono",
            "Source Code Pro",
        ],
    }
}

pub fn available_editor_font(configured: &str, installed: &[String]) -> String {
    if let Some(font) = installed
        .iter()
        .find(|font| font.eq_ignore_ascii_case(configured))
    {
        return font.clone();
    }

    editor_font_candidates(host_platform())
        .iter()
        .find_map(|candidate| {
            installed
                .iter()
                .find(|font| font.eq_ignore_ascii_case(candidate))
                .cloned()
        })
        .unwrap_or_else(|| default_editor_font(host_platform()).to_owned())
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
    pub indentation: IndentationSettings,
    pub recovery: RecoverySettings,
    pub recent_files: RecentFileSettings,
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
                let original = settings.clone();
                settings.appearance.normalize();
                settings.indentation.normalize();
                settings.recent_files.normalize();
                if settings != original
                    && let Err(error) = settings.save(path)
                {
                    tracing::warn!(
                        %error,
                        path = %path.display(),
                        "could not persist normalized settings"
                    );
                }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndentationSettings {
    pub tab_width: usize,
    pub hard_tabs: bool,
}

impl Default for IndentationSettings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            hard_tabs: false,
        }
    }
}

impl IndentationSettings {
    pub const MIN_TAB_WIDTH: usize = 1;
    pub const MAX_TAB_WIDTH: usize = 8;

    pub fn normalize(&mut self) {
        self.tab_width = self
            .tab_width
            .clamp(Self::MIN_TAB_WIDTH, Self::MAX_TAB_WIDTH);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentFileSettings {
    pub enabled: bool,
    pub max_files: usize,
}

impl Default for RecentFileSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_files: 10,
        }
    }
}

impl RecentFileSettings {
    pub const MIN_FILES: usize = 1;
    pub const MAX_FILES: usize = 100;

    pub fn normalize(&mut self) {
        self.max_files = self.max_files.clamp(Self::MIN_FILES, Self::MAX_FILES);
    }

    pub const fn limit(&self) -> usize {
        if self.enabled { self.max_files } else { 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub font_family: String,
    pub font_size: u16,
    pub minimap_on_by_default: bool,
    pub word_wrap_on_by_default: bool,
    pub show_line_numbers: bool,
    pub show_title_bar: bool,
    pub show_tagline: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            font_family: default_editor_font(host_platform()).to_owned(),
            font_size: 14,
            minimap_on_by_default: false,
            word_wrap_on_by_default: false,
            show_line_numbers: true,
            show_title_bar: true,
            show_tagline: true,
        }
    }
}

impl AppearanceSettings {
    pub const MIN_FONT_SIZE: u16 = 8;
    pub const MAX_FONT_SIZE: u16 = 40;

    pub fn normalize(&mut self) {
        self.normalize_for(host_platform());
    }

    fn normalize_for(&mut self, platform: HostPlatform) {
        self.font_family = self.font_family.trim().to_owned();
        if self.font_family.is_empty() || self.font_family.eq_ignore_ascii_case("SFMono-Regular") {
            self.font_family = default_editor_font(platform).to_owned();
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
    #[serde(alias = "quick_open_results")]
    pub open_tab_search_results: usize,
    pub search_max_file_bytes: u64,
    pub search_max_matches: usize,
    pub git_enabled: bool,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            open_tab_search_results: 100,
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
    #[serde(alias = "quick_open")]
    pub search_open_tabs: String,
    pub workspace_search: String,
    pub open_folder: String,
    pub toggle_sidebar: String,
    pub go_to_definition: String,
}

impl Default for TextifyKeymap {
    fn default() -> Self {
        Self {
            command_palette: "secondary-shift-p".to_owned(),
            search_open_tabs: "secondary-p".to_owned(),
            workspace_search: "secondary-shift-f".to_owned(),
            open_folder: "secondary-shift-o".to_owned(),
            toggle_sidebar: "secondary-b".to_owned(),
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
        assert!(!settings.appearance.minimap_on_by_default);
        assert!(!settings.appearance.word_wrap_on_by_default);
        assert_eq!(settings.indentation, IndentationSettings::default());
        assert_eq!(settings.recovery, RecoverySettings::default());
        assert_eq!(settings.recent_files, RecentFileSettings::default());
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
            ..AppearanceSettings::default()
        };
        appearance.normalize();
        assert_eq!(appearance.font_family, default_editor_font(host_platform()));
        assert_eq!(appearance.font_size, AppearanceSettings::MAX_FONT_SIZE);
    }

    #[test]
    fn application_data_directories_follow_each_platform_convention() {
        let environment = |name: &str| match name {
            "HOME" => Some(OsString::from("/home/textify")),
            "APPDATA" => Some(OsString::from(r"C:\Users\Textify\AppData\Roaming")),
            "XDG_CONFIG_HOME" => Some(OsString::from("/config")),
            _ => None,
        };

        assert_eq!(
            textify_data_dir_from(HostPlatform::Macos, environment),
            PathBuf::from("/home/textify/Library/Application Support/Textify")
        );
        assert_eq!(
            textify_data_dir_from(HostPlatform::Windows, environment),
            PathBuf::from(r"C:\Users\Textify\AppData\Roaming").join("Textify")
        );
        assert_eq!(
            textify_data_dir_from(HostPlatform::Unix, environment),
            PathBuf::from("/config/textify")
        );
    }

    #[test]
    fn explicit_data_directory_overrides_platform_defaults() {
        let expected = PathBuf::from("/portable/textify-data");
        let environment =
            |name: &str| (name == "TEXTIFY_DATA_DIR").then(|| OsString::from(expected.as_os_str()));

        for platform in [
            HostPlatform::Macos,
            HostPlatform::Windows,
            HostPlatform::Unix,
        ] {
            assert_eq!(textify_data_dir_from(platform, environment), expected);
        }
    }

    #[test]
    fn editor_fonts_have_native_cross_platform_defaults() {
        assert_eq!(default_editor_font(HostPlatform::Macos), "Menlo");
        assert_eq!(default_editor_font(HostPlatform::Windows), "Consolas");
        assert_eq!(default_editor_font(HostPlatform::Unix), "DejaVu Sans Mono");
    }

    #[test]
    fn legacy_generated_macos_font_migrates_to_each_platform_default() {
        for (platform, expected) in [
            (HostPlatform::Macos, "Menlo"),
            (HostPlatform::Windows, "Consolas"),
            (HostPlatform::Unix, "DejaVu Sans Mono"),
        ] {
            let mut appearance = AppearanceSettings {
                font_family: "SFMono-Regular".to_owned(),
                ..AppearanceSettings::default()
            };
            appearance.normalize_for(platform);
            assert_eq!(appearance.font_family, expected);
        }
    }

    #[test]
    fn unavailable_editor_font_uses_an_installed_monospace_candidate() {
        let candidates = editor_font_candidates(host_platform());
        let installed = vec!["Proportional Sans".to_owned(), candidates[1].to_owned()];
        assert_eq!(
            available_editor_font("Missing Font", &installed),
            candidates[1]
        );
        assert_eq!(
            available_editor_font("proportional sans", &installed),
            "Proportional Sans"
        );
    }

    #[test]
    fn loading_rewrites_the_legacy_generated_font() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("settings.json");
        let mut settings = TextifySettings::default();
        settings.appearance.font_family = "SFMono-Regular".to_owned();
        settings.save(&path).expect("save legacy settings");

        let loaded = TextifySettings::load(&path).expect("load settings");
        assert_eq!(
            loaded.appearance.font_family,
            default_editor_font(host_platform())
        );
        let persisted: TextifySettings =
            serde_json::from_slice(&fs::read(&path).expect("read migrated settings"))
                .expect("parse migrated settings");
        assert_eq!(persisted, loaded);
    }

    #[test]
    fn settings_save_round_trips_atomically() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("settings.json");
        let mut settings = TextifySettings::default();
        settings.appearance.font_size = 18;
        settings.appearance.show_tagline = false;
        settings.appearance.minimap_on_by_default = true;
        settings.appearance.word_wrap_on_by_default = true;
        settings.indentation = IndentationSettings {
            tab_width: 2,
            hard_tabs: true,
        };
        settings.recovery.keep_unsaved_changes = false;
        settings.recent_files.max_files = 24;

        settings.save(&path).expect("save");
        assert_eq!(TextifySettings::load(&path).expect("load"), settings);
    }

    #[test]
    fn indentation_settings_are_explicit_and_bounded() {
        let mut indentation = IndentationSettings {
            tab_width: usize::MAX,
            hard_tabs: true,
        };
        indentation.normalize();
        assert_eq!(indentation.tab_width, IndentationSettings::MAX_TAB_WIDTH);
        assert!(indentation.hard_tabs);

        indentation.tab_width = 0;
        indentation.normalize();
        assert_eq!(indentation.tab_width, IndentationSettings::MIN_TAB_WIDTH);
    }

    #[test]
    fn recent_file_settings_are_bounded_and_can_disable_history() {
        let mut recent = RecentFileSettings {
            enabled: true,
            max_files: usize::MAX,
        };
        recent.normalize();
        assert_eq!(recent.limit(), RecentFileSettings::MAX_FILES);

        recent.enabled = false;
        assert_eq!(recent.limit(), 0);
    }

    #[test]
    fn legacy_quick_open_configuration_migrates_to_open_tab_search() {
        let settings: TextifySettings =
            serde_json::from_str(r#"{"workspace":{"quick_open_results":42}}"#)
                .expect("legacy settings");
        assert_eq!(settings.workspace.open_tab_search_results, 42);

        let keymap: TextifyKeymap =
            serde_json::from_str(r#"{"quick_open":"cmd-k"}"#).expect("legacy keymap");
        assert_eq!(keymap.search_open_tabs, "cmd-k");
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

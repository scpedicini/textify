use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::document::TextEncoding;
use crate::file_io::save_atomic;

const SESSION_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTab {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub recovery_path: Option<PathBuf>,
    #[serde(default)]
    pub untitled_number: usize,
    #[serde(default)]
    pub label_override: Option<String>,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub encoding: TextEncoding,
    #[serde(default)]
    pub font_size_override: Option<u16>,
    #[serde(default)]
    pub word_wrap: bool,
}

impl SessionTab {
    pub fn saved(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            recovery_path: None,
            untitled_number: 0,
            label_override: None,
            dirty: false,
            encoding: TextEncoding::Utf8,
            font_size_override: None,
            word_wrap: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,
    pub active_index: usize,
    /// Kept in version 2 for compatibility with generated corpora and version 1 sessions.
    #[serde(default)]
    pub open_paths: Vec<PathBuf>,
    #[serde(default)]
    pub tabs: Vec<SessionTab>,
    #[serde(default)]
    pub workspace_root: Option<PathBuf>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            active_index: 0,
            open_paths: Vec::new(),
            tabs: Vec::new(),
            workspace_root: None,
        }
    }
}

impl SessionState {
    pub fn new(active_index: usize, open_paths: Vec<PathBuf>) -> Self {
        let tabs = open_paths.iter().cloned().map(SessionTab::saved).collect();
        let active_index = active_index.min(open_paths.len().saturating_sub(1));
        Self {
            active_index,
            open_paths,
            tabs,
            ..Self::default()
        }
    }

    pub fn from_tabs(active_index: usize, tabs: Vec<SessionTab>) -> Self {
        let active_index = active_index.min(tabs.len().saturating_sub(1));
        let open_paths = tabs.iter().filter_map(|tab| tab.path.clone()).collect();
        Self {
            active_index,
            open_paths,
            tabs,
            ..Self::default()
        }
    }

    pub fn with_workspace_root(mut self, workspace_root: Option<PathBuf>) -> Self {
        self.workspace_root = workspace_root;
        self
    }
}

pub fn load_session(path: &Path) -> Result<SessionState> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SessionState::default());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    let state: SessionState = serde_json::from_slice(&bytes)
        .with_context(|| format!("could not parse {}", path.display()))?;
    if state.version > SESSION_VERSION || state.version == 0 {
        return Ok(SessionState::default());
    }
    let workspace_root = state.workspace_root;
    let tabs = if state.version == 1 || state.tabs.is_empty() {
        state
            .open_paths
            .into_iter()
            .map(SessionTab::saved)
            .collect()
    } else {
        state.tabs
    };
    Ok(SessionState::from_tabs(state.active_index, tabs).with_workspace_root(workspace_root))
}

pub fn save_session(path: &Path, state: &SessionState) -> Result<()> {
    let json = serde_json::to_string_pretty(state).context("could not serialize session")?;
    save_atomic(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trip_clamps_active_tab() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("session.json");
        let state = SessionState::new(99, vec![PathBuf::from("one.md"), PathBuf::from("two.json")])
            .with_workspace_root(Some(PathBuf::from("project")));

        save_session(&path, &state).expect("save");
        let restored = load_session(&path).expect("load");

        assert_eq!(restored.active_index, 1);
        assert_eq!(restored.open_paths, state.open_paths);
        assert_eq!(restored.tabs, state.tabs);
        assert_eq!(restored.workspace_root, state.workspace_root);
    }

    #[test]
    fn missing_and_future_sessions_restore_empty() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("session.json");
        assert_eq!(
            load_session(&path).expect("missing"),
            SessionState::default()
        );

        fs::write(&path, r#"{"version":999,"active_index":0,"open_paths":[]}"#).expect("fixture");
        assert_eq!(
            load_session(&path).expect("future"),
            SessionState::default()
        );
    }

    #[test]
    fn recovery_tabs_round_trip_and_version_one_sessions_migrate() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("session.json");
        let tab = SessionTab {
            path: None,
            recovery_path: Some(PathBuf::from("draft.txt")),
            untitled_number: 4,
            label_override: Some("Draft".to_owned()),
            dirty: true,
            encoding: TextEncoding::Cp437,
            font_size_override: Some(18),
            word_wrap: true,
        };
        let state = SessionState::from_tabs(0, vec![tab.clone()]);
        save_session(&path, &state).expect("save");
        assert_eq!(load_session(&path).expect("load").tabs, vec![tab]);

        fs::write(
            &path,
            r#"{"version":1,"active_index":0,"open_paths":["legacy.md"]}"#,
        )
        .expect("legacy fixture");
        let migrated = load_session(&path).expect("legacy load");
        assert_eq!(
            migrated.tabs,
            vec![SessionTab::saved(PathBuf::from("legacy.md"))]
        );
    }
}

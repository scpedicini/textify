use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::file_io::save_atomic;

const SESSION_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,
    pub active_index: usize,
    pub open_paths: Vec<PathBuf>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            active_index: 0,
            open_paths: Vec::new(),
        }
    }
}

impl SessionState {
    pub fn new(active_index: usize, open_paths: Vec<PathBuf>) -> Self {
        let active_index = active_index.min(open_paths.len().saturating_sub(1));
        Self {
            active_index,
            open_paths,
            ..Self::default()
        }
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
    if state.version != SESSION_VERSION {
        return Ok(SessionState::default());
    }
    Ok(SessionState::new(state.active_index, state.open_paths))
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
        let state = SessionState::new(99, vec![PathBuf::from("one.md"), PathBuf::from("two.json")]);

        save_session(&path, &state).expect("save");
        let restored = load_session(&path).expect("load");

        assert_eq!(restored.active_index, 1);
        assert_eq!(restored.open_paths, state.open_paths);
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
}

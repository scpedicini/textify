use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;

use gpui::{
    App, AppContext as _, Application, Context, Entity, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, Render, StatefulInteractiveElement as _, Styled, Subscription,
    Timer, Window, WindowBounds, WindowOptions, actions, div, prelude::FluentBuilder as _, px,
    size, uniform_list,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Root, Sizable as _, StyledExt as _, Theme,
    ThemeMode, TitleBar, WindowExt,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{Input, InputEvent, InputState, RopeExt as _},
    switch::Switch,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_component_assets::Assets;

use crate::{
    document::{DocumentMetadata, FileAnalysis, FileMode, FilePolicy, Language, LineEnding},
    editor::EditorBackend,
    file_io::{
        DiskRevision, ExternalFileChanged, LoadedFile, load_utf8, optional_disk_revision,
        save_atomic_chunks_checked, suggested_save_path,
    },
    huge_file::{HugeFile, MAX_EDIT_RANGE_BYTES},
    huge_viewer::{HugeFileView, HugeViewerEvent},
    lsp::{DefinitionLocation, LspClient, LspEvent, parse_definition_locations},
    project::{
        ProjectIndex, SearchSummary, WorkspaceMatch, load_git_status, stream_workspace_search,
    },
    recovery::{load_snapshot, remove_snapshot, write_snapshot},
    session::{SessionState, SessionTab, load_session, save_session},
    settings::{
        AppearanceSettings, RecoverySettings, TextifyKeymap, TextifySettings, ensure_config_files,
        textify_data_dir,
    },
    watcher::FileWatcher,
};

actions!(
    textify,
    [
        NewDocument,
        OpenDocument,
        SaveDocument,
        SaveDocumentAs,
        CloseDocument,
        NextDocument,
        PreviousDocument,
        OpenFolder,
        ToggleSidebar,
        ShowCommandPalette,
        ShowQuickOpen,
        ShowWorkspaceSearch,
        ShowSettings,
        GoToDefinition,
        DismissOverlay
    ]
);

const WINDOW_TITLE: &str = "Textify IDE";
const RECOVERY_DEBOUNCE: Duration = Duration::from_millis(250);

fn new_recovery_key(id: u64) -> u128 {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    timestamp ^ ((std::process::id() as u128) << 64) ^ id as u128
}

fn open_file(path: &Path, policy: FilePolicy) -> anyhow::Result<OpenedFile> {
    let metadata = fs::metadata(path)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("could not inspect {}", path.display()))?;
    if metadata.len() >= policy.huge_file_bytes {
        let analysis = FileAnalysis {
            bytes: metadata.len(),
            lines: 0,
            longest_line_bytes: 0,
            line_ending: LineEnding::None,
        };
        return Ok(OpenedFile::Huge {
            file: HugeFile::open(path)?,
            metadata: DocumentMetadata::new(Some(path.to_path_buf()), analysis, policy),
        });
    }
    load_utf8(path, policy).map(OpenedFile::Editable)
}

struct EditorDocument {
    id: u64,
    untitled_number: usize,
    editor: EditorBackend,
    metadata: DocumentMetadata,
    dirty: bool,
    saving: bool,
    revision: u64,
    disk_revision: Option<DiskRevision>,
    external_changed: bool,
    programmatic_change: bool,
    label_override: Option<String>,
    huge_viewer: Option<Entity<HugeFileView>>,
    _subscription: Subscription,
    _huge_subscription: Option<Subscription>,
    recovery_key: u128,
    recovery_path: Option<PathBuf>,
    recovery_revision: u64,
}

struct DocumentSeed {
    text: String,
    metadata: DocumentMetadata,
    disk_revision: Option<DiskRevision>,
    label_override: Option<String>,
    untitled_number: usize,
    dirty: bool,
    recovery_path: Option<PathBuf>,
}

enum OpenedFile {
    Editable(LoadedFile),
    Huge {
        file: HugeFile,
        metadata: DocumentMetadata,
    },
}

enum RestoredFile {
    Opened(OpenedFile),
    Recovered(DocumentSeed),
}

fn restore_session_tab(tab: SessionTab, policy: FilePolicy) -> anyhow::Result<RestoredFile> {
    if let Some(recovery_path) = tab.recovery_path.clone() {
        let text = load_snapshot(&recovery_path)?;
        let analysis = FileAnalysis::from_bytes(text.as_bytes());
        let metadata = DocumentMetadata::new(tab.path.clone(), analysis, policy);
        let disk_revision = tab
            .path
            .as_deref()
            .map(optional_disk_revision)
            .transpose()?
            .flatten();
        return Ok(RestoredFile::Recovered(DocumentSeed {
            text,
            metadata,
            disk_revision,
            label_override: tab.label_override,
            untitled_number: tab.untitled_number,
            dirty: tab.dirty,
            recovery_path: Some(recovery_path),
        }));
    }

    if let Some(path) = tab.path {
        return open_file(&path, policy).map(RestoredFile::Opened);
    }

    let metadata = DocumentMetadata::new(None, FileAnalysis::from_bytes(b""), policy);
    Ok(RestoredFile::Recovered(DocumentSeed {
        text: String::new(),
        metadata,
        disk_revision: None,
        label_override: tab.label_override,
        untitled_number: tab.untitled_number.max(1),
        dirty: false,
        recovery_path: None,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayMode {
    Commands,
    QuickOpen,
    WorkspaceSearch,
}

#[derive(Debug, Clone)]
enum IdeCommand {
    NewFile,
    OpenFile,
    OpenFolder,
    QuickOpen,
    WorkspaceSearch,
    ToggleSidebar,
    RefreshProject,
    OpenSettings,
    OpenKeymap,
    GoToDefinition,
}

#[derive(Debug, Clone)]
enum OverlayTarget {
    Command(IdeCommand),
    File(PathBuf),
    Search(WorkspaceMatch),
}

#[derive(Debug, Clone)]
struct OverlayItem {
    title: String,
    subtitle: String,
    target: OverlayTarget,
}

enum WorkspaceSearchEvent {
    Match(WorkspaceMatch),
    Finished(SearchSummary),
}

struct WorkspaceSearchStream {
    cancel: crate::huge_file::CancellationToken,
    receiver: mpsc::Receiver<WorkspaceSearchEvent>,
}

#[derive(Debug, Clone, Copy)]
struct TextLocation {
    line: usize,
    column: usize,
    end_line: usize,
    end_column: usize,
}

#[derive(Debug, Clone)]
struct SettingsDraft {
    font_size: u16,
    recovery: RecoverySettings,
}

impl EditorDocument {
    fn display_name(&self) -> String {
        self.label_override
            .clone()
            .unwrap_or_else(|| self.metadata.display_name(self.untitled_number))
    }

    fn title(&self) -> String {
        if self.dirty {
            format!("{} •", self.display_name())
        } else {
            self.display_name()
        }
    }
}

pub struct Workspace {
    documents: Vec<EditorDocument>,
    active_index: usize,
    next_id: u64,
    next_untitled_number: usize,
    policy: FilePolicy,
    settings: TextifySettings,
    watcher: Option<FileWatcher>,
    session_path: PathBuf,
    restoring_session: bool,
    external_scan_in_progress: bool,
    created_at: Instant,
    first_paint_logged: bool,
    data_dir: PathBuf,
    keymap: TextifyKeymap,
    config_reload_in_progress: bool,
    project: Option<ProjectIndex>,
    workspace_root: Option<PathBuf>,
    project_loading: bool,
    sidebar_visible: bool,
    git_status: HashMap<PathBuf, String>,
    git_loading: bool,
    overlay_mode: Option<OverlayMode>,
    overlay_input: Entity<InputState>,
    overlay_items: Vec<OverlayItem>,
    settings_visible: bool,
    settings_draft: Option<SettingsDraft>,
    settings_font_input: Entity<InputState>,
    settings_location_input: Entity<InputState>,
    workspace_search: Option<WorkspaceSearchStream>,
    status_message: Option<String>,
    lsp: Option<LspClient>,
    lsp_starting: bool,
    lsp_opened: HashSet<PathBuf>,
    lsp_dirty: HashMap<u64, Instant>,
    pending_definitions: HashSet<u64>,
    recovery_pending: HashMap<u64, Instant>,
    recovery_in_flight: HashSet<u64>,
    _ide_subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let data_dir = textify_data_dir();
        let settings =
            TextifySettings::load(&data_dir.join("settings.json")).unwrap_or_else(|error| {
                tracing::warn!(%error, "using default settings");
                TextifySettings::default()
            });
        let keymap = TextifyKeymap::load(&data_dir.join("keymap.json")).unwrap_or_else(|error| {
            tracing::warn!(%error, "using default keymap");
            TextifyKeymap::default()
        });
        let overlay_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Type a command")
                .clean_on_escape()
        });
        let settings_font_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("SFMono-Regular"));
        let settings_location_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Textify application backups"));
        let _ide_subscriptions =
            vec![
                cx.subscribe_in(&overlay_input, window, |workspace, _, event, window, cx| {
                    match event {
                        InputEvent::Change => workspace.refresh_overlay(window, cx),
                        InputEvent::PressEnter { .. } => workspace.accept_overlay(0, window, cx),
                        _ => {}
                    }
                }),
            ];
        let watcher = FileWatcher::new()
            .map_err(|error| {
                tracing::warn!(%error, "external-change watching unavailable");
                error
            })
            .ok();
        let mut workspace = Self {
            documents: Vec::new(),
            active_index: 0,
            next_id: 1,
            next_untitled_number: 1,
            policy: FilePolicy::default(),
            settings,
            watcher,
            session_path: data_dir.join("session.json"),
            restoring_session: false,
            external_scan_in_progress: false,
            created_at: Instant::now(),
            first_paint_logged: false,
            data_dir,
            keymap,
            config_reload_in_progress: false,
            project: None,
            workspace_root: None,
            project_loading: false,
            sidebar_visible: false,
            git_status: HashMap::new(),
            git_loading: false,
            overlay_mode: None,
            overlay_input,
            overlay_items: Vec::new(),
            settings_visible: false,
            settings_draft: None,
            settings_font_input,
            settings_location_input,
            workspace_search: None,
            status_message: None,
            lsp: None,
            lsp_starting: false,
            lsp_opened: HashSet::new(),
            lsp_dirty: HashMap::new(),
            pending_definitions: HashSet::new(),
            recovery_pending: HashMap::new(),
            recovery_in_flight: HashSet::new(),
            _ide_subscriptions,
        };
        workspace.add_untitled(window, cx);
        workspace
    }

    fn active_document(&self) -> &EditorDocument {
        &self.documents[self.active_index]
    }

    fn active_document_mut(&mut self) -> &mut EditorDocument {
        &mut self.documents[self.active_index]
    }

    fn active_id(&self) -> u64 {
        self.active_document().id
    }

    fn document_index(&self, id: u64) -> Option<usize> {
        self.documents.iter().position(|document| document.id == id)
    }

    fn add_untitled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let analysis = FileAnalysis::from_bytes(b"");
        let metadata = DocumentMetadata::new(None, analysis, self.policy);
        let untitled_number = self.next_untitled_number;
        self.next_untitled_number += 1;
        self.push_document(
            DocumentSeed {
                text: String::new(),
                disk_revision: None,
                label_override: None,
                untitled_number,
                dirty: false,
                recovery_path: None,
                metadata,
            },
            window,
            cx,
        );
    }

    fn push_loaded(&mut self, loaded: LoadedFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.documents.iter().position(|document| {
            document.metadata.path.as_deref() == loaded.metadata.path.as_deref()
        }) {
            self.set_active_index(index, window, cx);
            return;
        }

        let path = loaded.metadata.path.clone();
        self.push_document(
            DocumentSeed {
                text: loaded.text,
                metadata: loaded.metadata,
                disk_revision: Some(loaded.disk_revision),
                label_override: None,
                untitled_number: 0,
                dirty: false,
                recovery_path: None,
            },
            window,
            cx,
        );
        if let (Some(watcher), Some(path)) = (&mut self.watcher, path)
            && let Err(error) = watcher.watch_file(&path)
        {
            tracing::warn!(%error, path = %path.display(), "could not watch open file");
        }
        self.persist_session(cx);
    }

    fn push_opened(&mut self, opened: OpenedFile, window: &mut Window, cx: &mut Context<Self>) {
        match opened {
            OpenedFile::Editable(loaded) => self.push_loaded(loaded, window, cx),
            OpenedFile::Huge { file, metadata } => self.push_huge(file, metadata, window, cx),
        }
    }

    fn push_huge(
        &mut self,
        file: HugeFile,
        metadata: DocumentMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.metadata.path == metadata.path)
        {
            self.set_active_index(index, window, cx);
            return;
        }
        let viewer = match HugeFileView::new_entity(file, window, cx) {
            Ok(viewer) => viewer,
            Err(error) => {
                Self::show_error("Could not open huge file", error, window, cx);
                return;
            }
        };
        self.push_document(
            DocumentSeed {
                text: String::new(),
                metadata,
                disk_revision: None,
                label_override: None,
                untitled_number: 0,
                dirty: false,
                recovery_path: None,
            },
            window,
            cx,
        );
        let subscription = cx.subscribe_in(
            &viewer,
            window,
            move |workspace, _, event, window, cx| match event {
                HugeViewerEvent::EditRange { path, range } => {
                    workspace.open_huge_range(path.clone(), range.clone(), window, cx);
                }
            },
        );
        let document = self.active_document_mut();
        document.huge_viewer = Some(viewer);
        document._huge_subscription = Some(subscription);
        self.persist_session(cx);
        cx.notify();
    }

    fn push_document(&mut self, seed: DocumentSeed, window: &mut Window, cx: &mut Context<Self>) {
        let DocumentSeed {
            text,
            metadata,
            disk_revision,
            label_override,
            untitled_number,
            dirty,
            recovery_path,
        } = seed;
        let focus_editor = metadata.mode != FileMode::HugeViewer;
        let id = self.next_id;
        self.next_id += 1;
        let editor = EditorBackend::new(
            text,
            metadata.parser_name(self.policy),
            metadata.mode,
            self.settings.editor,
            window,
            cx,
        );
        let subscription = cx.subscribe(editor.state(), move |workspace, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                let mut changed = false;
                if let Some(document) = workspace
                    .documents
                    .iter_mut()
                    .find(|document| document.id == id)
                    && !document.programmatic_change
                {
                    document.dirty = true;
                    document.revision = document.revision.wrapping_add(1);
                    workspace.lsp_dirty.insert(id, Instant::now());
                    changed = true;
                }
                if changed {
                    workspace.mark_recovery_pending(id, cx);
                }
                cx.notify();
            }
        });

        self.documents.push(EditorDocument {
            id,
            untitled_number,
            editor: editor.clone(),
            metadata,
            dirty,
            saving: false,
            revision: 0,
            disk_revision,
            external_changed: false,
            programmatic_change: false,
            label_override,
            huge_viewer: None,
            _subscription: subscription,
            _huge_subscription: None,
            recovery_key: new_recovery_key(id),
            recovery_path,
            recovery_revision: 0,
        });
        self.active_index = self.documents.len() - 1;
        self.update_window_title(window);

        if focus_editor {
            window.defer(cx, move |window, cx| editor.focus(window, cx));
        }
        cx.notify();
    }

    fn set_active_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.documents.len() {
            return;
        }

        self.active_index = index;
        if self.active_document().huge_viewer.is_none() {
            self.active_document().editor.focus(window, cx);
        }
        self.update_window_title(window);
        self.persist_session(cx);
        cx.notify();
    }

    fn update_window_title(&self, window: &mut Window) {
        window.set_window_title(&format!(
            "{} — Textify",
            self.active_document().display_name()
        ));
    }

    fn persist_session(&self, cx: &mut Context<Self>) {
        if self.restoring_session {
            return;
        }

        let recover_temporary = self.settings.recovery.save_temporary_files;
        let tabs = self
            .documents
            .iter()
            .filter(|document| document.metadata.path.is_some() || recover_temporary)
            .map(|document| {
                let recovery_enabled = self
                    .settings
                    .recovery
                    .enabled_for(document.metadata.path.is_some());
                (
                    document.id,
                    SessionTab {
                        path: document.metadata.path.clone(),
                        recovery_path: (document.dirty && recovery_enabled)
                            .then(|| document.recovery_path.clone())
                            .flatten(),
                        untitled_number: document.untitled_number,
                        label_override: document.label_override.clone(),
                        dirty: document.dirty
                            && recovery_enabled
                            && document.recovery_path.is_some(),
                    },
                )
            })
            .collect::<Vec<_>>();
        let active_id = self.active_id();
        let active_index = tabs
            .iter()
            .position(|(id, _)| *id == active_id)
            .unwrap_or(0);
        let workspace_root = self.workspace_root.clone();
        let state =
            SessionState::from_tabs(active_index, tabs.into_iter().map(|(_, tab)| tab).collect())
                .with_workspace_root(workspace_root);
        let path = self.session_path.clone();
        cx.background_spawn(async move {
            if let Err(error) = save_session(&path, &state) {
                tracing::warn!(%error, "could not persist session");
            }
        })
        .detach();
    }

    fn mark_recovery_pending(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        let document = &self.documents[index];
        if !self
            .settings
            .recovery
            .enabled_for(document.metadata.path.is_some())
            || document.huge_viewer.is_some()
        {
            return;
        }
        self.recovery_pending.insert(id, Instant::now());
        self.persist_session(cx);
    }

    fn flush_recovery_due(&mut self, cx: &mut Context<Self>) {
        let due = self
            .recovery_pending
            .iter()
            .filter(|(id, changed_at)| {
                changed_at.elapsed() >= RECOVERY_DEBOUNCE && !self.recovery_in_flight.contains(id)
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in due {
            self.start_recovery_snapshot(id, cx);
        }
    }

    fn start_recovery_snapshot(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            self.recovery_pending.remove(&id);
            return;
        };
        let document = &self.documents[index];
        if !document.dirty
            || document.huge_viewer.is_some()
            || !self
                .settings
                .recovery
                .enabled_for(document.metadata.path.is_some())
        {
            self.recovery_pending.remove(&id);
            return;
        }

        let revision = document.revision;
        let key = document.recovery_key;
        let rope = document.editor.rope(cx);
        let directory = self.settings.recovery.directory(&self.data_dir);
        self.recovery_pending.remove(&id);
        self.recovery_in_flight.insert(id);
        let task =
            cx.background_spawn(
                async move { write_snapshot(&directory, key, revision, rope.chunks()) },
            );
        cx.spawn(async move |workspace, cx| {
            let result = task.await;
            let Some(workspace) = workspace.upgrade() else {
                return;
            };
            workspace
                .update(cx, |workspace, cx| {
                    workspace.recovery_in_flight.remove(&id);
                    match result {
                        Ok(path) => {
                            let mut remove = Some(path.clone());
                            if let Some(index) = workspace.document_index(id) {
                                let enabled = workspace.settings.recovery.enabled_for(
                                    workspace.documents[index].metadata.path.is_some(),
                                );
                                let document = &mut workspace.documents[index];
                                if document.dirty
                                    && enabled
                                    && revision >= document.recovery_revision
                                {
                                    remove = document.recovery_path.replace(path);
                                    document.recovery_revision = revision;
                                }
                                if document.dirty && document.revision > revision && enabled {
                                    workspace.recovery_pending.insert(id, Instant::now());
                                }
                            }
                            if let Some(remove) = remove {
                                cx.background_spawn(async move {
                                    if let Err(error) = remove_snapshot(&remove) {
                                        tracing::warn!(%error, "could not prune recovery copy");
                                    }
                                })
                                .detach();
                            }
                            workspace.persist_session(cx);
                        }
                        Err(error) => {
                            workspace.status_message =
                                Some("Could not update crash-recovery copy".to_owned());
                            tracing::warn!(%error, "recovery snapshot failed");
                        }
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn clear_recovery(&mut self, id: u64, cx: &mut Context<Self>) {
        self.recovery_pending.remove(&id);
        let path = self
            .document_index(id)
            .and_then(|index| self.documents[index].recovery_path.take());
        if let Some(path) = path {
            cx.background_spawn(async move {
                if let Err(error) = remove_snapshot(&path) {
                    tracing::warn!(%error, "could not remove recovery copy");
                }
            })
            .detach();
        }
    }

    fn start_background_services(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        bind_ide_keymap(cx, &self.keymap);
        self.prepare_config_files(window, cx);
        self.restore_session(window, cx);
        self.poll_watcher(window, cx);
        self.poll_lazy_services(window, cx);
    }

    fn prepare_config_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let data_dir = self.data_dir.clone();
        let task = cx.background_spawn(async move { ensure_config_files(&data_dir) });
        cx.spawn_in(window, async move |workspace, window| {
            let result = task.await;
            workspace
                .update_in(window, |workspace, _, _cx| match result {
                    Ok(()) => {
                        if let Some(watcher) = &mut workspace.watcher {
                            for path in [
                                workspace.data_dir.join("settings.json"),
                                workspace.data_dir.join("keymap.json"),
                            ] {
                                if let Err(error) = watcher.watch_file(&path) {
                                    tracing::warn!(%error, "could not watch Textify configuration");
                                }
                            }
                        }
                    }
                    Err(error) => tracing::warn!(%error, "could not prepare configuration files"),
                })
                .ok()
        })
        .detach();
    }

    fn poll_lazy_services(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |workspace, window| {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                if workspace
                    .update_in(window, |workspace, window, cx| {
                        workspace.poll_workspace_search(cx);
                        workspace.poll_lsp(window, cx);
                        workspace.flush_recovery_due(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restoring_session = true;
        let path = self.session_path.clone();
        let policy = self.policy;
        let task = cx.background_spawn(async move {
            let session = load_session(&path)?;
            let active_index = session.active_index;
            let workspace_root = session.workspace_root;
            let loaded = session
                .tabs
                .into_iter()
                .map(|tab| {
                    let label = tab
                        .path
                        .clone()
                        .or_else(|| tab.recovery_path.clone())
                        .unwrap_or_else(|| PathBuf::from("Untitled"));
                    restore_session_tab(tab, policy).map_err(|error| (label, error))
                })
                .collect::<Vec<_>>();
            Ok::<_, anyhow::Error>((active_index, loaded, workspace_root))
        });

        cx.spawn_in(window, async move |workspace, window| {
            let restored = task.await;
            workspace
                .update_in(window, |workspace, window, cx| {
                    workspace.restoring_session = false;
                    match restored {
                        Ok((active_index, loaded, workspace_root)) => {
                            let had_paths = loaded.iter().any(Result::is_ok);
                            if had_paths
                                && workspace.documents.len() == 1
                                && workspace.documents[0].metadata.path.is_none()
                                && !workspace.documents[0].dirty
                            {
                                workspace.documents.clear();
                                workspace.active_index = 0;
                            }
                            for result in loaded {
                                match result {
                                    Ok(RestoredFile::Opened(opened)) => {
                                        workspace.push_opened(opened, window, cx)
                                    }
                                    Ok(RestoredFile::Recovered(seed)) => {
                                        workspace.next_untitled_number = workspace
                                            .next_untitled_number
                                            .max(seed.untitled_number.saturating_add(1));
                                        workspace.push_document(seed, window, cx);
                                    }
                                    Err((path, error)) => tracing::warn!(
                                        %error,
                                        path = %path.display(),
                                        "could not restore tab"
                                    ),
                                }
                            }
                            if workspace.documents.is_empty() {
                                workspace.add_untitled(window, cx);
                            } else {
                                workspace.set_active_index(
                                    active_index.min(workspace.documents.len() - 1),
                                    window,
                                    cx,
                                );
                            }
                            if let Some(root) = workspace_root {
                                workspace.start_project_index(root, window, cx);
                            }
                        }
                        Err(error) => tracing::warn!(%error, "could not restore session"),
                    }
                    workspace.persist_session(cx);
                })
                .ok()
        })
        .detach();
    }

    fn poll_watcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |workspace, window| {
            loop {
                Timer::after(Duration::from_millis(500)).await;
                if workspace
                    .update_in(window, |workspace, window, cx| {
                        workspace.scan_external_changes(window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn scan_external_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.external_scan_in_progress {
            return;
        }
        let Some(watcher) = &self.watcher else {
            return;
        };
        let directories = watcher.drain_changed_directories();
        if directories.is_empty() {
            return;
        }
        if directories.contains(&self.data_dir) {
            self.reload_configuration(window, cx);
        }

        let targets = self
            .documents
            .iter()
            .filter_map(|document| {
                if document.huge_viewer.is_some() {
                    return None;
                }
                let path = document.metadata.path.as_ref()?;
                let parent = path.parent().unwrap_or_else(|| Path::new("."));
                directories
                    .contains(parent)
                    .then(|| (document.id, path.clone()))
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return;
        }

        self.external_scan_in_progress = true;
        let task = cx.background_spawn(async move {
            targets
                .into_iter()
                .map(|(id, path)| (id, optional_disk_revision(&path)))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |workspace, window| {
            let revisions = task.await;
            workspace
                .update_in(window, |workspace, window, cx| {
                    workspace.external_scan_in_progress = false;
                    for (id, result) in revisions {
                        match result {
                            Ok(actual) => {
                                workspace.notice_external_revision(id, actual, window, cx)
                            }
                            Err(error) => tracing::warn!(%error, "could not inspect watched file"),
                        }
                    }
                })
                .ok()
        })
        .detach();
    }

    fn notice_external_revision(
        &mut self,
        id: u64,
        actual: Option<DiskRevision>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        let document = &mut self.documents[index];
        if document.disk_revision == actual || document.external_changed {
            return;
        }
        document.external_changed = true;
        self.prompt_external_change(id, actual, window, cx);
        cx.notify();
    }

    fn prompt_external_change(
        &mut self,
        id: u64,
        actual: Option<DiskRevision>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        let name = self.documents[index].display_name();
        let detail = if actual.is_some() {
            "The file changed outside Textify. Reload it, keep your buffer as the next save base, or compare both versions."
        } else {
            "The file was removed outside Textify. Keep your buffer or compare it with the missing disk version."
        };
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let keep_workspace = workspace.clone();
            let reload_workspace = workspace.clone();
            let compare_workspace = workspace.clone();
            let keep_revision = actual.clone();
            dialog
                .title(format!("{name} changed on disk"))
                .child(detail)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Reload")
                        .cancel_text("Keep Mine"),
                )
                .on_cancel(move |_, _, cx| {
                    keep_workspace.update(cx, |workspace, cx| {
                        if let Some(index) = workspace.document_index(id) {
                            let document = &mut workspace.documents[index];
                            document.disk_revision = keep_revision.clone();
                            document.external_changed = false;
                            document.dirty = true;
                            cx.notify();
                        }
                    });
                    true
                })
                .on_ok(move |_, window, cx| {
                    reload_workspace.update(cx, |workspace, cx| {
                        workspace.reload_document(id, window, cx)
                    });
                    true
                })
                .footer(move |reload, keep, window, cx| {
                    let compare_workspace = compare_workspace.clone();
                    vec![
                        keep(window, cx),
                        Button::new(("compare-external", id))
                            .label("Compare")
                            .on_click(move |_, window, cx| {
                                compare_workspace.update(cx, |workspace, cx| {
                                    workspace.compare_external(id, window, cx)
                                });
                                window.close_dialog(cx);
                            })
                            .into_any_element(),
                        reload(window, cx),
                    ]
                })
        });
    }

    fn reload_document(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .document_index(id)
            .and_then(|index| self.documents[index].metadata.path.clone())
        else {
            return;
        };
        let policy = self.policy;
        let task = cx.background_spawn(async move { load_utf8(&path, policy) });
        cx.spawn_in(window, async move |workspace, window| {
            let loaded = task.await;
            workspace
                .update_in(window, |workspace, window, cx| match loaded {
                    Ok(loaded) => {
                        let Some(index) = workspace.document_index(id) else {
                            return;
                        };
                        let editor = workspace.documents[index].editor.clone();
                        let parser = loaded.metadata.parser_name(workspace.policy);
                        let document = &mut workspace.documents[index];
                        document.programmatic_change = true;
                        editor.set_text(loaded.text, window, cx);
                        editor.set_parser(parser, cx);
                        document.programmatic_change = false;
                        document.metadata = loaded.metadata;
                        document.disk_revision = Some(loaded.disk_revision);
                        document.external_changed = false;
                        document.dirty = false;
                        document.revision = document.revision.wrapping_add(1);
                        workspace.update_window_title(window);
                        workspace.persist_session(cx);
                        workspace.refresh_git(window, cx);
                        cx.notify();
                    }
                    Err(error) => Self::show_error("Could not reload file", error, window, cx),
                })
                .ok()
        })
        .detach();
    }

    fn compare_external(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        let Some(path) = self.documents[index].metadata.path.clone() else {
            return;
        };
        let name = self.documents[index].display_name();
        let comparison_name = name.clone();
        let local = self.documents[index].editor.rope(cx);
        let policy = self.policy;
        let task = cx.background_spawn(async move {
            let disk = match load_utf8(&path, policy) {
                Ok(loaded) => loaded.text,
                Err(error) => format!("<disk version unavailable: {error}>"),
            };
            format!(
                "===== TEXTIFY BUFFER: {comparison_name} =====\n{}\n\n===== DISK VERSION: {comparison_name} =====\n{}",
                local, disk
            )
        });
        cx.spawn_in(window, async move |workspace, window| {
            let comparison = task.await;
            workspace
                .update_in(window, |workspace, window, cx| {
                    let analysis = FileAnalysis::from_bytes(comparison.as_bytes());
                    let metadata = DocumentMetadata::new(None, analysis, workspace.policy);
                    let untitled_number = workspace.next_untitled_number;
                    workspace.next_untitled_number += 1;
                    workspace.push_document(
                        DocumentSeed {
                            text: comparison,
                            metadata,
                            disk_revision: None,
                            label_override: Some(format!("Compare {name}")),
                            untitled_number,
                            dirty: false,
                            recovery_path: None,
                        },
                        window,
                        cx,
                    );
                })
                .ok()
        })
        .detach();
    }

    fn open_huge_range(
        &mut self,
        path: PathBuf,
        range: std::ops::Range<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Huge file")
            .to_owned();
        let label = format!("{display_name} bytes {}–{}", range.start, range.end);
        let task = cx.background_spawn(async move {
            let file = HugeFile::open(&path)?;
            file.read_utf8_range(range, MAX_EDIT_RANGE_BYTES)
        });
        cx.spawn_in(window, async move |workspace, window| {
            let text = task.await;
            workspace
                .update_in(window, |workspace, window, cx| match text {
                    Ok(text) => {
                        let analysis = FileAnalysis::from_bytes(text.as_bytes());
                        let metadata = DocumentMetadata::new(None, analysis, workspace.policy);
                        let untitled_number = workspace.next_untitled_number;
                        workspace.next_untitled_number += 1;
                        workspace.push_document(
                            DocumentSeed {
                                text,
                                metadata,
                                disk_revision: None,
                                label_override: Some(label),
                                untitled_number,
                                dirty: true,
                                recovery_path: None,
                            },
                            window,
                            cx,
                        );
                        let id = workspace.active_id();
                        workspace.mark_recovery_pending(id, cx);
                        cx.notify();
                    }
                    Err(error) => {
                        Self::show_error("Could not edit selected range", error, window, cx)
                    }
                })
                .ok()
        })
        .detach();
    }

    fn on_open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Folder in Textify".into()),
        });
        cx.spawn_in(window, async move |workspace, window| {
            let path = receiver.await.ok()?.ok()??.into_iter().next()?;
            workspace
                .update_in(window, |workspace, window, cx| {
                    workspace.start_project_index(path, window, cx)
                })
                .ok()
        })
        .detach();
    }

    fn start_project_index(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_loading {
            return;
        }
        self.workspace_root = Some(root.clone());
        self.project_loading = true;
        self.status_message = Some(format!("Indexing {}…", root.display()));
        let max_entries = self.settings.workspace.max_entries;
        let task = cx.background_spawn(async move { ProjectIndex::build(&root, max_entries) });
        cx.notify();
        cx.spawn_in(window, async move |workspace, window| {
            let result = task.await;
            workspace
                .update_in(window, |workspace, window, cx| {
                    workspace.project_loading = false;
                    match result {
                        Ok(index) => {
                            let count = index.files.len();
                            let truncated = index.truncated;
                            workspace.workspace_root = Some(index.root.clone());
                            workspace.project = Some(index);
                            workspace.sidebar_visible = true;
                            workspace.status_message = Some(if truncated {
                                format!("Indexed {count} files (entry limit reached)")
                            } else {
                                format!("Indexed {count} files")
                            });
                            workspace.refresh_git(window, cx);
                            workspace.restart_lsp(window, cx);
                            workspace.refresh_overlay(window, cx);
                            workspace.persist_session(cx);
                        }
                        Err(error) => {
                            workspace.workspace_root = workspace
                                .project
                                .as_ref()
                                .map(|project| project.root.clone());
                            workspace.status_message = Some(error.to_string());
                            Self::show_error("Could not open folder", error, window, cx);
                        }
                    }
                    cx.notify();
                })
                .ok()
        })
        .detach();
    }

    fn refresh_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(root) = self.project.as_ref().map(|project| project.root.clone()) {
            self.project = None;
            self.git_status.clear();
            self.start_project_index(root, window, cx);
        }
    }

    fn refresh_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.workspace.git_enabled || self.git_loading {
            return;
        }
        let Some(root) = self.project.as_ref().map(|project| project.root.clone()) else {
            return;
        };
        self.git_loading = true;
        let task = cx.background_spawn(async move { load_git_status(&root) });
        cx.spawn_in(window, async move |workspace, window| {
            let result = task.await;
            workspace
                .update_in(window, |workspace, _, cx| {
                    workspace.git_loading = false;
                    match result {
                        Ok(status) => workspace.git_status = status,
                        Err(error) => tracing::warn!(%error, "could not refresh Git status"),
                    }
                    cx.notify();
                })
                .ok()
        })
        .detach();
    }

    fn open_path_at(
        &mut self,
        path: PathBuf,
        location: Option<TextLocation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.metadata.path.as_deref() == Some(path.as_path()))
        {
            self.set_active_index(index, window, cx);
            if let Some(location) = location
                && self.documents[index].huge_viewer.is_none()
            {
                self.documents[index].editor.select_position(
                    location.line,
                    location.column,
                    location.end_line,
                    location.end_column,
                    window,
                    cx,
                );
            }
            return;
        }
        let policy = self.policy;
        let load_path = path.clone();
        let task = cx.background_spawn(async move { open_file(&load_path, policy) });
        cx.spawn_in(window, async move |workspace, window| {
            let result = task.await;
            workspace
                .update_in(window, |workspace, window, cx| match result {
                    Ok(opened) => {
                        workspace.push_opened(opened, window, cx);
                        if let Some(location) = location
                            && workspace.active_document().huge_viewer.is_none()
                        {
                            workspace.active_document().editor.select_position(
                                location.line,
                                location.column,
                                location.end_line,
                                location.end_column,
                                window,
                                cx,
                            );
                        }
                    }
                    Err(error) => Self::show_error("Could not open file", error, window, cx),
                })
                .ok()
        })
        .detach();
    }

    fn show_overlay(&mut self, mode: OverlayMode, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = self.workspace_search.take() {
            search.cancel.cancel();
        }
        self.overlay_mode = Some(mode);
        self.overlay_items.clear();
        let placeholder = match mode {
            OverlayMode::Commands => "Type a command",
            OverlayMode::QuickOpen => "Quick open a project file",
            OverlayMode::WorkspaceSearch => "Search text in the workspace",
        };
        self.overlay_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
            input.focus(window, cx);
        });
        self.refresh_overlay(window, cx);
        cx.notify();
    }

    fn hide_overlay(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.workspace_search.take() {
            search.cancel.cancel();
        }
        self.overlay_mode = None;
        self.overlay_items.clear();
        cx.notify();
    }

    fn refresh_overlay(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.overlay_mode else {
            return;
        };
        let query = self.overlay_input.read(cx).value().trim().to_lowercase();
        match mode {
            OverlayMode::Commands => {
                self.overlay_items = command_items()
                    .into_iter()
                    .filter(|item| {
                        query.is_empty()
                            || item.title.to_lowercase().contains(&query)
                            || item.subtitle.to_lowercase().contains(&query)
                    })
                    .collect();
            }
            OverlayMode::QuickOpen => {
                self.overlay_items = self
                    .project
                    .as_ref()
                    .map(|project| {
                        project
                            .quick_open(&query, self.settings.workspace.quick_open_results)
                            .into_iter()
                            .map(|path| OverlayItem {
                                title: path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("File")
                                    .to_owned(),
                                subtitle: path
                                    .strip_prefix(&project.root)
                                    .unwrap_or(&path)
                                    .display()
                                    .to_string(),
                                target: OverlayTarget::File(path),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            OverlayMode::WorkspaceSearch => self.start_workspace_search(query, cx),
        }
        cx.notify();
    }

    fn start_workspace_search(&mut self, query: String, cx: &mut Context<Self>) {
        if let Some(search) = self.workspace_search.take() {
            search.cancel.cancel();
        }
        self.overlay_items.clear();
        let Some(project) = &self.project else {
            self.status_message = Some("Open a folder before searching the workspace".to_owned());
            return;
        };
        if query.is_empty() {
            self.status_message = Some("Type to search the workspace".to_owned());
            return;
        }
        let files = project.files.clone();
        let max_file_bytes = self.settings.workspace.search_max_file_bytes;
        let max_matches = self.settings.workspace.search_max_matches;
        let cancel = crate::huge_file::CancellationToken::default();
        let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        self.workspace_search = Some(WorkspaceSearchStream { cancel, receiver });
        self.status_message = Some(format!("Searching for “{query}”…"));
        cx.background_spawn(async move {
            let summary = stream_workspace_search(
                &files,
                &query,
                max_file_bytes,
                max_matches,
                &worker_cancel,
                |item| sender.send(WorkspaceSearchEvent::Match(item)).is_ok(),
            );
            let _ = sender.send(WorkspaceSearchEvent::Finished(summary));
        })
        .detach();
    }

    fn poll_workspace_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &self.workspace_search else {
            return;
        };
        let events = search.receiver.try_iter().collect::<Vec<_>>();
        let mut finished = false;
        for event in events {
            match event {
                WorkspaceSearchEvent::Match(item) => {
                    let subtitle = format!(
                        "{}:{}:{}",
                        item.path.display(),
                        item.line + 1,
                        item.column + 1
                    );
                    self.overlay_items.push(OverlayItem {
                        title: item.preview.clone(),
                        subtitle,
                        target: OverlayTarget::Search(item),
                    });
                }
                WorkspaceSearchEvent::Finished(summary) => {
                    finished = true;
                    self.status_message = Some(if summary.completed {
                        format!(
                            "{} matches across {} files",
                            summary.matches, summary.files_scanned
                        )
                    } else {
                        format!("Search stopped after {} matches", summary.matches)
                    });
                }
            }
        }
        if finished {
            self.workspace_search = None;
        }
        if !self.overlay_items.is_empty() || finished {
            cx.notify();
        }
    }

    fn accept_overlay(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.overlay_items.get(index).cloned() else {
            return;
        };
        let search_len = self.overlay_input.read(cx).value().trim().chars().count();
        self.hide_overlay(cx);
        match item.target {
            OverlayTarget::Command(command) => self.run_ide_command(command, window, cx),
            OverlayTarget::File(path) => self.open_path_at(path, None, window, cx),
            OverlayTarget::Search(item) => self.open_path_at(
                item.path,
                Some(TextLocation {
                    line: item.line,
                    column: item.column,
                    end_line: item.line,
                    end_column: item.column + search_len,
                }),
                window,
                cx,
            ),
        }
    }

    fn run_ide_command(
        &mut self,
        command: IdeCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            IdeCommand::NewFile => self.on_new(&NewDocument, window, cx),
            IdeCommand::OpenFile => self.on_open(&OpenDocument, window, cx),
            IdeCommand::OpenFolder => self.on_open_folder(&OpenFolder, window, cx),
            IdeCommand::QuickOpen => self.on_quick_open(&ShowQuickOpen, window, cx),
            IdeCommand::WorkspaceSearch => {
                self.on_workspace_search(&ShowWorkspaceSearch, window, cx)
            }
            IdeCommand::ToggleSidebar => self.on_toggle_sidebar(&ToggleSidebar, window, cx),
            IdeCommand::RefreshProject => self.refresh_project(window, cx),
            IdeCommand::OpenSettings => self.show_settings(window, cx),
            IdeCommand::OpenKeymap => self.open_config_file("keymap.json", window, cx),
            IdeCommand::GoToDefinition => self.on_go_to_definition(&GoToDefinition, window, cx),
        }
    }

    fn open_config_file(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = ensure_config_files(&self.data_dir) {
            Self::show_error("Could not prepare Textify settings", error, window, cx);
            return;
        }
        self.open_path_at(self.data_dir.join(name), None, window, cx);
    }

    fn on_command_palette(
        &mut self,
        _: &ShowCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_overlay(OverlayMode::Commands, window, cx);
    }

    fn on_show_settings(&mut self, _: &ShowSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings(window, cx);
    }

    fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_overlay(cx);
        self.settings_visible = true;
        self.settings_draft = Some(SettingsDraft {
            font_size: self.settings.appearance.font_size,
            recovery: self.settings.recovery.clone(),
        });
        let font_family = self.settings.appearance.font_family.clone();
        let recovery_directory = self.settings.recovery.directory(&self.data_dir);
        self.settings_font_input.update(cx, |input, cx| {
            input.set_value(font_family, window, cx);
        });
        self.settings_location_input.update(cx, |input, cx| {
            input.set_value(recovery_directory.display().to_string(), window, cx);
        });
        self.settings_font_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn hide_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_visible = false;
        self.settings_draft = None;
        if self.active_document().huge_viewer.is_none() {
            self.active_document().editor.focus(window, cx);
        }
        cx.notify();
    }

    fn choose_recovery_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Textify Recovery Folder".into()),
        });
        cx.spawn_in(window, async move |workspace, window| {
            let path = receiver.await.ok()?.ok()??.into_iter().next()?;
            workspace
                .update_in(window, |workspace, window, cx| {
                    if let Some(draft) = &mut workspace.settings_draft {
                        draft.recovery.temporary_files_location = Some(path.clone());
                    }
                    workspace.settings_location_input.update(cx, |input, cx| {
                        input.set_value(path.display().to_string(), window, cx)
                    });
                    cx.notify();
                })
                .ok()
        })
        .detach();
    }

    fn save_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut draft) = self.settings_draft.clone() else {
            return;
        };
        let font_family = self.settings_font_input.read(cx).value().to_string();
        let location = self
            .settings_location_input
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let default_location = self.data_dir.join("Backups");
        draft.recovery.temporary_files_location =
            if location.is_empty() || Path::new(&location) == default_location.as_path() {
                None
            } else {
                Some(PathBuf::from(location))
            };

        let mut settings = self.settings.clone();
        settings.appearance = AppearanceSettings {
            font_family,
            font_size: draft.font_size,
        };
        settings.appearance.normalize();
        settings.recovery = draft.recovery;
        let path = self.data_dir.join("settings.json");
        let task = cx.background_spawn({
            let settings = settings.clone();
            async move {
                fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))?;
                settings.save(&path)
            }
        });
        cx.spawn_in(window, async move |workspace, window| {
            let result = task.await;
            workspace
                .update_in(window, |workspace, window, cx| match result {
                    Ok(()) => {
                        let lsp_changed = workspace.settings.lsp != settings.lsp;
                        workspace.settings = settings;
                        for document in &workspace.documents {
                            document.editor.set_budgets(
                                workspace.settings.editor,
                                document.metadata.mode,
                                cx,
                            );
                        }
                        let ids = workspace
                            .documents
                            .iter()
                            .map(|document| document.id)
                            .collect::<Vec<_>>();
                        for id in ids {
                            let Some(index) = workspace.document_index(id) else {
                                continue;
                            };
                            let enabled = workspace
                                .settings
                                .recovery
                                .enabled_for(workspace.documents[index].metadata.path.is_some());
                            if enabled && workspace.documents[index].dirty {
                                workspace.recovery_pending.insert(id, Instant::now());
                            } else if !enabled {
                                workspace.clear_recovery(id, cx);
                            }
                        }
                        if lsp_changed {
                            workspace.restart_lsp(window, cx);
                        }
                        workspace.persist_session(cx);
                        workspace.status_message = Some("Settings saved".to_owned());
                        workspace.hide_settings(window, cx);
                    }
                    Err(error) => {
                        workspace.status_message = Some(error.to_string());
                        Self::show_error("Could not save settings", error, window, cx);
                    }
                })
                .ok()
        })
        .detach();
    }

    fn on_quick_open(&mut self, _: &ShowQuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        self.show_overlay(OverlayMode::QuickOpen, window, cx);
    }

    fn on_workspace_search(
        &mut self,
        _: &ShowWorkspaceSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_overlay(OverlayMode::WorkspaceSearch, window, cx);
    }

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn on_dismiss_overlay(
        &mut self,
        _: &DismissOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_visible {
            self.hide_settings(window, cx);
        } else if self.overlay_mode.is_some() {
            self.hide_overlay(cx);
        } else {
            cx.propagate();
        }
    }

    fn reload_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_reload_in_progress {
            return;
        }
        self.config_reload_in_progress = true;
        let settings_path = self.data_dir.join("settings.json");
        let keymap_path = self.data_dir.join("keymap.json");
        let task = cx.background_spawn(async move {
            (
                TextifySettings::load(&settings_path),
                TextifyKeymap::load(&keymap_path),
            )
        });
        cx.spawn_in(window, async move |workspace, window| {
            let (settings, keymap) = task.await;
            workspace
                .update_in(window, |workspace, window, cx| {
                    workspace.config_reload_in_progress = false;
                    let mut changed = false;
                    match settings {
                        Ok(settings) => {
                            let lsp_changed = workspace.settings.lsp != settings.lsp;
                            workspace.settings = settings;
                            for document in &workspace.documents {
                                document.editor.set_budgets(
                                    workspace.settings.editor,
                                    document.metadata.mode,
                                    cx,
                                );
                            }
                            if lsp_changed {
                                workspace.restart_lsp(window, cx);
                            }
                            changed = true;
                        }
                        Err(error) => {
                            workspace.status_message = Some(error.to_string());
                            tracing::warn!(%error, "settings reload failed");
                        }
                    }
                    match keymap {
                        Ok(keymap) => {
                            workspace.keymap = keymap;
                            bind_ide_keymap(cx, &workspace.keymap);
                            changed = true;
                        }
                        Err(error) => {
                            workspace.status_message = Some(error.to_string());
                            tracing::warn!(%error, "keymap reload failed");
                        }
                    }
                    if changed {
                        workspace.status_message = Some("Settings and keymap reloaded".to_owned());
                    }
                    cx.notify();
                })
                .ok()
        })
        .detach();
    }

    fn restart_lsp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lsp = None;
        self.lsp_opened.clear();
        self.pending_definitions.clear();
        self.lsp_dirty.clear();
        if !self.settings.lsp.enabled || self.settings.lsp.command.is_empty() || self.lsp_starting {
            return;
        }
        let Some(root) = self.project.as_ref().map(|project| project.root.clone()) else {
            return;
        };
        let command = self.settings.lsp.command.clone();
        self.lsp_starting = true;
        self.status_message = Some("Starting language server…".to_owned());
        let task = cx.background_spawn(async move { LspClient::start(&command, &root) });
        cx.spawn_in(window, async move |workspace, window| {
            let result = task.await;
            workspace
                .update_in(window, |workspace, _, cx| {
                    workspace.lsp_starting = false;
                    match result {
                        Ok(client) => {
                            workspace.lsp = Some(client);
                            workspace.status_message =
                                Some("Language server initializing…".to_owned());
                        }
                        Err(error) => {
                            workspace.status_message = Some(error.to_string());
                            tracing::warn!(%error, "language server failed to start");
                        }
                    }
                    cx.notify();
                })
                .ok()
        })
        .detach();
    }

    fn lsp_accepts(&self, document: &EditorDocument) -> bool {
        if document.huge_viewer.is_some()
            || document.metadata.analysis.bytes > self.settings.lsp.max_document_bytes
        {
            return false;
        }
        document
            .metadata
            .path
            .as_ref()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                self.settings
                    .lsp
                    .file_extensions
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            })
    }

    fn poll_lsp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let events = self
            .lsp
            .as_ref()
            .map(|client| client.drain_events().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut failed = false;
        for event in events {
            match event {
                LspEvent::Diagnostics { path, items } => {
                    if let Some(document) = self
                        .documents
                        .iter()
                        .find(|document| document.metadata.path.as_deref() == Some(path.as_path()))
                    {
                        document.editor.set_diagnostics(items, cx);
                    }
                }
                LspEvent::Response { id, result } => {
                    let initialize = self.lsp.as_ref().is_some_and(|client| {
                        id == client.initialize_id() && !client.is_initialized()
                    });
                    if initialize {
                        if let Some(client) = &mut self.lsp {
                            match client.finish_initialize() {
                                Ok(()) => {
                                    self.status_message = Some("Language server ready".to_owned())
                                }
                                Err(error) => {
                                    self.status_message = Some(error.to_string());
                                    failed = true;
                                }
                            }
                        }
                    } else if self.pending_definitions.remove(&id) {
                        if let Some(location) =
                            parse_definition_locations(&result).into_iter().next()
                        {
                            self.open_definition(location, window, cx);
                        } else {
                            self.status_message = Some("No definition found".to_owned());
                        }
                    }
                }
                LspEvent::ServerRequest { id, method, params } => {
                    let result = if method == "workspace/configuration" {
                        let count = params
                            .get("items")
                            .and_then(serde_json::Value::as_array)
                            .map_or(0, Vec::len);
                        serde_json::Value::Array(vec![serde_json::Value::Null; count])
                    } else if method == "workspace/workspaceFolders" {
                        serde_json::Value::Array(Vec::new())
                    } else {
                        serde_json::Value::Null
                    };
                    if let Some(client) = &self.lsp
                        && let Err(error) = client.respond(id, result)
                    {
                        tracing::warn!(%error, %method, "could not answer language-server request");
                    }
                }
                LspEvent::Failed(message) => {
                    self.status_message = Some(message);
                    failed = true;
                }
            }
        }
        if failed {
            self.lsp = None;
            self.lsp_opened.clear();
            return;
        }
        self.sync_lsp_documents(cx);
    }

    fn sync_lsp_documents(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.lsp.as_ref().filter(|client| client.is_initialized()) else {
            return;
        };
        let new_documents = self
            .documents
            .iter()
            .filter(|document| self.lsp_accepts(document))
            .filter_map(|document| {
                let path = document.metadata.path.clone()?;
                (!self.lsp_opened.contains(&path)).then(|| {
                    let rope = document.editor.rope(cx);
                    let text = (rope.len() as u64 <= self.settings.lsp.max_document_bytes)
                        .then(|| rope.to_string());
                    let language = path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .unwrap_or("plaintext")
                        .to_owned();
                    (path, language, document.revision, text)
                })
            })
            .filter_map(|(path, language, version, text)| {
                text.map(|text| (path, language, version, text))
            })
            .collect::<Vec<_>>();
        for (path, language, version, text) in new_documents {
            match client.did_open(&path, &language, version, &text) {
                Ok(()) => {
                    self.lsp_opened.insert(path);
                }
                Err(error) => tracing::warn!(%error, "could not notify LSP of open document"),
            }
        }

        let ready = self
            .lsp_dirty
            .iter()
            .filter(|(_, changed_at)| changed_at.elapsed() >= Duration::from_millis(300))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in ready {
            self.lsp_dirty.remove(&id);
            let Some(document) = self.documents.iter().find(|document| document.id == id) else {
                continue;
            };
            if !self.lsp_accepts(document) {
                continue;
            }
            let Some(path) = document.metadata.path.as_ref() else {
                continue;
            };
            let rope = document.editor.rope(cx);
            if rope.len() as u64 > self.settings.lsp.max_document_bytes {
                continue;
            }
            let text = rope.to_string();
            if let Err(error) = client.did_change(path, document.revision, &text) {
                tracing::warn!(%error, "could not notify LSP of document change");
            }
        }
    }

    fn on_go_to_definition(&mut self, _: &GoToDefinition, _: &mut Window, cx: &mut Context<Self>) {
        let document = self.active_document();
        let Some(path) = document.metadata.path.clone() else {
            self.status_message =
                Some("Save the document before requesting a definition".to_owned());
            cx.notify();
            return;
        };
        let cursor = document.editor.state().read(cx).cursor_position();
        let Some(client) = &mut self.lsp else {
            self.status_message = Some("No language server is configured".to_owned());
            cx.notify();
            return;
        };
        if !client.is_initialized() {
            self.status_message = Some("Language server is still initializing".to_owned());
            cx.notify();
            return;
        }
        match client.definition(&path, cursor.line as usize, cursor.character as usize) {
            Ok(id) => {
                self.pending_definitions.insert(id);
                self.status_message = Some("Finding definition…".to_owned());
            }
            Err(error) => self.status_message = Some(error.to_string()),
        }
        cx.notify();
    }

    fn open_definition(
        &mut self,
        location: DefinitionLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_path_at(
            location.path,
            Some(TextLocation {
                line: location.start_line,
                column: location.start_character,
                end_line: location.end_line,
                end_column: location.end_character,
            }),
            window,
            cx,
        );
    }

    fn on_new(&mut self, _: &NewDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.add_untitled(window, cx);
        self.persist_session(cx);
    }

    fn on_open(&mut self, _: &OpenDocument, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open in Textify".into()),
        });
        let policy = self.policy;

        cx.spawn_in(window, async move |workspace, window| {
            let path = receiver.await.ok()?.ok()??.into_iter().next()?;
            let started_at = Instant::now();
            let load_path = path.clone();
            let task = window.background_spawn(async move { open_file(&load_path, policy) });
            let loaded = task.await;
            let elapsed = started_at.elapsed();

            workspace
                .update_in(window, |workspace, window, cx| match loaded {
                    Ok(opened) => {
                        tracing::info!(
                            path = %path.display(),
                            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
                            "opened document"
                        );
                        workspace.push_opened(opened, window, cx)
                    }
                    Err(error) => Self::show_error("Could not open file", error, window, cx),
                })
                .ok()
        })
        .detach();
    }

    fn on_save(&mut self, _: &SaveDocument, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_document().huge_viewer.is_some() {
            Self::show_error(
                "Huge file viewer is read-only",
                anyhow::anyhow!("Select lines and use Edit Selection to open an editable copy."),
                window,
                cx,
            );
            return;
        }
        let id = self.active_id();
        if let Some(path) = self.active_document().metadata.path.clone() {
            self.start_save(id, path, window, cx);
        } else {
            self.prompt_save_as(id, window, cx);
        }
    }

    fn on_save_as(&mut self, _: &SaveDocumentAs, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_document().huge_viewer.is_some() {
            self.on_save(&SaveDocument, window, cx);
            return;
        }
        self.prompt_save_as(self.active_id(), window, cx);
    }

    fn prompt_save_as(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        let document = &self.documents[index];
        let directory = document
            .metadata
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = if document.metadata.path.is_some() {
            document.display_name()
        } else {
            suggested_save_path(&directory, document.untitled_number)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Untitled.txt")
                .to_owned()
        };
        let receiver = cx.prompt_for_new_path(&directory, Some(&suggested));

        cx.spawn_in(window, async move |workspace, window| {
            let path = receiver.await.ok().into_iter().flatten().flatten().next()?;
            workspace
                .update_in(window, |workspace, window, cx| {
                    workspace.start_save(id, path, window, cx)
                })
                .ok()
        })
        .detach();
    }

    fn start_save(&mut self, id: u64, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        let document = &mut self.documents[index];
        if document.saving {
            return;
        }

        document.saving = true;
        let revision = document.revision;
        let rope = document.editor.rope(cx);
        let expected = (document.metadata.path.as_deref() == Some(path.as_path()))
            .then(|| document.disk_revision.clone())
            .flatten();
        let started_at = Instant::now();
        let task = cx.background_spawn(async move {
            let analysis = FileAnalysis::from_str_chunks(rope.chunks());
            let result = save_atomic_chunks_checked(&path, rope.chunks(), expected.as_ref());
            (path, analysis, result, started_at.elapsed())
        });
        let policy = self.policy;
        cx.notify();

        cx.spawn_in(window, async move |workspace, window| {
            let (path, analysis, result, elapsed) = task.await;
            workspace
                .update_in(window, |workspace, window, cx| match result {
                    Ok(disk_revision) => {
                        let Some(index) = workspace.document_index(id) else {
                            return;
                        };
                        let metadata =
                            DocumentMetadata::new(Some(path.clone()), analysis, workspace.policy);
                        let parser = metadata.parser_name(policy);
                        let editor = workspace.documents[index].editor.clone();
                        let document = &mut workspace.documents[index];
                        document.metadata = metadata;
                        document.disk_revision = Some(disk_revision);
                        document.external_changed = false;
                        document.saving = false;
                        let saved_clean = document.revision == revision;
                        if saved_clean {
                            document.dirty = false;
                        }
                        editor.set_parser(parser, cx);
                        if let Some(watcher) = &mut workspace.watcher
                            && let Err(error) = watcher.watch_file(&path)
                        {
                            tracing::warn!(%error, "could not watch saved file");
                        }
                        tracing::info!(
                            path = %path.display(),
                            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
                            "saved document"
                        );
                        workspace.update_window_title(window);
                        if saved_clean {
                            workspace.clear_recovery(id, cx);
                        }
                        workspace.persist_session(cx);
                        workspace.refresh_git(window, cx);
                        cx.notify();
                    }
                    Err(error) => {
                        if let Some(index) = workspace.document_index(id) {
                            workspace.documents[index].saving = false;
                        }
                        if let Some(conflict) = error.downcast_ref::<ExternalFileChanged>() {
                            workspace.notice_external_revision(
                                id,
                                conflict.actual.clone(),
                                window,
                                cx,
                            );
                        } else {
                            Self::show_error("Could not save file", error, window, cx);
                        }
                        cx.notify();
                    }
                })
                .ok()
        })
        .detach();
    }

    fn on_close(&mut self, _: &CloseDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(self.active_id(), window, cx);
    }

    fn request_close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            return;
        };

        if !self.documents[index].dirty {
            self.remove_document(id, window, cx);
            return;
        }

        let name = self.documents[index].display_name();
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let workspace = workspace.clone();
            dialog
                .title(format!("Discard changes to {name}?"))
                .child("Your unsaved changes cannot be recovered.")
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Discard")
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("Keep Editing"),
                )
                .confirm()
                .on_ok(move |_, window, cx| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.remove_document(id, window, cx)
                    });
                    true
                })
        });
    }

    fn remove_document(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        let removed = self.documents.remove(index);
        self.recovery_pending.remove(&id);
        if let Some(path) = removed.recovery_path.clone() {
            cx.background_spawn(async move {
                if let Err(error) = remove_snapshot(&path) {
                    tracing::warn!(%error, "could not remove discarded recovery copy");
                }
            })
            .detach();
        }
        if let Some(path) = removed.metadata.path
            && self.lsp_opened.remove(&path)
            && let Some(client) = &self.lsp
            && let Err(error) = client.did_close(&path)
        {
            tracing::warn!(%error, "could not notify LSP of closed document");
        }

        if self.documents.is_empty() {
            self.active_index = 0;
            self.add_untitled(window, cx);
            return;
        }

        if index < self.active_index {
            self.active_index -= 1;
        } else if index == self.active_index {
            self.active_index = self.active_index.min(self.documents.len() - 1);
        }

        if self.active_document().huge_viewer.is_none() {
            self.active_document().editor.focus(window, cx);
        }
        self.update_window_title(window);
        self.persist_session(cx);
        cx.notify();
    }

    fn on_next(&mut self, _: &NextDocument, window: &mut Window, cx: &mut Context<Self>) {
        let next = (self.active_index + 1) % self.documents.len();
        self.set_active_index(next, window, cx);
    }

    fn on_previous(&mut self, _: &PreviousDocument, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self
            .active_index
            .checked_sub(1)
            .unwrap_or(self.documents.len() - 1);
        self.set_active_index(previous, window, cx);
    }

    fn show_error(title: &'static str, error: anyhow::Error, window: &mut Window, cx: &mut App) {
        let message = error.to_string();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(title)
                .child(message.clone())
                .button_props(DialogButtonProps::default().ok_text("OK"))
                .alert()
        });
    }

    fn render_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = cx.entity();
        let tabs = self
            .documents
            .iter()
            .map(|document| {
                let id = document.id;
                let close_workspace = workspace.clone();
                Tab::new().label(document.title()).suffix(
                    Button::new(("close-tab", id))
                        .ghost()
                        .xsmall()
                        .compact()
                        .icon(IconName::Close)
                        .tooltip("Close")
                        .on_click(move |_, window, cx| {
                            close_workspace.update(cx, |workspace, cx| {
                                workspace.request_close(id, window, cx)
                            });
                        }),
                )
            })
            .collect::<Vec<_>>();

        TabBar::new("document-tabs")
            .outline()
            .w_full()
            .bg(cx.theme().tab_bar)
            .border_b_1()
            .border_color(cx.theme().border)
            .selected_index(self.active_index)
            .on_click(cx.listener(|workspace, index: &usize, window, cx| {
                workspace.set_active_index(*index, window, cx);
            }))
            .prefix(
                h_flex()
                    .px_1()
                    .gap_1()
                    .child(
                        Button::new("new-document")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip_with_action("New file", &NewDocument, None)
                            .on_click(cx.listener(|workspace, _, window, cx| {
                                workspace.add_untitled(window, cx);
                                workspace.persist_session(cx);
                            })),
                    )
                    .child(
                        Button::new("open-document")
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderOpen)
                            .tooltip_with_action("Open file", &OpenDocument, None)
                            .on_click(cx.listener(|workspace, _, window, cx| {
                                workspace.on_open(&OpenDocument, window, cx)
                            })),
                    )
                    .child(
                        Button::new("open-folder")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Folder)
                            .tooltip_with_action("Open folder", &OpenFolder, None)
                            .on_click(cx.listener(|workspace, _, window, cx| {
                                workspace.on_open_folder(&OpenFolder, window, cx)
                            })),
                    )
                    .child(
                        Button::new("quick-open")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Search)
                            .tooltip_with_action("Quick open", &ShowQuickOpen, None)
                            .on_click(cx.listener(|workspace, _, window, cx| {
                                workspace.on_quick_open(&ShowQuickOpen, window, cx)
                            })),
                    ),
            )
            .children(tabs)
            .suffix(
                Button::new("save-document")
                    .ghost()
                    .small()
                    .compact()
                    .icon(IconName::File)
                    .label(if self.active_document().saving {
                        "Saving…"
                    } else if self.active_document().huge_viewer.is_some() {
                        "Read Only"
                    } else {
                        "Save"
                    })
                    .loading(self.active_document().saving)
                    .disabled(self.active_document().huge_viewer.is_some())
                    .tooltip_with_action("Save file", &SaveDocument, None)
                    .on_click(cx.listener(|workspace, _, window, cx| {
                        workspace.on_save(&SaveDocument, window, cx)
                    })),
            )
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.active_document();
        let (line_summary, position_summary) = if let Some(viewer) = &document.huge_viewer {
            let viewer = viewer.read(cx);
            let lines = viewer
                .line_count()
                .map(|lines| format!("{lines} lines"))
                .unwrap_or_else(|| "Indexing lines…".to_owned());
            let range = viewer.visible_range();
            (lines, format!("Bytes {}–{}", range.start, range.end))
        } else {
            let input = document.editor.state().read(cx);
            let cursor = input.cursor_position();
            (
                format!("{} lines", input.text().lines_len()),
                format!("Ln {}, Col {}", cursor.line + 1, cursor.character + 1),
            )
        };
        let parser_suppressed = document.metadata.language != Language::PlainText
            && document.metadata.parser_name(self.policy).is_none();
        let path = document
            .metadata
            .path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Unsaved document".to_owned());

        let project_status = self
            .project
            .as_ref()
            .and_then(|project| project.root.file_name())
            .and_then(|name| name.to_str())
            .map(|name| format!("Folder: {name}"));
        h_flex()
            .h_7()
            .px_3()
            .gap_3()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().tab_bar)
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_3()
                    .when(document.metadata.mode != FileMode::Normal, |row| {
                        row.child(
                            div()
                                .px_2()
                                .py(px(1.))
                                .rounded_sm()
                                .bg(cx.theme().warning.opacity(0.16))
                                .text_color(cx.theme().warning)
                                .child(document.metadata.mode.label()),
                        )
                    })
                    .when(parser_suppressed, |row| {
                        row.child(
                            div()
                                .px_2()
                                .py(px(1.))
                                .rounded_sm()
                                .bg(cx.theme().warning.opacity(0.12))
                                .text_color(cx.theme().warning)
                                .child("PARSER OFF"),
                        )
                    })
                    .when(document.external_changed, |row| {
                        row.child(
                            div()
                                .px_2()
                                .py(px(1.))
                                .rounded_sm()
                                .bg(cx.theme().danger.opacity(0.14))
                                .text_color(cx.theme().danger)
                                .child("DISK CHANGED"),
                        )
                    })
                    .child(path),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_4()
                    .child(line_summary)
                    .child(position_summary)
                    .children(project_status)
                    .children(self.status_message.clone())
                    .child("UTF-8")
                    .child(document.metadata.analysis.line_ending.label())
                    .child(document.metadata.language.label()),
            )
    }

    fn render_project_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self
            .project
            .as_ref()
            .map(|project| project.entries.len())
            .unwrap_or(0);
        let root_label = self
            .project
            .as_ref()
            .map(|project| project.root.display().to_string())
            .unwrap_or_else(|| {
                if self.project_loading {
                    "Indexing folder…".to_owned()
                } else {
                    "No folder open".to_owned()
                }
            });
        v_flex()
            .w_64()
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h_9()
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .font_semibold()
                            .truncate()
                            .child(root_label),
                    )
                    .child(
                        Button::new("project-refresh")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Redo)
                            .tooltip("Refresh project and Git status")
                            .disabled(self.project.is_none() || self.project_loading)
                            .on_click(cx.listener(|workspace, _, window, cx| {
                                workspace.refresh_project(window, cx)
                            })),
                    )
                    .child(
                        Button::new("project-open-folder")
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderOpen)
                            .tooltip("Open Folder")
                            .on_click(cx.listener(|workspace, _, window, cx| {
                                workspace.on_open_folder(&OpenFolder, window, cx)
                            })),
                    ),
            )
            .child(
                uniform_list(
                    "project-explorer",
                    count,
                    cx.processor(|workspace, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|index| {
                                let project = workspace.project.as_ref()?;
                                let entry = project.entries.get(index)?.clone();
                                let relative = entry.relative.clone();
                                let status = workspace.git_status.get(&relative).cloned();
                                let name = entry
                                    .path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("File")
                                    .to_owned();
                                let path = entry.path.clone();
                                Some(
                                    h_flex()
                                        .id(("project-entry", index))
                                        .h_6()
                                        .px_2()
                                        .gap_1()
                                        .cursor_pointer()
                                        .hover(|row| row.bg(cx.theme().list_hover))
                                        .on_click(cx.listener(move |workspace, _, window, cx| {
                                            if !entry.is_directory {
                                                workspace.open_path_at(
                                                    path.clone(),
                                                    None,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .child(div().w(px((entry.depth * 12) as f32)))
                                        .child(if entry.is_directory { "▾" } else { "·" })
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_sm()
                                                .child(name),
                                        )
                                        .children(status.map(|status| {
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().warning)
                                                .child(status)
                                        })),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0(),
            )
    }

    fn render_overlay(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.overlay_items.len();
        let title = match self.overlay_mode {
            Some(OverlayMode::Commands) => "COMMAND PALETTE",
            Some(OverlayMode::QuickOpen) => "QUICK OPEN",
            Some(OverlayMode::WorkspaceSearch) => "WORKSPACE SEARCH",
            None => "",
        };
        v_flex()
            .absolute()
            .top(px(72.))
            .left(px(180.))
            .right(px(180.))
            .max_h(px(520.))
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(
                h_flex()
                    .h_8()
                    .px_3()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(title)
                    .child("Enter opens first result · Esc closes"),
            )
            .child(div().px_2().pb_2().child(Input::new(&self.overlay_input)))
            .child(
                uniform_list(
                    "ide-overlay-results",
                    count,
                    cx.processor(|workspace, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|index| {
                                let item = workspace.overlay_items.get(index)?.clone();
                                Some(
                                    v_flex()
                                        .id(("overlay-item", index))
                                        .min_h_10()
                                        .px_3()
                                        .py_1()
                                        .cursor_pointer()
                                        .border_t_1()
                                        .border_color(cx.theme().border.opacity(0.55))
                                        .hover(|row| row.bg(cx.theme().list_hover))
                                        .on_click(cx.listener(move |workspace, _, window, cx| {
                                            workspace.accept_overlay(index, window, cx)
                                        }))
                                        .child(div().text_sm().child(item.title))
                                        .when(!item.subtitle.is_empty(), |row| {
                                            row.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(item.subtitle),
                                            )
                                        }),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .max_h(px(430.)),
            )
    }

    fn render_settings_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let draft = self.settings_draft.clone().unwrap_or(SettingsDraft {
            font_size: self.settings.appearance.font_size,
            recovery: self.settings.recovery.clone(),
        });
        let temporary_enabled = draft.recovery.save_temporary_files;
        let unsaved_enabled = draft.recovery.keep_unsaved_changes;
        let temporary_workspace = cx.entity();
        let unsaved_workspace = cx.entity();

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .bg(cx.theme().background.opacity(0.78))
            .child(
                v_flex()
                    .absolute()
                    .top(px(52.))
                    .bottom(px(52.))
                    .left(px(150.))
                    .right(px(150.))
                    .max_h(px(680.))
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .h_12()
                            .px_5()
                            .justify_between()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(div().text_lg().font_semibold().child("Settings"))
                            .child(
                                Button::new("close-settings")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .tooltip("Close Settings")
                                    .on_click(cx.listener(|workspace, _, window, cx| {
                                        workspace.hide_settings(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .p_5()
                            .gap_5()
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(div().text_sm().font_semibold().child("Editor Font"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Applied to every tab unless that tab is zoomed."),
                                    )
                                    .child(Input::new(&self.settings_font_input).w_full()),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(div().text_sm().font_semibold().child("Text Size"))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Default editor size in points."),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                Button::new("settings-font-smaller")
                                                    .outline()
                                                    .label("−")
                                                    .on_click(cx.listener(
                                                        |workspace, _, _, cx| {
                                                            if let Some(draft) =
                                                                &mut workspace.settings_draft
                                                            {
                                                                draft.font_size = draft
                                                                    .font_size
                                                                    .saturating_sub(1)
                                                                    .max(AppearanceSettings::MIN_FONT_SIZE);
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .w_12()
                                                    .text_center()
                                                    .child(format!("{} pt", draft.font_size)),
                                            )
                                            .child(
                                                Button::new("settings-font-larger")
                                                    .outline()
                                                    .label("+")
                                                    .on_click(cx.listener(
                                                        |workspace, _, _, cx| {
                                                            if let Some(draft) =
                                                                &mut workspace.settings_draft
                                                            {
                                                                draft.font_size = (draft.font_size + 1)
                                                                    .min(AppearanceSettings::MAX_FONT_SIZE);
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .child("Save Temporary Files"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Continuously recover Untitled tabs after a crash or force quit."),
                                            ),
                                    )
                                    .child(
                                        Switch::new("settings-save-temporary")
                                            .checked(temporary_enabled)
                                            .on_click(move |checked, _, cx| {
                                                temporary_workspace.update(cx, |workspace, cx| {
                                                    if let Some(draft) =
                                                        &mut workspace.settings_draft
                                                    {
                                                        draft.recovery.save_temporary_files =
                                                            *checked;
                                                    }
                                                    cx.notify();
                                                });
                                            }),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .child("Keep Unsaved Changes"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Recover edits to named files without overwriting the originals."),
                                            ),
                                    )
                                    .child(
                                        Switch::new("settings-keep-unsaved")
                                            .checked(unsaved_enabled)
                                            .on_click(move |checked, _, cx| {
                                                unsaved_workspace.update(cx, |workspace, cx| {
                                                    if let Some(draft) =
                                                        &mut workspace.settings_draft
                                                    {
                                                        draft.recovery.keep_unsaved_changes =
                                                            *checked;
                                                    }
                                                    cx.notify();
                                                });
                                            }),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .child("Temporary Files Location"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Recovery copies stay local and are removed after save or discard."),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                Input::new(&self.settings_location_input)
                                                    .flex_1()
                                                    .min_w_0(),
                                            )
                                            .child(
                                                Button::new("choose-recovery-folder")
                                                    .outline()
                                                    .label("Choose…")
                                                    .on_click(cx.listener(
                                                        |workspace, _, window, cx| {
                                                            workspace.choose_recovery_directory(
                                                                window, cx,
                                                            )
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .h(px(56.))
                            .px_5()
                            .gap_2()
                            .justify_end()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new("cancel-settings")
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(cx.listener(|workspace, _, window, cx| {
                                        workspace.hide_settings(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("save-settings")
                                    .primary()
                                    .label("Save Settings")
                                    .on_click(cx.listener(|workspace, _, window, cx| {
                                        workspace.save_settings_window(window, cx)
                                    })),
                            ),
                    ),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_paint_logged {
            self.first_paint_logged = true;
            tracing::info!(
                elapsed_ms = self.created_at.elapsed().as_secs_f64() * 1000.0,
                "first workspace paint"
            );
        }
        let editor = self.active_document().editor.clone();
        let content = self
            .active_document()
            .huge_viewer
            .clone()
            .map(|viewer| viewer.into_any_element())
            .unwrap_or_else(|| {
                editor
                    .render(
                        &self.settings.appearance.font_family,
                        self.settings.appearance.font_size,
                        cx,
                    )
                    .size_full()
                    .into_any_element()
            });

        let workspace = v_flex()
            .id("textify-workspace")
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_new))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_as))
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_previous))
            .on_action(cx.listener(Self::on_open_folder))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_command_palette))
            .on_action(cx.listener(Self::on_quick_open))
            .on_action(cx.listener(Self::on_workspace_search))
            .on_action(cx.listener(Self::on_show_settings))
            .on_action(cx.listener(Self::on_go_to_definition))
            .on_action(cx.listener(Self::on_dismiss_overlay))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .pr_3()
                        .items_center()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().size_2().rounded_full().bg(cx.theme().primary))
                                .child(div().text_sm().font_semibold().child("TEXTIFY"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("IDE"),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("A fast place for text"),
                        ),
                ),
            )
            .child(self.render_tabs(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.sidebar_visible, |body| {
                        body.child(self.render_project_sidebar(cx))
                    })
                    .child(
                        div()
                            .id("editor-surface")
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .overflow_hidden()
                            .bg(cx.theme().background)
                            .child(content),
                    ),
            )
            .child(self.render_status(cx));

        div()
            .relative()
            .size_full()
            .child(workspace)
            .when(self.overlay_mode.is_some(), |root| {
                root.child(self.render_overlay(cx))
            })
            .when(self.settings_visible, |root| {
                root.child(self.render_settings_panel(cx))
            })
    }
}

fn command_items() -> Vec<OverlayItem> {
    [
        ("New File", "Create an untitled editor", IdeCommand::NewFile),
        ("Open File…", "Open a file from disk", IdeCommand::OpenFile),
        (
            "Open Folder…",
            "Index a project lazily",
            IdeCommand::OpenFolder,
        ),
        (
            "Quick Open",
            "Find an indexed project file",
            IdeCommand::QuickOpen,
        ),
        (
            "Search Workspace",
            "Stream text matches from project files",
            IdeCommand::WorkspaceSearch,
        ),
        (
            "Toggle File Explorer",
            "Show or hide the project sidebar",
            IdeCommand::ToggleSidebar,
        ),
        (
            "Refresh Project",
            "Rescan files and Git decorations",
            IdeCommand::RefreshProject,
        ),
        (
            "Open Settings",
            "Change fonts and crash recovery",
            IdeCommand::OpenSettings,
        ),
        (
            "Open Keymap",
            "Edit keymap.json (reloads on save)",
            IdeCommand::OpenKeymap,
        ),
        (
            "Go to Definition",
            "Ask the configured language server",
            IdeCommand::GoToDefinition,
        ),
    ]
    .into_iter()
    .map(|(title, subtitle, command)| OverlayItem {
        title: title.to_owned(),
        subtitle: subtitle.to_owned(),
        target: OverlayTarget::Command(command),
    })
    .collect()
}

fn push_key_binding<A: gpui::Action>(bindings: &mut Vec<KeyBinding>, shortcut: &str, action: A) {
    if !shortcut.trim().is_empty()
        && shortcut
            .split_whitespace()
            .all(|keystroke| gpui::Keystroke::parse(keystroke).is_ok())
    {
        bindings.push(KeyBinding::new(shortcut, action, None));
    } else {
        tracing::warn!(shortcut, "ignoring invalid Textify key binding");
    }
}

fn bind_ide_keymap(cx: &mut App, keymap: &TextifyKeymap) {
    let mut bindings = Vec::new();
    push_key_binding(&mut bindings, &keymap.command_palette, ShowCommandPalette);
    push_key_binding(&mut bindings, &keymap.quick_open, ShowQuickOpen);
    push_key_binding(&mut bindings, &keymap.workspace_search, ShowWorkspaceSearch);
    push_key_binding(&mut bindings, &keymap.open_folder, OpenFolder);
    push_key_binding(&mut bindings, &keymap.toggle_sidebar, ToggleSidebar);
    push_key_binding(&mut bindings, &keymap.go_to_definition, GoToDefinition);
    cx.bind_keys(bindings);
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "textify=info".into()),
        )
        .with_target(false)
        .compact()
        .init();

    Application::new().with_assets(Assets).run(|cx| {
        gpui_component::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-n", NewDocument, None),
            KeyBinding::new("cmd-o", OpenDocument, None),
            KeyBinding::new("cmd-s", SaveDocument, None),
            KeyBinding::new("cmd-shift-s", SaveDocumentAs, None),
            KeyBinding::new("cmd-w", CloseDocument, None),
            KeyBinding::new("cmd-,", ShowSettings, None),
            KeyBinding::new("ctrl-tab", NextDocument, None),
            KeyBinding::new("ctrl-shift-tab", PreviousDocument, None),
            KeyBinding::new("escape", DismissOverlay, None),
        ]);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1180.), px(780.)), cx)),
            window_min_size: Some(size(px(680.), px(420.))),
            titlebar: Some(TitleBar::title_bar_options()),
            ..Default::default()
        };

        cx.spawn(async move |cx| {
            cx.open_window(options, |window, cx| {
                window.activate_window();
                window.set_window_title(WINDOW_TITLE);
                Theme::change(ThemeMode::Dark, Some(window), cx);
                let workspace = cx.new(|cx| Workspace::new(window, cx));
                let services = workspace.clone();
                window.defer(cx, move |window, cx| {
                    services.update(cx, |workspace, cx| {
                        workspace.start_background_services(window, cx)
                    });
                });
                cx.new(|cx| Root::new(workspace, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn workspace_renders_lazy_ide_shell(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let workspace_slot = Rc::new(RefCell::new(None));
        let capture = workspace_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *capture.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = workspace_slot.borrow().clone().expect("workspace");
        workspace.update(cx, |workspace, _| {
            assert_eq!(workspace.documents.len(), 1);
            assert!(workspace.project.is_none());
            assert!(workspace.lsp.is_none());
        });

        let directory = tempfile::tempdir().expect("project");
        fs::write(directory.path().join("main.rs"), "fn main() {}\n").expect("fixture");
        let project = ProjectIndex::build(directory.path(), 100).expect("index");
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.project = Some(project);
                workspace.sidebar_visible = true;
                workspace.show_overlay(OverlayMode::Commands, window, cx);
            });
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, _| {
            assert!(!workspace.overlay_items.is_empty());
            assert_eq!(workspace.project.as_ref().unwrap().files.len(), 1);
        });
    }

    #[test]
    fn open_file_routes_at_the_huge_file_threshold() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let small = directory.path().join("small.txt");
        let huge = directory.path().join("huge.txt");
        fs::write(&small, vec![b'x'; 63]).expect("small fixture");
        fs::write(&huge, vec![b'x'; 64]).expect("huge fixture");
        let policy = FilePolicy {
            huge_file_bytes: 64,
            ..FilePolicy::default()
        };

        assert!(matches!(
            open_file(&small, policy).expect("small"),
            OpenedFile::Editable(_)
        ));
        assert!(matches!(
            open_file(&huge, policy).expect("huge"),
            OpenedFile::Huge { .. }
        ));
    }

    #[test]
    fn recovered_named_file_keeps_disk_identity_and_draft_text() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("notes.md");
        fs::write(&path, "saved\n").expect("saved fixture");
        let recovery_path =
            write_snapshot(directory.path(), 12, 7, ["unsaved 🦀\n"]).expect("recovery fixture");
        let restored = restore_session_tab(
            SessionTab {
                path: Some(path.clone()),
                recovery_path: Some(recovery_path.clone()),
                untitled_number: 0,
                label_override: None,
                dirty: true,
            },
            FilePolicy::default(),
        )
        .expect("restore");

        let RestoredFile::Recovered(seed) = restored else {
            panic!("expected recovered editor");
        };
        assert_eq!(seed.text, "unsaved 🦀\n");
        assert_eq!(seed.metadata.path.as_deref(), Some(path.as_path()));
        assert_eq!(seed.metadata.language, Language::Markdown);
        assert!(seed.disk_revision.is_some());
        assert!(seed.dirty);
        assert_eq!(seed.recovery_path.as_deref(), Some(recovery_path.as_path()));
    }

    #[gpui::test]
    fn settings_window_uses_current_appearance_and_recovery_values(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let workspace_slot = Rc::new(RefCell::new(None));
        let capture = workspace_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *capture.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = workspace_slot.borrow().clone().expect("workspace");
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.show_settings(window, cx);
            });
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.settings_visible);
            assert_eq!(
                workspace.settings_font_input.read(cx).value(),
                workspace.settings.appearance.font_family
            );
            let draft = workspace.settings_draft.as_ref().expect("draft");
            assert_eq!(draft.font_size, workspace.settings.appearance.font_size);
            assert_eq!(draft.recovery, workspace.settings.recovery);
        });
    }
}

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;

use gpui::{
    App, AppContext as _, Application, Context, Entity, ExternalPaths, InteractiveElement as _,
    IntoElement, KeyBinding, KeyDownEvent, Menu, MenuItem, MouseButton, OsAction,
    ParentElement as _, Render, ScrollDelta, ScrollHandle, ScrollStrategy, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled, Subscription, SystemMenuType, Timer,
    UniformListScrollHandle, Window, WindowBounds, WindowOptions, actions, div,
    prelude::FluentBuilder as _, px, size, uniform_list,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Root, Sizable as _, StyledExt as _,
    Theme, ThemeMode, TitleBar, WindowExt,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{
        Copy, Cut, Escape as InputEscape, Input, InputEvent, InputState, Paste, Redo, Rope,
        RopeExt as _, SelectAll, Undo,
    },
    scroll::ScrollableElement as _,
    select::{SearchableVec, Select, SelectState},
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
    recent::RecentFiles,
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
        ShowOpenTabs,
        ShowRecentFiles,
        ClearRecentFiles,
        SearchOpenTabs,
        ShowWorkspaceSearch,
        ShowSettings,
        ToggleWordWrap,
        ToggleTitleBar,
        GoToDefinition,
        DismissOverlay,
        QuitTextify
    ]
);

const WINDOW_TITLE: &str = "Textify IDE";
const RECOVERY_DEBOUNCE: Duration = Duration::from_millis(250);
const UNTITLED_TITLE_CHARS: usize = 36;
const UNTITLED_TITLE_SCAN_CHARS: usize = 256;

fn new_recovery_key(id: u64) -> u128 {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    timestamp ^ ((std::process::id() as u128) << 64) ^ id as u128
}

fn consume_zoom_delta(delta: ScrollDelta, accumulator: &mut f32) -> i8 {
    match delta {
        ScrollDelta::Lines(delta) => {
            if delta.y > 0.0 {
                1
            } else if delta.y < 0.0 {
                -1
            } else {
                0
            }
        }
        ScrollDelta::Pixels(delta) => {
            *accumulator += delta.y / px(40.);
            if accumulator.abs() < 1.0 {
                return 0;
            }
            let step = if *accumulator > 0.0 { 1 } else { -1 };
            *accumulator -= step as f32;
            step
        }
    }
}

fn first_line_title(chars: impl IntoIterator<Item = char>) -> Option<String> {
    let mut title = String::new();
    let mut visible_chars = 0usize;
    let mut pending_space = false;
    let mut truncated = false;

    for character in chars.into_iter().take(UNTITLED_TITLE_SCAN_CHARS) {
        if matches!(character, '\n' | '\r') {
            break;
        }
        if character.is_whitespace() || character.is_control() {
            pending_space = !title.is_empty();
            continue;
        }
        if pending_space {
            if visible_chars >= UNTITLED_TITLE_CHARS {
                truncated = true;
                break;
            }
            title.push(' ');
            visible_chars += 1;
            pending_space = false;
        }
        if visible_chars >= UNTITLED_TITLE_CHARS {
            truncated = true;
            break;
        }
        title.push(character);
        visible_chars += 1;
    }

    if title.is_empty() {
        None
    } else {
        if truncated {
            title.push('…');
        }
        Some(title)
    }
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

fn load_dropped_paths(
    paths: Vec<PathBuf>,
    policy: FilePolicy,
) -> Vec<(PathBuf, anyhow::Result<OpenedFile>)> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .map(|path| {
            let result = if path.is_file() {
                open_file(&path, policy)
            } else {
                Err(anyhow::anyhow!("{} is not a file", path.display()))
            };
            (path, result)
        })
        .collect()
}

struct QuitSnapshot {
    id: u64,
    key: u128,
    revision: u64,
    rope: Rope,
}

fn persist_quit_state(
    mut tabs: Vec<(u64, SessionTab)>,
    active_index: usize,
    workspace_root: Option<PathBuf>,
    snapshots: Vec<QuitSnapshot>,
    recovery_directory: &Path,
    session_path: &Path,
) -> (Vec<anyhow::Error>, anyhow::Result<()>) {
    let mut failures = Vec::new();
    for snapshot in snapshots {
        match write_snapshot(
            recovery_directory,
            snapshot.key,
            snapshot.revision,
            snapshot.rope.chunks(),
        ) {
            Ok(path) => {
                if let Some((_, tab)) = tabs.iter_mut().find(|(tab_id, _)| *tab_id == snapshot.id) {
                    tab.recovery_path = Some(path);
                    tab.dirty = true;
                }
            }
            Err(error) => failures.push(error),
        }
    }
    let state =
        SessionState::from_tabs(active_index, tabs.into_iter().map(|(_, tab)| tab).collect())
            .with_workspace_root(workspace_root);
    (failures, save_session(session_path, &state))
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
    font_size_override: Option<u16>,
    zoom_accumulator: f32,
    word_wrap: bool,
}

struct DocumentSeed {
    text: String,
    metadata: DocumentMetadata,
    disk_revision: Option<DiskRevision>,
    label_override: Option<String>,
    untitled_number: usize,
    dirty: bool,
    recovery_path: Option<PathBuf>,
    font_size_override: Option<u16>,
    word_wrap: bool,
}

enum OpenedFile {
    Editable(LoadedFile),
    Huge {
        file: HugeFile,
        metadata: DocumentMetadata,
    },
}

enum RestoredFile {
    Opened(OpenedFile, Option<u16>, bool),
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
            font_size_override: tab.font_size_override,
            word_wrap: tab.word_wrap,
        }));
    }

    if let Some(path) = tab.path {
        return open_file(&path, policy)
            .map(|opened| RestoredFile::Opened(opened, tab.font_size_override, tab.word_wrap));
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
        font_size_override: tab.font_size_override,
        word_wrap: tab.word_wrap,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayMode {
    Commands,
    OpenTabs,
    RecentFiles,
    OpenTabSearch,
    WorkspaceSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdeCommand {
    NewFile,
    OpenFile,
    SaveFile,
    SaveFileAs,
    CloseFile,
    NextTab,
    PreviousTab,
    OpenTabs,
    OpenRecent,
    ClearRecent,
    ToggleWordWrap,
    ToggleTitleBar,
    OpenFolder,
    SearchOpenTabs,
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
    Tab(u64),
    File(PathBuf),
    Search(WorkspaceMatch),
    OpenTabSearch { id: u64, location: TextLocation },
}

#[derive(Debug, Clone)]
struct OverlayItem {
    title: String,
    subtitle: String,
    search_text: String,
    target: OverlayTarget,
}

#[derive(Clone)]
struct OpenTabSearchDocument {
    id: u64,
    order: usize,
    title: String,
    subtitle: String,
    text: Rope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenTabContentMatch {
    id: u64,
    order: usize,
    title: String,
    subtitle: String,
    preview: String,
    line: usize,
    column: usize,
    end_column: usize,
    score: i64,
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
    show_tagline: bool,
    recovery: RecoverySettings,
    recent_files: crate::settings::RecentFileSettings,
}

impl EditorDocument {
    fn display_name(&self, cx: &App) -> String {
        if let Some(label) = &self.label_override {
            return label.clone();
        }
        if self.metadata.path.is_none()
            && let Some(title) = first_line_title(self.editor.rope(cx).chars())
        {
            return title;
        }
        self.metadata.display_name(self.untitled_number)
    }

    fn title(&self, cx: &App) -> String {
        if self.dirty {
            format!("{} •", self.display_name(cx))
        } else {
            self.display_name(cx)
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
    recent_files: RecentFiles,
    recent_files_path: PathBuf,
    recent_files_revision: u64,
    recent_files_persisting: bool,
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
    tab_scroll_handle: ScrollHandle,
    overlay_mode: Option<OverlayMode>,
    overlay_input: Entity<InputState>,
    overlay_items: Vec<OverlayItem>,
    overlay_selected_index: usize,
    overlay_scroll_handle: UniformListScrollHandle,
    settings_visible: bool,
    settings_draft: Option<SettingsDraft>,
    settings_font_select: Entity<SelectState<SearchableVec<String>>>,
    settings_location_input: Entity<InputState>,
    workspace_search: Option<WorkspaceSearchStream>,
    open_tab_search_cancel: Option<crate::huge_file::CancellationToken>,
    open_tab_search_revision: u64,
    status_message: Option<String>,
    lsp: Option<LspClient>,
    lsp_starting: bool,
    lsp_opened: HashSet<PathBuf>,
    lsp_dirty: HashMap<u64, Instant>,
    pending_definitions: HashSet<u64>,
    recovery_pending: HashMap<u64, Instant>,
    recovery_in_flight: HashSet<u64>,
    close_after_save: HashSet<u64>,
    quitting: bool,
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
        let recent_files_path = data_dir.join("recent-files.json");
        let recent_files = RecentFiles::load(&recent_files_path, settings.recent_files.limit())
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "using empty recent-file history");
                RecentFiles::default()
            });
        let overlay_input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command"));
        let font_families = editor_font_families(cx, &settings.appearance.font_family);
        let selected_font = font_families
            .iter()
            .position(|font| font == &settings.appearance.font_family)
            .map(|index| IndexPath::default().row(index));
        let settings_font_select = cx.new(|cx| {
            SelectState::new(SearchableVec::new(font_families), selected_font, window, cx)
                .searchable(true)
        });
        let settings_location_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Textify application backups"));
        let _ide_subscriptions =
            vec![
                cx.subscribe_in(&overlay_input, window, |workspace, _, event, window, cx| {
                    match event {
                        InputEvent::Change => workspace.refresh_overlay(window, cx),
                        InputEvent::PressEnter { .. } => {
                            workspace.accept_overlay(workspace.overlay_selected_index, window, cx)
                        }
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
            recent_files,
            recent_files_path,
            recent_files_revision: 0,
            recent_files_persisting: false,
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
            tab_scroll_handle: ScrollHandle::new(),
            overlay_mode: None,
            overlay_input,
            overlay_items: Vec::new(),
            overlay_selected_index: 0,
            overlay_scroll_handle: UniformListScrollHandle::new(),
            settings_visible: false,
            settings_draft: None,
            settings_font_select,
            settings_location_input,
            workspace_search: None,
            open_tab_search_cancel: None,
            open_tab_search_revision: 0,
            status_message: None,
            lsp: None,
            lsp_starting: false,
            lsp_opened: HashSet::new(),
            lsp_dirty: HashMap::new(),
            pending_definitions: HashSet::new(),
            recovery_pending: HashMap::new(),
            recovery_in_flight: HashSet::new(),
            close_after_save: HashSet::new(),
            quitting: false,
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
                font_size_override: None,
                word_wrap: false,
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
                font_size_override: None,
                word_wrap: false,
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
                font_size_override: None,
                word_wrap: false,
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
            font_size_override,
            word_wrap,
        } = seed;
        let focus_editor = metadata.mode != FileMode::HugeViewer;
        let id = self.next_id;
        self.next_id += 1;
        let editor = EditorBackend::new(
            text,
            metadata.parser_name(self.policy),
            metadata.mode,
            self.settings.editor,
            word_wrap,
            window,
            cx,
        );
        let subscription = cx.subscribe_in(
            editor.state(),
            window,
            move |workspace, _, event, window, cx| {
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
                        if workspace.active_id() == id {
                            workspace.update_window_title(window, cx);
                        }
                    }
                    cx.notify();
                }
            },
        );

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
            font_size_override,
            zoom_accumulator: 0.0,
            word_wrap,
        });
        self.active_index = self.documents.len() - 1;
        self.tab_scroll_handle.scroll_to_item(self.active_index);
        self.update_window_title(window, cx);

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
        self.tab_scroll_handle.scroll_to_item(index);
        if self.active_document().huge_viewer.is_none() {
            self.active_document().editor.focus(window, cx);
        }
        self.update_window_title(window, cx);
        self.persist_session(cx);
        cx.notify();
    }

    fn update_window_title(&self, window: &mut Window, cx: &App) {
        window.set_window_title(&format!("{} — Textify", self.active_document().title(cx)));
    }

    fn persist_session(&self, cx: &mut Context<Self>) {
        if self.restoring_session {
            return;
        }

        let tabs = self.session_tabs();
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

    fn session_tabs(&self) -> Vec<(u64, SessionTab)> {
        let recover_temporary = self.settings.recovery.save_temporary_files;
        self.documents
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
                        font_size_override: document.font_size_override,
                        word_wrap: document.word_wrap,
                    },
                )
            })
            .collect()
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
        let needs_first_snapshot =
            document.recovery_path.is_none() && !self.recovery_in_flight.contains(&id);
        self.recovery_pending.insert(
            id,
            if needs_first_snapshot {
                Instant::now() - RECOVERY_DEBOUNCE
            } else {
                Instant::now()
            },
        );
        self.persist_session(cx);
        if needs_first_snapshot {
            self.flush_recovery_due(cx);
        }
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
                                    Ok(RestoredFile::Opened(
                                        opened,
                                        font_size_override,
                                        word_wrap,
                                    )) => {
                                        workspace.push_opened(opened, window, cx);
                                        let document = workspace.active_document_mut();
                                        document.font_size_override = font_size_override;
                                        document.word_wrap = word_wrap;
                                        if document.metadata.mode == FileMode::Normal
                                            && document.huge_viewer.is_none()
                                        {
                                            document.editor.set_soft_wrap(word_wrap, window, cx);
                                        }
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
        let name = self.documents[index].display_name(cx);
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
                        let normal_mode = loaded.metadata.mode == FileMode::Normal;
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
                        editor.set_soft_wrap(document.word_wrap && normal_mode, window, cx);
                        workspace.update_window_title(window, cx);
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
        let name = self.documents[index].display_name(cx);
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
                            font_size_override: None,
                            word_wrap: false,
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
                                font_size_override: None,
                                word_wrap: false,
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
            self.record_recent_file(path, cx);
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
                        workspace.record_recent_file(path.clone(), cx);
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
        if let Some(cancel) = self.open_tab_search_cancel.take() {
            cancel.cancel();
        }
        self.open_tab_search_revision = self.open_tab_search_revision.wrapping_add(1);
        self.overlay_mode = Some(mode);
        self.overlay_items.clear();
        self.overlay_selected_index = 0;
        let placeholder = match mode {
            OverlayMode::Commands => "Type a command",
            OverlayMode::OpenTabs => "Find an open tab",
            OverlayMode::RecentFiles => "Find a recent file",
            OverlayMode::OpenTabSearch => "Search text across open tabs",
            OverlayMode::WorkspaceSearch => "Search text in the workspace",
        };
        self.overlay_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
            input.focus(window, cx);
        });
        let overlay_input = self.overlay_input.clone();
        window.defer(cx, move |window, cx| {
            overlay_input.update(cx, |input, cx| input.focus(window, cx));
        });
        self.refresh_overlay(window, cx);
        cx.notify();
    }

    fn hide_overlay(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.workspace_search.take() {
            search.cancel.cancel();
        }
        if let Some(cancel) = self.open_tab_search_cancel.take() {
            cancel.cancel();
        }
        self.overlay_mode = None;
        self.overlay_items.clear();
        self.overlay_selected_index = 0;
        cx.notify();
    }

    fn dismiss_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_overlay(cx);
        if self.active_document().huge_viewer.is_none() {
            self.active_document().editor.focus(window, cx);
        }
    }

    fn refresh_overlay(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.overlay_mode else {
            return;
        };
        self.overlay_selected_index = 0;
        let query = self.overlay_input.read(cx).value().trim().to_lowercase();
        match mode {
            OverlayMode::Commands => {
                self.overlay_items = matching_command_items(&query);
            }
            OverlayMode::OpenTabs => {
                let items = self
                    .documents
                    .iter()
                    .map(|document| {
                        let title = document.title(cx);
                        let subtitle = document
                            .metadata
                            .path
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "Unsaved document".to_owned());
                        OverlayItem {
                            search_text: format!("{title} {subtitle}").to_lowercase(),
                            title,
                            subtitle,
                            target: OverlayTarget::Tab(document.id),
                        }
                    })
                    .collect();
                self.overlay_items = matching_open_tab_items(items, &query);
            }
            OverlayMode::RecentFiles => {
                let items = self
                    .recent_files
                    .paths
                    .iter()
                    .cloned()
                    .map(|path| OverlayItem {
                        title: path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("File")
                            .to_owned(),
                        subtitle: path.display().to_string(),
                        search_text: path.display().to_string().to_lowercase(),
                        target: OverlayTarget::File(path),
                    })
                    .collect();
                self.overlay_items = matching_open_tab_items(items, &query);
            }
            OverlayMode::OpenTabSearch => self.start_open_tab_search(query, cx),
            OverlayMode::WorkspaceSearch => self.start_workspace_search(query, cx),
        }
        cx.notify();
    }

    fn start_open_tab_search(&mut self, query: String, cx: &mut Context<Self>) {
        if let Some(cancel) = self.open_tab_search_cancel.take() {
            cancel.cancel();
        }
        self.overlay_items.clear();
        if query.is_empty() {
            self.status_message = Some("Type to search all open tabs".to_owned());
            return;
        }
        let documents = self
            .documents
            .iter()
            .enumerate()
            .filter(|(_, document)| document.huge_viewer.is_none())
            .map(|(order, document)| OpenTabSearchDocument {
                id: document.id,
                order,
                title: document.display_name(cx),
                subtitle: document
                    .metadata
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Unsaved document".to_owned()),
                text: document.editor.rope(cx),
            })
            .collect::<Vec<_>>();
        let max_matches = self.settings.workspace.search_max_matches;
        let cancel = crate::huge_file::CancellationToken::default();
        let worker_cancel = cancel.clone();
        self.open_tab_search_cancel = Some(cancel);
        self.open_tab_search_revision = self.open_tab_search_revision.wrapping_add(1);
        let revision = self.open_tab_search_revision;
        self.status_message = Some(format!("Searching open tabs for “{query}”…"));
        let search_query = query.clone();
        let task = cx.background_spawn(async move {
            search_open_tabs(&documents, &search_query, max_matches, &worker_cancel)
        });
        cx.spawn(async move |workspace, cx| {
            let matches = task.await;
            let Some(workspace) = workspace.upgrade() else {
                return;
            };
            workspace
                .update(cx, |workspace, cx| {
                    if workspace.overlay_mode != Some(OverlayMode::OpenTabSearch)
                        || workspace.open_tab_search_revision != revision
                        || workspace
                            .overlay_input
                            .read(cx)
                            .value()
                            .trim()
                            .to_lowercase()
                            != query
                    {
                        return;
                    }
                    workspace.open_tab_search_cancel = None;
                    workspace.overlay_items = matches
                        .into_iter()
                        .map(|item| OverlayItem {
                            title: item.preview,
                            subtitle: format!(
                                "{} · {} · line {}, column {}",
                                item.title,
                                item.subtitle,
                                item.line + 1,
                                item.column + 1
                            ),
                            search_text: String::new(),
                            target: OverlayTarget::OpenTabSearch {
                                id: item.id,
                                location: TextLocation {
                                    line: item.line,
                                    column: item.column,
                                    end_line: item.line,
                                    end_column: item.end_column,
                                },
                            },
                        })
                        .collect();
                    workspace.overlay_selected_index = 0;
                    workspace.status_message = Some(format!(
                        "{} matches across open tabs",
                        workspace.overlay_items.len()
                    ));
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn move_overlay_selection(&mut self, direction: isize, cx: &mut Context<Self>) {
        if self.overlay_mode.is_none() {
            cx.propagate();
            return;
        }
        if self.overlay_items.is_empty() {
            cx.stop_propagation();
            return;
        }
        let last = self.overlay_items.len() - 1;
        self.overlay_selected_index = if direction < 0 {
            self.overlay_selected_index.saturating_sub(1)
        } else {
            (self.overlay_selected_index + 1).min(last)
        };
        self.overlay_scroll_handle
            .scroll_to_item(self.overlay_selected_index, ScrollStrategy::Center);
        cx.stop_propagation();
        cx.notify();
    }

    fn on_overlay_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_mode.is_none() || event.keystroke.modifiers.modified() {
            return;
        }
        match event.keystroke.key.as_str() {
            "up" => self.move_overlay_selection(-1, cx),
            "down" => self.move_overlay_selection(1, cx),
            _ => {}
        }
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
                        search_text: item.preview.clone(),
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
            OverlayTarget::Tab(id) => {
                if let Some(index) = self.document_index(id) {
                    self.set_active_index(index, window, cx);
                }
            }
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
            OverlayTarget::OpenTabSearch { id, location } => {
                if let Some(index) = self.document_index(id) {
                    self.set_active_index(index, window, cx);
                    if self.documents[index].huge_viewer.is_none() {
                        self.documents[index].editor.select_position(
                            location.line,
                            location.column,
                            location.end_line,
                            location.end_column,
                            window,
                            cx,
                        );
                    }
                }
            }
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
            IdeCommand::SaveFile => self.on_save(&SaveDocument, window, cx),
            IdeCommand::SaveFileAs => self.on_save_as(&SaveDocumentAs, window, cx),
            IdeCommand::CloseFile => self.on_close(&CloseDocument, window, cx),
            IdeCommand::NextTab => self.on_next(&NextDocument, window, cx),
            IdeCommand::PreviousTab => self.on_previous(&PreviousDocument, window, cx),
            IdeCommand::OpenTabs => self.on_open_tabs(&ShowOpenTabs, window, cx),
            IdeCommand::OpenRecent => self.on_recent_files(&ShowRecentFiles, window, cx),
            IdeCommand::ClearRecent => self.on_clear_recent_files(&ClearRecentFiles, window, cx),
            IdeCommand::ToggleWordWrap => self.on_toggle_word_wrap(&ToggleWordWrap, window, cx),
            IdeCommand::ToggleTitleBar => self.on_toggle_title_bar(&ToggleTitleBar, window, cx),
            IdeCommand::OpenFolder => self.on_open_folder(&OpenFolder, window, cx),
            IdeCommand::SearchOpenTabs => self.on_search_open_tabs(&SearchOpenTabs, window, cx),
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
            show_tagline: self.settings.appearance.show_tagline,
            recovery: self.settings.recovery.clone(),
            recent_files: self.settings.recent_files.clone(),
        });
        let font_family = self.settings.appearance.font_family.clone();
        let font_families = editor_font_families(cx, &font_family);
        let recovery_directory = self.settings.recovery.directory(&self.data_dir);
        self.settings_font_select.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(font_families), window, cx);
            select.set_selected_value(&font_family, window, cx);
            select.focus(window, cx);
        });
        self.settings_location_input.update(cx, |input, cx| {
            input.set_value(recovery_directory.display().to_string(), window, cx);
        });
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
        let font_family = self
            .settings_font_select
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_else(|| self.settings.appearance.font_family.clone());
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
        settings.appearance.font_family = font_family;
        settings.appearance.font_size = draft.font_size;
        settings.appearance.show_tagline = draft.show_tagline;
        settings.appearance.normalize();
        settings.recovery = draft.recovery;
        settings.recent_files = draft.recent_files;
        settings.recent_files.normalize();
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
                        workspace.apply_recent_file_settings(cx);
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

    fn on_search_open_tabs(
        &mut self,
        _: &SearchOpenTabs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_overlay(OverlayMode::OpenTabSearch, window, cx);
    }

    fn on_open_tabs(&mut self, _: &ShowOpenTabs, window: &mut Window, cx: &mut Context<Self>) {
        self.show_overlay(OverlayMode::OpenTabs, window, cx);
    }

    fn on_recent_files(
        &mut self,
        _: &ShowRecentFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_overlay(OverlayMode::RecentFiles, window, cx);
    }

    fn on_clear_recent_files(
        &mut self,
        _: &ClearRecentFiles,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.recent_files.clear();
        self.persist_recent_files(cx);
        self.status_message = Some("Recent file history cleared".to_owned());
        if self.overlay_mode == Some(OverlayMode::RecentFiles) {
            self.overlay_items.clear();
            self.overlay_selected_index = 0;
        }
        cx.notify();
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

    fn on_toggle_word_wrap(
        &mut self,
        _: &ToggleWordWrap,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let document = self.active_document();
        if document.metadata.mode != FileMode::Normal || document.huge_viewer.is_some() {
            self.status_message = Some("Word wrap is disabled by large-file policy".to_owned());
            cx.notify();
            return;
        }
        let document = self.active_document_mut();
        document.word_wrap = !document.word_wrap;
        let enabled = document.word_wrap;
        document.editor.set_soft_wrap(enabled, window, cx);
        self.status_message = Some(if enabled {
            "Word wrap enabled for this tab".to_owned()
        } else {
            "Word wrap disabled for this tab".to_owned()
        });
        self.persist_session(cx);
        cx.notify();
    }

    fn on_toggle_title_bar(&mut self, _: &ToggleTitleBar, _: &mut Window, cx: &mut Context<Self>) {
        self.settings.appearance.show_title_bar = !self.settings.appearance.show_title_bar;
        self.status_message = Some(if self.settings.appearance.show_title_bar {
            "Title bar shown".to_owned()
        } else {
            "Title bar hidden".to_owned()
        });
        self.persist_settings_silently(cx);
        cx.notify();
    }

    fn persist_settings_silently(&self, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        let path = self.data_dir.join("settings.json");
        cx.background_spawn(async move {
            if let Some(parent) = path.parent()
                && let Err(error) = fs::create_dir_all(parent)
            {
                tracing::warn!(%error, "could not prepare settings directory");
                return;
            }
            if let Err(error) = settings.save(&path) {
                tracing::warn!(%error, "could not persist settings");
            }
        })
        .detach();
    }

    fn record_recent_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let limit = self.settings.recent_files.limit();
        if limit == 0 {
            return;
        }
        self.recent_files.record(path, limit);
        self.persist_recent_files(cx);
    }

    fn apply_recent_file_settings(&mut self, cx: &mut Context<Self>) {
        self.recent_files
            .normalize(self.settings.recent_files.limit());
        self.persist_recent_files(cx);
    }

    fn persist_recent_files(&mut self, cx: &mut Context<Self>) {
        self.recent_files_revision = self.recent_files_revision.wrapping_add(1);
        self.start_recent_file_persist(cx);
    }

    fn start_recent_file_persist(&mut self, cx: &mut Context<Self>) {
        if self.recent_files_persisting {
            return;
        }
        self.recent_files_persisting = true;
        let revision = self.recent_files_revision;
        let history = self.recent_files.clone();
        let path = self.recent_files_path.clone();
        let task = cx.background_spawn(async move {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            history.save(&path)
        });
        cx.spawn(async move |workspace, cx| {
            let result = task.await;
            let Some(workspace) = workspace.upgrade() else {
                return;
            };
            workspace
                .update(cx, |workspace, cx| {
                    workspace.recent_files_persisting = false;
                    if let Err(error) = result {
                        tracing::warn!(%error, "could not persist recent-file history");
                    }
                    if workspace.recent_files_revision != revision {
                        workspace.start_recent_file_persist(cx);
                    }
                })
                .ok();
        })
        .detach();
    }

    fn on_quit_textify(&mut self, _: &QuitTextify, window: &mut Window, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.status_message = Some("Saving recovery copies before quitting…".to_owned());
        let tabs = self.session_tabs();
        let active_id = self.active_id();
        let active_index = tabs
            .iter()
            .position(|(id, _)| *id == active_id)
            .unwrap_or(0);
        let snapshots = self
            .documents
            .iter()
            .filter(|document| {
                document.dirty
                    && document.huge_viewer.is_none()
                    && self
                        .settings
                        .recovery
                        .enabled_for(document.metadata.path.is_some())
            })
            .map(|document| QuitSnapshot {
                id: document.id,
                key: document.recovery_key,
                revision: document.revision,
                rope: document.editor.rope(cx),
            })
            .collect::<Vec<_>>();
        let directory = self.settings.recovery.directory(&self.data_dir);
        let session_path = self.session_path.clone();
        let workspace_root = self.workspace_root.clone();
        let task = cx.background_spawn(async move {
            persist_quit_state(
                tabs,
                active_index,
                workspace_root,
                snapshots,
                &directory,
                &session_path,
            )
        });
        cx.notify();
        cx.spawn_in(window, async move |workspace, window| {
            let (failures, session_result) = task.await;
            workspace
                .update_in(window, |workspace, _, cx| {
                    for error in failures {
                        tracing::warn!(%error, "could not finish recovery snapshot during quit");
                    }
                    if let Err(error) = session_result {
                        tracing::warn!(%error, "could not persist final session during quit");
                    }
                    workspace.status_message = Some("Recovery state saved".to_owned());
                    cx.quit();
                })
                .ok()
        })
        .detach();
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
            self.dismiss_overlay(window, cx);
        } else {
            cx.propagate();
        }
    }

    fn on_input_escape(&mut self, _: &InputEscape, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_visible {
            self.hide_settings(window, cx);
        } else if self.overlay_mode.is_some() {
            self.dismiss_overlay(window, cx);
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
                            workspace.apply_recent_file_settings(cx);
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
                        workspace.push_opened(opened, window, cx);
                        workspace.record_recent_file(path, cx);
                    }
                    Err(error) => Self::show_error("Could not open file", error, window, cx),
                })
                .ok()
        })
        .detach();
    }

    fn open_dropped_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        self.status_message = Some(format!("Opening {count} dropped path(s)…"));
        let policy = self.policy;
        let task = cx.background_spawn(async move { load_dropped_paths(paths, policy) });
        cx.notify();
        cx.spawn_in(window, async move |workspace, window| {
            let results = task.await;
            workspace
                .update_in(window, |workspace, window, cx| {
                    let mut opened = 0usize;
                    let mut failures = Vec::new();
                    for (path, result) in results {
                        match result {
                            Ok(file) => {
                                workspace.push_opened(file, window, cx);
                                workspace.record_recent_file(path, cx);
                                opened += 1;
                            }
                            Err(error) => failures.push(format!("{}: {error}", path.display())),
                        }
                    }
                    workspace.status_message = Some(match (opened, failures.len()) {
                        (opened, 0) => format!("Opened {opened} dropped file(s)"),
                        (0, failed) => format!("Could not open {failed} dropped path(s)"),
                        (opened, failed) => {
                            format!("Opened {opened} file(s); skipped {failed} path(s)")
                        }
                    });
                    if opened == 0
                        && let Some(message) = failures.into_iter().next()
                    {
                        Self::show_error(
                            "Could not open dropped file",
                            anyhow::anyhow!(message),
                            window,
                            cx,
                        );
                    }
                    cx.notify();
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
            document.display_name(cx)
        } else {
            suggested_save_path(&directory, document.untitled_number)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Untitled.txt")
                .to_owned()
        };
        let receiver = cx.prompt_for_new_path(&directory, Some(&suggested));

        cx.spawn_in(window, async move |workspace, window| {
            let path = receiver.await.ok().into_iter().flatten().flatten().next();
            workspace
                .update_in(window, |workspace, window, cx| match path {
                    Some(path) => workspace.start_save(id, path, window, cx),
                    None => {
                        workspace.close_after_save.remove(&id);
                        workspace.status_message = Some("Close canceled".to_owned());
                        cx.notify();
                    }
                })
                .ok()?;
            Some(())
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
                        let normal_mode = metadata.mode == FileMode::Normal;
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
                        editor.set_soft_wrap(document.word_wrap && normal_mode, window, cx);
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
                        workspace.update_window_title(window, cx);
                        workspace.record_recent_file(path.clone(), cx);
                        if saved_clean {
                            workspace.clear_recovery(id, cx);
                        }
                        let close_after_save = workspace.close_after_save.remove(&id);
                        if close_after_save && saved_clean {
                            workspace.remove_document(id, window, cx);
                            return;
                        }
                        if close_after_save {
                            workspace.status_message = Some(
                                "The document changed while saving; close canceled".to_owned(),
                            );
                        }
                        workspace.persist_session(cx);
                        workspace.refresh_git(window, cx);
                        cx.notify();
                    }
                    Err(error) => {
                        workspace.close_after_save.remove(&id);
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

        let name = self.documents[index].display_name(cx);
        let workspace = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let save_workspace = workspace.clone();
            dialog
                .title(format!("Save changes to {name}?"))
                .child("Your changes will be lost if you close without saving.")
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Save")
                        .cancel_text("Cancel"),
                )
                .footer({
                    let discard_workspace = workspace.clone();
                    move |save, cancel, window, cx| {
                        let discard_workspace = discard_workspace.clone();
                        vec![
                            Button::new(("discard-close", id))
                                .label("Don't Save")
                                .with_variant(ButtonVariant::Danger)
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    discard_workspace.update(cx, |workspace, cx| {
                                        workspace.remove_document(id, window, cx)
                                    });
                                })
                                .into_any_element(),
                            cancel(window, cx),
                            save(window, cx),
                        ]
                    }
                })
                .overlay_closable(false)
                .close_button(false)
                .on_ok(move |_, window, cx| {
                    save_workspace.update(cx, |workspace, cx| {
                        workspace.save_before_close(id, window, cx);
                    });
                    true
                })
        });
    }

    fn save_before_close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        self.close_after_save.insert(id);
        if let Some(path) = self.documents[index].metadata.path.clone() {
            self.start_save(id, path, window, cx);
        } else {
            self.prompt_save_as(id, window, cx);
        }
    }

    fn remove_document(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.document_index(id) else {
            return;
        };
        let removed = self.documents.remove(index);
        self.close_after_save.remove(&id);
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
        self.tab_scroll_handle.scroll_to_item(self.active_index);

        if self.active_document().huge_viewer.is_none() {
            self.active_document().editor.focus(window, cx);
        }
        self.update_window_title(window, cx);
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

    fn on_editor_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.modifiers.secondary() || self.active_document().huge_viewer.is_some() {
            cx.propagate();
            return;
        }
        let step = {
            let document = self.active_document_mut();
            consume_zoom_delta(event.delta, &mut document.zoom_accumulator)
        };
        if step == 0 {
            cx.stop_propagation();
            return;
        }
        let default_size = self.settings.appearance.font_size;
        let document = self.active_document_mut();
        let current = document.font_size_override.unwrap_or(default_size);
        let next = if step > 0 {
            current.saturating_add(1)
        } else {
            current.saturating_sub(1)
        }
        .clamp(
            AppearanceSettings::MIN_FONT_SIZE,
            AppearanceSettings::MAX_FONT_SIZE,
        );
        if next == current {
            cx.stop_propagation();
            return;
        }
        document.editor.preserve_cursor_anchor(cx);
        document.font_size_override = Some(next);
        self.status_message = Some(format!("Text size: {next} pt (this tab)"));
        self.persist_session(cx);
        cx.stop_propagation();
        cx.notify();
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
                Tab::new().label(document.title(cx)).suffix(
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
            .menu(true)
            .track_scroll(&self.tab_scroll_handle)
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
                        Button::new("search-open-tabs")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Search)
                            .tooltip_with_action("Search open tabs", &SearchOpenTabs, None)
                            .on_click(cx.listener(|workspace, _, window, cx| {
                                workspace.on_search_open_tabs(&SearchOpenTabs, window, cx)
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
                    .when(document.dirty, |row| {
                        row.child(
                            div()
                                .px_2()
                                .py(px(1.))
                                .rounded_sm()
                                .bg(cx.theme().warning.opacity(0.12))
                                .text_color(cx.theme().warning)
                                .child("UNSAVED"),
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
                    .child(
                        if document.word_wrap && document.metadata.mode == FileMode::Normal {
                            "WRAP"
                        } else {
                            "NO WRAP"
                        },
                    )
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
            Some(OverlayMode::OpenTabs) => "OPEN TABS",
            Some(OverlayMode::RecentFiles) => "OPEN RECENT",
            Some(OverlayMode::OpenTabSearch) => "SEARCH OPEN TABS",
            Some(OverlayMode::WorkspaceSearch) => "WORKSPACE SEARCH",
            None => "",
        };
        let panel = v_flex()
            .id("ide-overlay-panel")
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
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .h_8()
                    .px_3()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(title)
                    .child("↑↓ navigate · Enter opens · Esc closes"),
            )
            .child(div().px_2().pb_2().child(Input::new(&self.overlay_input)))
            .when(count == 0, |panel| {
                panel.child(
                    div()
                        .id("ide-overlay-empty")
                        .h_10()
                        .px_3()
                        .flex()
                        .items_center()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No matching results"),
                )
            })
            .when(count > 0, |panel| {
                panel.child(
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
                                            .h_10()
                                            .px_3()
                                            .py_1()
                                            .cursor_pointer()
                                            .border_t_1()
                                            .border_color(cx.theme().border.opacity(0.55))
                                            .when(
                                                index == workspace.overlay_selected_index,
                                                |row| row.bg(cx.theme().list_active),
                                            )
                                            .hover(|row| row.bg(cx.theme().list_hover))
                                            .on_click(cx.listener(
                                                move |workspace, _, window, cx| {
                                                    workspace.accept_overlay(index, window, cx)
                                                },
                                            ))
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
                    .track_scroll(self.overlay_scroll_handle.clone())
                    .h(px(count.min(10) as f32 * 40.)),
                )
            });

        div()
            .id("ide-overlay-backdrop")
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|workspace, _, window, cx| workspace.dismiss_overlay(window, cx)),
            )
            .child(panel)
    }

    fn render_settings_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let draft = self.settings_draft.clone().unwrap_or(SettingsDraft {
            font_size: self.settings.appearance.font_size,
            show_tagline: self.settings.appearance.show_tagline,
            recovery: self.settings.recovery.clone(),
            recent_files: self.settings.recent_files.clone(),
        });
        let temporary_enabled = draft.recovery.save_temporary_files;
        let unsaved_enabled = draft.recovery.keep_unsaved_changes;
        let tagline_enabled = draft.show_tagline;
        let recent_enabled = draft.recent_files.enabled;
        let recent_limit = draft.recent_files.max_files;
        let temporary_workspace = cx.entity();
        let unsaved_workspace = cx.entity();
        let tagline_workspace = cx.entity();
        let recent_workspace = cx.entity();

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
                            .overflow_y_scrollbar()
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
                                    .child(
                                        Select::new(&self.settings_font_select)
                                            .search_placeholder("Search installed fonts…")
                                            .w_full(),
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
                                                    .child("Show Tagline"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(
                                                        "Show “A fast place for text” in the title bar.",
                                                    ),
                                            ),
                                    )
                                    .child(
                                        Switch::new("settings-show-tagline")
                                            .checked(tagline_enabled)
                                            .on_click(move |checked, _, cx| {
                                                tagline_workspace.update(cx, |workspace, cx| {
                                                    if let Some(draft) =
                                                        &mut workspace.settings_draft
                                                    {
                                                        draft.show_tagline = *checked;
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
                                                    .child("Remember Recent Files"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Keep a local Open Recent history; disabling this clears it."),
                                            ),
                                    )
                                    .child(
                                        Switch::new("settings-remember-recent-files")
                                            .checked(recent_enabled)
                                            .on_click(move |checked, _, cx| {
                                                recent_workspace.update(cx, |workspace, cx| {
                                                    if let Some(draft) =
                                                        &mut workspace.settings_draft
                                                    {
                                                        draft.recent_files.enabled = *checked;
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
                                                    .child("Recent File Limit"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Maximum files shown by Open Recent."),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                Button::new("settings-recent-fewer")
                                                    .outline()
                                                    .disabled(!recent_enabled)
                                                    .label("−")
                                                    .on_click(cx.listener(
                                                        |workspace, _, _, cx| {
                                                            if let Some(draft) =
                                                                &mut workspace.settings_draft
                                                            {
                                                                draft.recent_files.max_files = draft
                                                                    .recent_files
                                                                    .max_files
                                                                    .saturating_sub(1)
                                                                    .max(crate::settings::RecentFileSettings::MIN_FILES);
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .w_12()
                                                    .text_center()
                                                    .child(recent_limit.to_string()),
                                            )
                                            .child(
                                                Button::new("settings-recent-more")
                                                    .outline()
                                                    .disabled(!recent_enabled)
                                                    .label("+")
                                                    .on_click(cx.listener(
                                                        |workspace, _, _, cx| {
                                                            if let Some(draft) =
                                                                &mut workspace.settings_draft
                                                            {
                                                                draft.recent_files.max_files =
                                                                    (draft.recent_files.max_files + 1)
                                                                        .min(crate::settings::RecentFileSettings::MAX_FILES);
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                Button::new("settings-clear-recent")
                                                    .outline()
                                                    .label("Clear History")
                                                    .disabled(self.recent_files.paths.is_empty())
                                                    .on_click(cx.listener(
                                                        |workspace, _, window, cx| {
                                                            workspace.on_clear_recent_files(
                                                                &ClearRecentFiles,
                                                                window,
                                                                cx,
                                                            )
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
        let font_size = self
            .active_document()
            .font_size_override
            .unwrap_or(self.settings.appearance.font_size);
        let content = self
            .active_document()
            .huge_viewer
            .clone()
            .map(|viewer| viewer.into_any_element())
            .unwrap_or_else(|| {
                editor
                    .render(&self.settings.appearance.font_family, font_size, cx)
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
            .on_action(cx.listener(Self::on_open_tabs))
            .on_action(cx.listener(Self::on_recent_files))
            .on_action(cx.listener(Self::on_clear_recent_files))
            .on_action(cx.listener(Self::on_search_open_tabs))
            .on_action(cx.listener(Self::on_workspace_search))
            .on_action(cx.listener(Self::on_show_settings))
            .on_action(cx.listener(Self::on_toggle_word_wrap))
            .on_action(cx.listener(Self::on_toggle_title_bar))
            .on_action(cx.listener(Self::on_quit_textify))
            .on_action(cx.listener(Self::on_go_to_definition))
            .on_action(cx.listener(Self::on_dismiss_overlay))
            .on_drop(cx.listener(|workspace, paths: &ExternalPaths, window, cx| {
                workspace.open_dropped_paths(paths.paths().to_vec(), window, cx)
            }))
            .when(self.settings.appearance.show_title_bar, |workspace| {
                workspace.child(
                    TitleBar::new().child(
                        h_flex()
                            .id("textify-title-bar-content")
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
                            .when(self.settings.appearance.show_tagline, |title| {
                                title.child(
                                    div()
                                        .id("textify-tagline")
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("A fast place for text"),
                                )
                            }),
                    ),
                )
            })
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
                            .on_scroll_wheel(cx.listener(Self::on_editor_scroll))
                            .child(content),
                    ),
            )
            .child(self.render_status(cx));

        div()
            .relative()
            .size_full()
            .on_action(cx.listener(Self::on_input_escape))
            .capture_key_down(cx.listener(Self::on_overlay_key_down))
            .child(workspace)
            .when(self.overlay_mode.is_some(), |root| {
                root.child(self.render_overlay(cx))
            })
            .when(self.settings_visible, |root| {
                root.child(self.render_settings_panel(cx))
            })
    }
}

fn search_open_tabs(
    documents: &[OpenTabSearchDocument],
    query: &str,
    max_matches: usize,
    cancel: &crate::huge_file::CancellationToken,
) -> Vec<OpenTabContentMatch> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() || max_matches == 0 || cancel.is_cancelled() {
        return Vec::new();
    }
    let tokens = query.split_whitespace().collect::<Vec<_>>();
    let candidate_limit = max_matches.saturating_mul(4).max(max_matches);
    let mut matches = Vec::new();

    'documents: for document in documents {
        for (line_index, line) in document.text.iter_lines().enumerate() {
            if cancel.is_cancelled() {
                return Vec::new();
            }
            let line = line.to_string();
            let line = line.trim_end_matches(['\n', '\r']);
            let line_lower = line.to_ascii_lowercase();
            let mut exact_from = 0usize;
            let mut found_exact = false;
            while let Some(relative) = line_lower[exact_from..].find(&query) {
                found_exact = true;
                let start = exact_from + relative;
                let end = start + query.len();
                push_open_tab_match(
                    &mut matches,
                    document,
                    line,
                    line_index,
                    start,
                    end,
                    1_000 - start as i64,
                );
                if matches.len() >= candidate_limit {
                    break 'documents;
                }
                exact_from = end;
            }
            if found_exact || tokens.len() < 2 {
                continue;
            }

            let mut start = usize::MAX;
            let mut end = 0usize;
            let mut all_tokens = true;
            for token in &tokens {
                let Some(index) = line_lower.find(token) else {
                    all_tokens = false;
                    break;
                };
                start = start.min(index);
                end = end.max(index + token.len());
            }
            if all_tokens {
                push_open_tab_match(
                    &mut matches,
                    document,
                    line,
                    line_index,
                    start,
                    end,
                    600 - end.saturating_sub(start) as i64,
                );
                if matches.len() >= candidate_limit {
                    break 'documents;
                }
            }
        }
    }

    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.column.cmp(&right.column))
    });
    matches.truncate(max_matches);
    matches
}

fn push_open_tab_match(
    matches: &mut Vec<OpenTabContentMatch>,
    document: &OpenTabSearchDocument,
    line: &str,
    line_index: usize,
    start: usize,
    end: usize,
    score: i64,
) {
    let column = line[..start].chars().count();
    let end_column = column + line[start..end].chars().count();
    matches.push(OpenTabContentMatch {
        id: document.id,
        order: document.order,
        title: document.title.clone(),
        subtitle: document.subtitle.clone(),
        preview: line.chars().take(240).collect(),
        line: line_index,
        column,
        end_column,
        score,
    });
}

fn editor_font_families(cx: &App, configured: &str) -> Vec<String> {
    normalize_editor_font_families(cx.text_system().all_font_names(), configured)
}

fn normalize_editor_font_families(mut fonts: Vec<String>, configured: &str) -> Vec<String> {
    fonts.retain(|font| !font.trim().is_empty());
    for font in &mut fonts {
        if font.eq_ignore_ascii_case(configured) {
            *font = configured.to_owned();
        }
    }
    if !configured.trim().is_empty() && !fonts.iter().any(|font| font == configured) {
        fonts.push(configured.to_owned());
    }
    fonts.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    fonts.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    fonts
}

fn command_items() -> Vec<OverlayItem> {
    [
        (
            "New File",
            "Create an untitled editor",
            "new tab document make create blank note",
            IdeCommand::NewFile,
        ),
        (
            "Open File…",
            "Open a file from disk",
            "open load browse choose existing document",
            IdeCommand::OpenFile,
        ),
        (
            "Save File",
            "Save changes to disk",
            "save write keep changes current document",
            IdeCommand::SaveFile,
        ),
        (
            "Save File As…",
            "Save under a different name",
            "save as copy another different rename somewhere else",
            IdeCommand::SaveFileAs,
        ),
        (
            "Close Tab",
            "Close the current document",
            "close remove dismiss tab file document current",
            IdeCommand::CloseFile,
        ),
        (
            "Next Tab",
            "Activate the next document",
            "next forward switch cycle tab document",
            IdeCommand::NextTab,
        ),
        (
            "Previous Tab",
            "Activate the previous document",
            "previous back switch cycle tab document",
            IdeCommand::PreviousTab,
        ),
        (
            "Show Open Tabs",
            "Search and activate an open document",
            "show every list find fuzzy wildcard switch focus reveal open tabs files documents",
            IdeCommand::OpenTabs,
        ),
        (
            "Open Recent File",
            "Find a file from local history",
            "open recent history previous file document reopen",
            IdeCommand::OpenRecent,
        ),
        (
            "Clear Recent Files",
            "Forget local file history",
            "clear erase forget recent history files privacy",
            IdeCommand::ClearRecent,
        ),
        (
            "Toggle Word Wrap",
            "Wrap long lines in the current tab",
            "toggle turn on off enable disable word wrap long lines current tab",
            IdeCommand::ToggleWordWrap,
        ),
        (
            "Toggle Title Bar",
            "Show or hide the Textify heading",
            "toggle show hide top title bar header textify heading chrome",
            IdeCommand::ToggleTitleBar,
        ),
        (
            "Open Folder…",
            "Index a project lazily",
            "open folder directory project workspace load browse",
            IdeCommand::OpenFolder,
        ),
        (
            "Search Open Tabs",
            "Find text in every open document",
            "search find text content across all open tabs documents unsaved",
            IdeCommand::SearchOpenTabs,
        ),
        (
            "Search Workspace",
            "Stream text matches from project files",
            "search find text content across project workspace files",
            IdeCommand::WorkspaceSearch,
        ),
        (
            "Toggle File Explorer",
            "Show or hide the project sidebar",
            "toggle show hide file list tree explorer sidebar project",
            IdeCommand::ToggleSidebar,
        ),
        (
            "Refresh Project",
            "Rescan files and Git decorations",
            "refresh reload rescan project git status files",
            IdeCommand::RefreshProject,
        ),
        (
            "Open Settings",
            "Change fonts and crash recovery",
            "open show settings preferences options configure font size recovery temporary unsaved",
            IdeCommand::OpenSettings,
        ),
        (
            "Open Keymap",
            "Edit keymap.json (reloads on save)",
            "open edit keyboard shortcuts hotkeys bindings keymap configure",
            IdeCommand::OpenKeymap,
        ),
        (
            "Go to Definition",
            "Ask the configured language server",
            "go jump navigate definition symbol declaration lsp language server",
            IdeCommand::GoToDefinition,
        ),
    ]
    .into_iter()
    .map(|(title, subtitle, keywords, command)| OverlayItem {
        title: title.to_owned(),
        subtitle: subtitle.to_owned(),
        search_text: format!("{title} {subtitle} {keywords}").to_lowercase(),
        target: OverlayTarget::Command(command),
    })
    .collect()
}

fn matching_command_items(query: &str) -> Vec<OverlayItem> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return command_items();
    }
    let stop_words = [
        "a", "an", "the", "to", "please", "i", "want", "can", "you", "me", "my", "this", "that",
        "for", "of",
    ];
    let tokens = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty() && !stop_words.contains(token))
        .collect::<Vec<_>>();

    let mut matches = command_items()
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut score = if item.title.to_lowercase().contains(&query) {
                500
            } else {
                0
            };
            for token in &tokens {
                score += fuzzy_command_token_score(&item.search_text, token)?;
            }
            Some((score, index, item))
        })
        .collect::<Vec<_>>();
    matches.sort_by(
        |(left_score, left_index, _), (right_score, right_index, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_index.cmp(right_index))
        },
    );
    matches.into_iter().map(|(_, _, item)| item).collect()
}

fn matching_open_tab_items(items: Vec<OverlayItem>, query: &str) -> Vec<OverlayItem> {
    let query = query.trim().to_lowercase();
    if query.contains('*') {
        let mut matches = items
            .into_iter()
            .enumerate()
            .filter_map(|(index, item)| {
                wildcard_score(&item.search_text, &query).map(|score| (score, index, item))
            })
            .collect::<Vec<_>>();
        matches.sort_by(
            |(left_score, left_index, _), (right_score, right_index, _)| {
                right_score
                    .cmp(left_score)
                    .then_with(|| left_index.cmp(right_index))
            },
        );
        return matches.into_iter().map(|(_, _, item)| item).collect();
    }

    let tokens = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return items;
    }

    let mut matches = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut score = 0;
            for token in &tokens {
                score += fuzzy_command_token_score(&item.search_text, token)?;
            }
            Some((score, index, item))
        })
        .collect::<Vec<_>>();
    matches.sort_by(
        |(left_score, left_index, _), (right_score, right_index, _)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_index.cmp(right_index))
        },
    );
    matches.into_iter().map(|(_, _, item)| item).collect()
}

fn wildcard_score(candidate: &str, query: &str) -> Option<i64> {
    let mut cursor = 0;
    let mut score = 0;
    let mut matched_any = false;
    for fragment in query.split('*').filter(|fragment| !fragment.is_empty()) {
        let relative = candidate[cursor..].find(fragment)?;
        matched_any = true;
        score += 100 - relative as i64;
        cursor += relative + fragment.len();
    }
    matched_any.then_some(score)
}

fn fuzzy_command_token_score(candidate: &str, token: &str) -> Option<i64> {
    if let Some(index) = candidate.find(token) {
        return Some(120 - (index as i64 / 8));
    }

    let mut score = 0i64;
    let mut wanted = token.chars();
    let mut current = wanted.next()?;
    let mut previous_match = None;
    for (index, character) in candidate.char_indices() {
        if character != current {
            continue;
        }
        score += 8;
        if previous_match.is_some_and(|previous| previous + character.len_utf8() == index) {
            score += 5;
        }
        previous_match = Some(index);
        let Some(next) = wanted.next() else {
            return Some(score);
        };
        current = next;
    }
    None
}

fn native_menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Textify".into(),
            items: vec![
                MenuItem::action("Settings…", ShowSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Textify", QuitTextify),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Tab", NewDocument),
                MenuItem::action("Open File…", OpenDocument),
                MenuItem::action("Open Recent…", ShowRecentFiles),
                MenuItem::action("Clear Recent Files", ClearRecentFiles),
                MenuItem::action("Open Folder…", OpenFolder),
                MenuItem::separator(),
                MenuItem::action("Save", SaveDocument),
                MenuItem::action("Save As…", SaveDocumentAs),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseDocument),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", ShowCommandPalette),
                MenuItem::action("Search Open Tabs…", SearchOpenTabs),
                MenuItem::action("Search Workspace…", ShowWorkspaceSearch),
                MenuItem::separator(),
                MenuItem::action("Toggle Word Wrap", ToggleWordWrap),
                MenuItem::action("Toggle Title Bar", ToggleTitleBar),
                MenuItem::action("Toggle File Explorer", ToggleSidebar),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Show Open Tabs…", ShowOpenTabs),
                MenuItem::separator(),
                MenuItem::action("Next Tab", NextDocument),
                MenuItem::action("Previous Tab", PreviousDocument),
            ],
        },
    ]
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
    push_key_binding(&mut bindings, &keymap.quick_open, SearchOpenTabs);
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
            KeyBinding::new("cmd-t", NewDocument, None),
            KeyBinding::new("cmd-o", OpenDocument, None),
            KeyBinding::new("cmd-s", SaveDocument, None),
            KeyBinding::new("cmd-shift-s", SaveDocumentAs, None),
            KeyBinding::new("cmd-w", CloseDocument, None),
            KeyBinding::new("cmd-,", ShowSettings, None),
            KeyBinding::new("alt-z", ToggleWordWrap, None),
            KeyBinding::new("ctrl-tab", NextDocument, None),
            KeyBinding::new("ctrl-shift-tab", PreviousDocument, None),
            KeyBinding::new("cmd-alt-p", ShowOpenTabs, None),
            KeyBinding::new("escape", DismissOverlay, None),
            KeyBinding::new("cmd-q", QuitTextify, None),
        ]);
        cx.set_menus(native_menus());

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
    fn dropped_paths_are_deduplicated_and_validated_off_the_ui_path() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let valid = directory.path().join("valid.txt");
        let invalid = directory.path().join("invalid.txt");
        fs::write(&valid, "hello\n").expect("valid fixture");
        fs::write(&invalid, [0xff, 0xfe]).expect("invalid fixture");

        let results = load_dropped_paths(
            vec![
                valid.clone(),
                directory.path().to_path_buf(),
                valid,
                invalid,
            ],
            FilePolicy::default(),
        );
        assert_eq!(results.len(), 3);
        assert_eq!(
            results.iter().filter(|(_, result)| result.is_ok()).count(),
            1
        );
        assert_eq!(
            results.iter().filter(|(_, result)| result.is_err()).count(),
            2
        );
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
                font_size_override: Some(17),
                word_wrap: true,
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
        assert_eq!(seed.font_size_override, Some(17));
        assert!(seed.word_wrap);
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
                workspace
                    .settings_font_select
                    .read(cx)
                    .selected_value()
                    .map(String::as_str),
                Some(workspace.settings.appearance.font_family.as_str())
            );
            let draft = workspace.settings_draft.as_ref().expect("draft");
            assert_eq!(draft.font_size, workspace.settings.appearance.font_size);
            assert_eq!(
                draft.show_tagline,
                workspace.settings.appearance.show_tagline
            );
            assert_eq!(draft.recovery, workspace.settings.recovery);
            assert_eq!(draft.recent_files, workspace.settings.recent_files);
            workspace.documents[0].dirty = true;
            assert_eq!(workspace.documents[0].title(cx), "Untitled 1 •");
        });
    }

    #[gpui::test]
    fn command_w_closes_a_clean_temporary_tab(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        cx.update(|cx| cx.bind_keys([KeyBinding::new("cmd-w", CloseDocument, None)]));
        let directory = tempfile::tempdir().expect("session directory");
        let workspace_slot = Rc::new(RefCell::new(None));
        let capture = workspace_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *capture.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = workspace_slot.borrow().clone().expect("workspace");
        let original_id = workspace.update(cx, |workspace, _| workspace.active_id());
        workspace.update(cx, |workspace, _| {
            workspace.session_path = directory.path().join("session.json")
        });

        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-w");
        cx.run_until_parked();
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_ne!(workspace.active_id(), original_id);
            assert_eq!(workspace.documents.len(), 1);
        });
    }

    #[gpui::test]
    fn command_w_prompts_and_save_closes_a_dirty_named_tab(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        cx.update(|cx| cx.bind_keys([KeyBinding::new("cmd-w", CloseDocument, None)]));
        let directory = tempfile::tempdir().expect("document directory");
        let path = directory.path().join("draft.txt");
        fs::write(&path, "before").expect("fixture");
        let workspace_slot = Rc::new(RefCell::new(None));
        let capture = workspace_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *capture.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = workspace_slot.borrow().clone().expect("workspace");
        let original_id = workspace.update(cx, |workspace, _| workspace.active_id());

        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.session_path = directory.path().join("session.json");
                let document = workspace.active_document_mut();
                document.metadata.path = Some(path.clone());
                document.disk_revision = optional_disk_revision(&path).expect("disk revision");
                document.editor.set_text("after".to_owned(), window, cx);
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("cmd-w");
        cx.run_until_parked();
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.active_id(), original_id);
            assert!(workspace.active_document().dirty);
        });

        cx.update(|window, cx| {
            window.close_dialog(cx);
            workspace.update(cx, |workspace, cx| {
                workspace.save_before_close(original_id, window, cx)
            });
        });
        cx.run_until_parked();
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_ne!(workspace.active_id(), original_id);
            assert!(!workspace.close_after_save.contains(&original_id));
        });
        assert_eq!(fs::read_to_string(path).expect("saved document"), "after");
    }

    #[test]
    fn graceful_quit_flushes_the_latest_recovery_before_session_state() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let recovery_directory = directory.path().join("Backups");
        let session_path = directory.path().join("session.json");
        let tab = SessionTab {
            path: None,
            recovery_path: None,
            untitled_number: 1,
            label_override: None,
            dirty: false,
            font_size_override: None,
            word_wrap: false,
        };
        let snapshots = vec![QuitSnapshot {
            id: 42,
            key: 99,
            revision: 7,
            rope: Rope::from("latest unsaved text 🦀"),
        }];

        let (failures, session_result) = persist_quit_state(
            vec![(42, tab)],
            0,
            None,
            snapshots,
            &recovery_directory,
            &session_path,
        );
        assert!(failures.is_empty());
        session_result.expect("session save");
        let session = load_session(&session_path).expect("session load");
        assert!(session.tabs[0].dirty);
        let recovery_path = session.tabs[0]
            .recovery_path
            .as_deref()
            .expect("recovery path");
        assert_eq!(
            load_snapshot(recovery_path).expect("recovery contents"),
            "latest unsaved text 🦀"
        );
    }

    #[test]
    fn trackpad_zoom_accumulates_while_wheel_zoom_steps_once() {
        let mut accumulator = 0.0;
        assert_eq!(
            consume_zoom_delta(
                ScrollDelta::Pixels(gpui::point(px(0.), px(20.))),
                &mut accumulator,
            ),
            0
        );
        assert_eq!(
            consume_zoom_delta(
                ScrollDelta::Pixels(gpui::point(px(0.), px(20.))),
                &mut accumulator,
            ),
            1
        );
        assert_eq!(
            consume_zoom_delta(ScrollDelta::Lines(gpui::point(0., -3.)), &mut accumulator),
            -1
        );
    }

    #[test]
    fn editor_font_picker_sorts_deduplicates_and_preserves_configuration() {
        assert_eq!(
            normalize_editor_font_families(
                vec![
                    "Zed Mono".to_owned(),
                    "alpha code".to_owned(),
                    "ALPHA CODE".to_owned(),
                    "".to_owned(),
                ],
                "Missing Mono",
            ),
            ["ALPHA CODE", "Missing Mono", "Zed Mono"]
        );
        assert_eq!(
            normalize_editor_font_families(
                vec!["SFMONO-REGULAR".to_owned(), "Other".to_owned()],
                "SFMono-Regular",
            ),
            ["Other", "SFMono-Regular"]
        );
    }

    #[test]
    fn untitled_titles_use_a_bounded_normalized_first_line() {
        assert_eq!(
            first_line_title("  A   useful\ttitle  \nignored".chars()),
            Some("A useful title".to_owned())
        );
        assert_eq!(first_line_title("\nsecond line".chars()), None);

        let long = "🦀".repeat(UNTITLED_TITLE_CHARS + 10);
        let title = first_line_title(long.chars()).expect("title");
        assert_eq!(title.chars().count(), UNTITLED_TITLE_CHARS + 1);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn command_palette_understands_natural_language_intent() {
        let first_command = |query: &str| {
            matching_command_items(query)
                .into_iter()
                .next()
                .and_then(|item| match item.target {
                    OverlayTarget::Command(command) => Some(command),
                    _ => None,
                })
        };

        assert_eq!(
            first_command("please make a new tab"),
            Some(IdeCommand::NewFile)
        );
        assert_eq!(
            first_command("save this somewhere else"),
            Some(IdeCommand::SaveFileAs)
        );
        assert_eq!(
            first_command("hide the file list"),
            Some(IdeCommand::ToggleSidebar)
        );
        assert_eq!(
            first_command("preferences for font"),
            Some(IdeCommand::OpenSettings)
        );
        assert_eq!(
            first_command("show me every open file"),
            Some(IdeCommand::OpenTabs)
        );
        assert_eq!(
            first_command("hide the top title bar"),
            Some(IdeCommand::ToggleTitleBar)
        );
        assert_eq!(
            first_command("open a recent file"),
            Some(IdeCommand::OpenRecent)
        );
        assert_eq!(
            first_command("search all open tabs"),
            Some(IdeCommand::SearchOpenTabs)
        );
    }

    #[test]
    fn open_tab_search_supports_fuzzy_wildcard_fragments() {
        let item = |id, title: &str, path: &str| OverlayItem {
            title: title.to_owned(),
            subtitle: path.to_owned(),
            search_text: format!("{title} {path}").to_lowercase(),
            target: OverlayTarget::Tab(id),
        };
        let matches = matching_open_tab_items(
            vec![
                item(1, "notes.txt", "/work/notes.txt"),
                item(2, "main.rs", "/work/src/main.rs"),
                item(3, "manual.md", "/work/docs/manual.md"),
            ],
            "ma*rs",
        );

        assert_eq!(matches.len(), 1);
        assert!(matches!(matches[0].target, OverlayTarget::Tab(2)));
    }

    #[test]
    fn open_tab_content_search_ranks_phrases_and_all_word_matches() {
        let documents = vec![
            OpenTabSearchDocument {
                id: 1,
                order: 0,
                title: "one.txt".to_owned(),
                subtitle: "Unsaved document".to_owned(),
                text: Rope::from("alpha second\nsecond then alpha\nNEEDLE café\n"),
            },
            OpenTabSearchDocument {
                id: 2,
                order: 1,
                title: "two.txt".to_owned(),
                subtitle: "/tmp/two.txt".to_owned(),
                text: Rope::from("alpha second again\n"),
            },
        ];
        let cancel = crate::huge_file::CancellationToken::default();
        let matches = search_open_tabs(&documents, "alpha second", 10, &cancel);
        assert_eq!(matches.len(), 3);
        assert_eq!((matches[0].id, matches[0].line), (1, 0));
        assert_eq!((matches[1].id, matches[1].line), (2, 0));
        assert_eq!((matches[2].id, matches[2].line), (1, 1));

        let case_insensitive = search_open_tabs(&documents, "needle", 10, &cancel);
        assert_eq!(case_insensitive[0].line, 2);
        assert_eq!(case_insensitive[0].column, 0);
        assert_eq!(case_insensitive[0].end_column, 6);

        cancel.cancel();
        assert!(search_open_tabs(&documents, "alpha", 10, &cancel).is_empty());
    }

    #[gpui::test]
    fn activating_an_overflowed_tab_reveals_it(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("session directory");
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
                workspace.session_path = directory.path().join("session.json");
                for index in 1..18 {
                    workspace.add_untitled(window, cx);
                    workspace.documents[index].label_override =
                        Some(format!("document-{index}.txt"));
                }
                workspace.documents[17].label_override = Some("overflow-target.rs".to_owned());
                workspace.show_overlay(OverlayMode::OpenTabs, window, cx);
                workspace.overlay_input.update(cx, |input, cx| {
                    input.set_value("target", window, cx);
                });
                workspace.refresh_overlay(window, cx);
                workspace.accept_overlay(0, window, cx);
            });
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, cx| {
            assert_eq!(workspace.active_index, 17);
            assert_eq!(
                workspace.active_document().display_name(cx),
                "overflow-target.rs"
            );
            let viewport = workspace.tab_scroll_handle.bounds();
            let tab = workspace
                .tab_scroll_handle
                .bounds_for_item(17)
                .expect("overflow tab bounds");
            let offset = workspace.tab_scroll_handle.offset();
            assert!(tab.left() + offset.x >= viewport.left());
            assert!(tab.right() + offset.x <= viewport.right());
        });
    }

    #[gpui::test]
    fn open_tabs_filters_and_navigates_with_the_keyboard(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("session directory");
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
                workspace.session_path = directory.path().join("session.json");
                workspace.documents[0].label_override = Some("document-00.txt".to_owned());
                for index in 1..16 {
                    workspace.add_untitled(window, cx);
                    workspace.documents[index].label_override = Some(if index == 15 {
                        "needle-notes.md".to_owned()
                    } else {
                        format!("document-{index:02}.txt")
                    });
                }
                workspace.set_active_index(0, window, cx);
                workspace.show_overlay(OverlayMode::OpenTabs, window, cx);
            });
        });
        cx.run_until_parked();
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.overlay_items.len(), 16);
            assert_eq!(workspace.overlay_selected_index, 0);
        });

        cx.update(|window, cx| {
            assert_eq!(
                window.focused_input(cx),
                Some(workspace.read(cx).overlay_input.clone())
            )
        });
        let down_keys = "down ".repeat(12);
        cx.simulate_keystrokes(down_keys.trim());
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.overlay_selected_index, 12);
        });
        cx.simulate_keystrokes("enter");
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.active_index, 12);
            assert_eq!(workspace.overlay_mode, None);
        });

        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.show_overlay(OverlayMode::OpenTabs, window, cx)
            });
        });
        cx.simulate_input("needle");
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.overlay_items.len(), 1);
            assert_eq!(workspace.overlay_items[0].title, "needle-notes.md");
        });
        cx.simulate_keystrokes("enter");
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.active_index, 15);
            assert_eq!(workspace.overlay_mode, None);
        });
    }

    #[gpui::test]
    fn recent_files_are_bounded_searchable_openable_and_clearable(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("history directory");
        let paths = ["alpha.txt", "beta.txt", "gamma.txt", "delta.txt"]
            .map(|name| directory.path().join(name));
        for path in &paths {
            fs::write(path, format!("contents of {}", path.display())).expect("fixture");
        }
        let workspace_slot = Rc::new(RefCell::new(None));
        let capture = workspace_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *capture.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = workspace_slot.borrow().clone().expect("workspace");
        let history_path = directory.path().join("recent-files.json");
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.data_dir = directory.path().to_path_buf();
                workspace.session_path = directory.path().join("session.json");
                workspace.recent_files_path = history_path.clone();
                workspace.recent_files.clear();
                workspace.settings.recent_files.enabled = true;
                workspace.settings.recent_files.max_files = 3;
                workspace.record_recent_file(paths[0].clone(), cx);
                workspace.record_recent_file(paths[1].clone(), cx);
                workspace.record_recent_file(paths[2].clone(), cx);
                workspace.record_recent_file(paths[0].clone(), cx);
                workspace.record_recent_file(paths[3].clone(), cx);
                workspace.show_overlay(OverlayMode::RecentFiles, window, cx);
            });
        });
        cx.run_until_parked();

        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(
                workspace.recent_files.paths,
                vec![paths[3].clone(), paths[0].clone(), paths[2].clone()]
            );
            assert_eq!(workspace.overlay_items.len(), 3);
        });
        assert_eq!(
            RecentFiles::load(&history_path, 3)
                .expect("persisted history")
                .paths,
            vec![paths[3].clone(), paths[0].clone(), paths[2].clone()]
        );

        cx.simulate_input("alpha");
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.overlay_items.len(), 1);
            assert_eq!(workspace.overlay_items[0].title, "alpha.txt");
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(
                workspace.active_document().metadata.path.as_deref(),
                Some(paths[0].as_path())
            );
        });
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.on_clear_recent_files(&ClearRecentFiles, window, cx)
            });
        });
        cx.run_until_parked();
        workspace.update(&mut cx.cx, |workspace, _| {
            assert!(workspace.recent_files.paths.is_empty());
        });
        assert!(
            RecentFiles::load(&history_path, 3)
                .expect("cleared history")
                .paths
                .is_empty()
        );
    }

    #[gpui::test]
    fn search_open_tabs_finds_unsaved_text_and_jumps_to_the_match(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("session directory");
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
                workspace.session_path = directory.path().join("session.json");
                workspace.documents[0].label_override = Some("first draft".to_owned());
                workspace.documents[0].editor.set_text(
                    "heading\nfind me in the first draft".to_owned(),
                    window,
                    cx,
                );
                workspace.add_untitled(window, cx);
                workspace.documents[1].label_override = Some("second draft".to_owned());
                workspace.documents[1].editor.set_text(
                    "another heading\nfind me in the second draft".to_owned(),
                    window,
                    cx,
                );
                workspace.set_active_index(0, window, cx);
                workspace.show_overlay(OverlayMode::OpenTabSearch, window, cx);
            });
        });
        cx.run_until_parked();
        cx.simulate_input("find me");
        cx.run_until_parked();
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.overlay_items.len(), 2);
            assert!(workspace.overlay_items[0].subtitle.contains("first draft"));
            assert!(workspace.overlay_items[1].subtitle.contains("second draft"));
        });

        cx.simulate_keystrokes("down enter");
        workspace.update(&mut cx.cx, |workspace, cx| {
            assert_eq!(workspace.active_index, 1);
            assert_eq!(workspace.overlay_mode, None);
            assert_eq!(
                workspace
                    .active_document()
                    .editor
                    .state()
                    .read(cx)
                    .cursor_position()
                    .line,
                1
            );
        });
    }

    #[gpui::test]
    fn command_palette_dismisses_from_escape_and_the_backdrop(cx: &mut gpui::TestAppContext) {
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
                workspace.show_overlay(OverlayMode::Commands, window, cx);
            });
        });
        cx.simulate_keystrokes("escape");
        cx.simulate_input("x");
        workspace.update(&mut cx.cx, |workspace, cx| {
            assert_eq!(workspace.overlay_mode, None);
            assert_eq!(workspace.active_document().editor.rope(cx).to_string(), "x");
        });

        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.show_overlay(OverlayMode::Commands, window, cx);
            });
        });
        cx.run_until_parked();
        cx.simulate_click(gpui::point(px(200.), px(90.)), gpui::Modifiers::none());
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.overlay_mode, Some(OverlayMode::Commands));
        });
        cx.simulate_click(gpui::point(px(20.), px(180.)), gpui::Modifiers::none());
        workspace.update(&mut cx.cx, |workspace, _| {
            assert_eq!(workspace.overlay_mode, None);
        });
    }

    #[test]
    fn native_menu_bar_exposes_editor_commands_without_window_chrome() {
        let menus = native_menus();
        assert_eq!(
            menus
                .iter()
                .map(|menu| menu.name.as_ref())
                .collect::<Vec<_>>(),
            ["Textify", "File", "Edit", "View", "Window"]
        );
        let view = menus
            .iter()
            .find(|menu| menu.name == "View")
            .expect("View menu");
        let actions = view
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { name, .. } => Some(name.as_ref()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(actions.contains(&"Toggle Word Wrap"));
        assert!(actions.contains(&"Toggle Title Bar"));
        assert!(actions.contains(&"Command Palette…"));
        assert!(actions.contains(&"Search Open Tabs…"));

        let file = menus
            .iter()
            .find(|menu| menu.name == "File")
            .expect("File menu");
        let file_actions = file
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { name, .. } => Some(name.as_ref()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(file_actions.contains(&"Open Recent…"));
        assert!(file_actions.contains(&"Clear Recent Files"));
    }

    #[gpui::test]
    fn title_bar_toggle_is_persisted(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("settings directory");
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
                workspace.data_dir = directory.path().to_path_buf();
                assert!(workspace.settings.appearance.show_title_bar);
                workspace.on_toggle_title_bar(&ToggleTitleBar, window, cx);
                assert!(!workspace.settings.appearance.show_title_bar);
            });
        });
        cx.run_until_parked();

        let saved =
            TextifySettings::load(&directory.path().join("settings.json")).expect("saved settings");
        assert!(!saved.appearance.show_title_bar);
    }

    #[gpui::test]
    fn tagline_setting_is_staged_saved_and_applied(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("settings directory");
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
                workspace.data_dir = directory.path().to_path_buf();
                workspace.show_settings(window, cx);
                workspace
                    .settings_draft
                    .as_mut()
                    .expect("settings draft")
                    .show_tagline = false;
                workspace.save_settings_window(window, cx);
            });
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, _| {
            assert!(!workspace.settings.appearance.show_tagline);
            assert!(!workspace.settings_visible);
        });
        let saved =
            TextifySettings::load(&directory.path().join("settings.json")).expect("saved settings");
        assert!(!saved.appearance.show_tagline);
    }

    #[gpui::test]
    fn disabling_recent_files_in_settings_clears_local_history(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("settings directory");
        let history_path = directory.path().join("recent-files.json");
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
                workspace.data_dir = directory.path().to_path_buf();
                workspace.session_path = directory.path().join("session.json");
                workspace.recent_files_path = history_path.clone();
                workspace.recent_files.paths = vec![PathBuf::from("private-note.txt")];
                workspace.show_settings(window, cx);
                let draft = workspace.settings_draft.as_mut().expect("settings draft");
                draft.recent_files.enabled = false;
                draft.recent_files.max_files = 25;
                workspace.save_settings_window(window, cx);
            });
        });
        cx.run_until_parked();

        workspace.update(cx, |workspace, _| {
            assert!(!workspace.settings.recent_files.enabled);
            assert_eq!(workspace.settings.recent_files.max_files, 25);
            assert!(workspace.recent_files.paths.is_empty());
        });
        let saved =
            TextifySettings::load(&directory.path().join("settings.json")).expect("saved settings");
        assert!(!saved.recent_files.enabled);
        assert!(
            RecentFiles::load(&history_path, 10)
                .expect("cleared history")
                .paths
                .is_empty()
        );
    }

    #[gpui::test]
    fn word_wrap_is_per_tab_and_blocked_for_large_files(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("session directory");
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
                workspace.session_path = directory.path().join("session.json");
                workspace.on_toggle_word_wrap(&ToggleWordWrap, window, cx);
                assert!(workspace.documents[0].word_wrap);

                workspace.add_untitled(window, cx);
                assert!(!workspace.documents[1].word_wrap);
                workspace.documents[1].metadata.mode = FileMode::Large;
                workspace.on_toggle_word_wrap(&ToggleWordWrap, window, cx);
                assert!(!workspace.documents[1].word_wrap);
                assert_eq!(
                    workspace.status_message.as_deref(),
                    Some("Word wrap is disabled by large-file policy")
                );
            });
        });
    }

    #[gpui::test]
    fn command_scroll_zoom_is_isolated_to_the_active_tab(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let directory = tempfile::tempdir().expect("session directory");
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
                workspace.session_path = directory.path().join("session.json");
                let default_size = workspace.settings.appearance.font_size;
                workspace.on_editor_scroll(
                    &ScrollWheelEvent {
                        delta: ScrollDelta::Lines(gpui::point(0., 1.)),
                        modifiers: gpui::Modifiers {
                            platform: true,
                            ..gpui::Modifiers::default()
                        },
                        ..ScrollWheelEvent::default()
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    workspace.documents[0].font_size_override,
                    Some(default_size + 1)
                );

                workspace.add_untitled(window, cx);
                workspace.on_editor_scroll(
                    &ScrollWheelEvent {
                        delta: ScrollDelta::Lines(gpui::point(0., -1.)),
                        modifiers: gpui::Modifiers {
                            platform: true,
                            ..gpui::Modifiers::default()
                        },
                        ..ScrollWheelEvent::default()
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    workspace.documents[0].font_size_override,
                    Some(default_size + 1)
                );
                assert_eq!(
                    workspace.documents[1].font_size_override,
                    Some(default_size - 1)
                );
            });
        });
    }
}

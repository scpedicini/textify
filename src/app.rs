use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{
    App, AppContext as _, Application, Context, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Render, Styled, Subscription, Timer, Window, WindowBounds, WindowOptions,
    actions, div, prelude::FluentBuilder as _, px, size,
};
use gpui_component::{
    ActiveTheme as _, IconName, Root, Sizable as _, StyledExt as _, Theme, ThemeMode, TitleBar,
    WindowExt,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{InputEvent, RopeExt as _},
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_component_assets::Assets;

use crate::{
    document::{DocumentMetadata, FileAnalysis, FileMode, FilePolicy, Language},
    editor::EditorBackend,
    file_io::{
        DiskRevision, ExternalFileChanged, LoadedFile, load_utf8, optional_disk_revision,
        save_atomic_chunks_checked, suggested_save_path,
    },
    session::{SessionState, load_session, save_session},
    settings::{TextifySettings, textify_data_dir},
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
        PreviousDocument
    ]
);

const WINDOW_TITLE: &str = "Textify IDE";

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
    _subscription: Subscription,
}

struct DocumentSeed {
    text: String,
    metadata: DocumentMetadata,
    disk_revision: Option<DiskRevision>,
    label_override: Option<String>,
    untitled_number: usize,
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
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let data_dir = textify_data_dir();
        let settings =
            TextifySettings::load(&data_dir.join("settings.json")).unwrap_or_else(|error| {
                tracing::warn!(%error, "using default settings");
                TextifySettings::default()
            });
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
        };
        workspace.add_untitled(window, cx);
        workspace
    }

    fn active_document(&self) -> &EditorDocument {
        &self.documents[self.active_index]
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

    fn push_document(&mut self, seed: DocumentSeed, window: &mut Window, cx: &mut Context<Self>) {
        let DocumentSeed {
            text,
            metadata,
            disk_revision,
            label_override,
            untitled_number,
        } = seed;
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
                if let Some(document) = workspace
                    .documents
                    .iter_mut()
                    .find(|document| document.id == id)
                    && !document.programmatic_change
                {
                    document.dirty = true;
                    document.revision = document.revision.wrapping_add(1);
                }
                cx.notify();
            }
        });

        self.documents.push(EditorDocument {
            id,
            untitled_number,
            editor: editor.clone(),
            metadata,
            dirty: false,
            saving: false,
            revision: 0,
            disk_revision,
            external_changed: false,
            programmatic_change: false,
            label_override,
            _subscription: subscription,
        });
        self.active_index = self.documents.len() - 1;
        self.update_window_title(window);

        window.defer(cx, move |window, cx| editor.focus(window, cx));
        cx.notify();
    }

    fn set_active_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.documents.len() {
            return;
        }

        self.active_index = index;
        self.active_document().editor.focus(window, cx);
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

        let open_paths = self
            .documents
            .iter()
            .filter_map(|document| document.metadata.path.clone())
            .collect::<Vec<_>>();
        let active_path = self.active_document().metadata.path.as_ref();
        let active_index = active_path
            .and_then(|active| open_paths.iter().position(|path| path == active))
            .unwrap_or(0);
        let state = SessionState::new(active_index, open_paths);
        let path = self.session_path.clone();
        cx.background_spawn(async move {
            if let Err(error) = save_session(&path, &state) {
                tracing::warn!(%error, "could not persist session");
            }
        })
        .detach();
    }

    fn start_background_services(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_session(window, cx);
        self.poll_watcher(window, cx);
    }

    fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restoring_session = true;
        let path = self.session_path.clone();
        let policy = self.policy;
        let task = cx.background_spawn(async move {
            let session = load_session(&path)?;
            let active_index = session.active_index;
            let loaded = session
                .open_paths
                .into_iter()
                .map(|path| load_utf8(&path, policy).map_err(|error| (path, error)))
                .collect::<Vec<_>>();
            Ok::<_, anyhow::Error>((active_index, loaded))
        });

        cx.spawn_in(window, async move |workspace, window| {
            let restored = task.await;
            workspace
                .update_in(window, |workspace, window, cx| {
                    workspace.restoring_session = false;
                    match restored {
                        Ok((active_index, loaded)) => {
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
                                    Ok(loaded) => workspace.push_loaded(loaded, window, cx),
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

        let targets = self
            .documents
            .iter()
            .filter_map(|document| {
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
                        },
                        window,
                        cx,
                    );
                })
                .ok()
        })
        .detach();
    }

    fn on_new(&mut self, _: &NewDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.add_untitled(window, cx);
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
            let task = window.background_spawn(async move { load_utf8(&load_path, policy) });
            let loaded = task.await;
            let elapsed = started_at.elapsed();

            workspace
                .update_in(window, |workspace, window, cx| match loaded {
                    Ok(loaded) => {
                        tracing::info!(
                            path = %path.display(),
                            elapsed_ms = elapsed.as_secs_f64() * 1000.0,
                            "opened document"
                        );
                        workspace.push_loaded(loaded, window, cx)
                    }
                    Err(error) => Self::show_error("Could not open file", error, window, cx),
                })
                .ok()
        })
        .detach();
    }

    fn on_save(&mut self, _: &SaveDocument, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.active_id();
        if let Some(path) = self.active_document().metadata.path.clone() {
            self.start_save(id, path, window, cx);
        } else {
            self.prompt_save_as(id, window, cx);
        }
    }

    fn on_save_as(&mut self, _: &SaveDocumentAs, window: &mut Window, cx: &mut Context<Self>) {
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
                        if document.revision == revision {
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
                        workspace.persist_session(cx);
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
        self.documents.remove(index);

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

        self.active_document().editor.focus(window, cx);
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
                                workspace.add_untitled(window, cx)
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
                    } else {
                        "Save"
                    })
                    .loading(self.active_document().saving)
                    .tooltip_with_action("Save file", &SaveDocument, None)
                    .on_click(cx.listener(|workspace, _, window, cx| {
                        workspace.on_save(&SaveDocument, window, cx)
                    })),
            )
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.active_document();
        let input = document.editor.state().read(cx);
        let cursor = input.cursor_position();
        let line_count = input.text().lines_len();
        let parser_suppressed = document.metadata.language != Language::PlainText
            && document.metadata.parser_name(self.policy).is_none();
        let path = document
            .metadata
            .path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Unsaved document".to_owned());

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
                    .child(format!("{} lines", line_count))
                    .child(format!(
                        "Ln {}, Col {}",
                        cursor.line + 1,
                        cursor.character + 1
                    ))
                    .child("UTF-8")
                    .child(document.metadata.analysis.line_ending.label())
                    .child(document.metadata.language.label()),
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

        v_flex()
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
                div()
                    .id("editor-surface")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(cx.theme().background)
                    .child(editor.render(cx).size_full()),
            )
            .child(self.render_status(cx))
    }
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
            KeyBinding::new("ctrl-tab", NextDocument, None),
            KeyBinding::new("ctrl-shift-tab", PreviousDocument, None),
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

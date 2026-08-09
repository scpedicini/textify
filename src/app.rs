use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext as _, Application, Context, InteractiveElement as _, IntoElement, KeyBinding,
    ParentElement as _, Render, Styled, Subscription, Window, WindowBounds, WindowOptions, actions,
    div, prelude::FluentBuilder as _, px, size,
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
    file_io::{LoadedFile, load_utf8, save_atomic_chunks, suggested_save_path},
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
    _subscription: Subscription,
}

impl EditorDocument {
    fn display_name(&self) -> String {
        self.metadata.display_name(self.untitled_number)
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
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut workspace = Self {
            documents: Vec::new(),
            active_index: 0,
            next_id: 1,
            next_untitled_number: 1,
            policy: FilePolicy::default(),
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
        self.push_document(String::new(), metadata, untitled_number, window, cx);
    }

    fn push_loaded(&mut self, loaded: LoadedFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.documents.iter().position(|document| {
            document.metadata.path.as_deref() == loaded.metadata.path.as_deref()
        }) {
            self.set_active_index(index, window, cx);
            return;
        }

        self.push_document(loaded.text, loaded.metadata, 0, window, cx);
    }

    fn push_document(
        &mut self,
        text: String,
        metadata: DocumentMetadata,
        untitled_number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        let editor = EditorBackend::new(
            text,
            metadata.parser_name(self.policy),
            metadata.mode,
            window,
            cx,
        );
        let subscription = cx.subscribe(editor.state(), move |workspace, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(document) = workspace
                    .documents
                    .iter_mut()
                    .find(|document| document.id == id)
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
        cx.notify();
    }

    fn update_window_title(&self, window: &mut Window) {
        window.set_window_title(&format!(
            "{} — Textify",
            self.active_document().display_name()
        ));
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
            let task = window.background_spawn(async move { load_utf8(&path, policy) });
            let loaded = task.await;

            workspace
                .update_in(window, |workspace, window, cx| match loaded {
                    Ok(loaded) => workspace.push_loaded(loaded, window, cx),
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
        let task = cx.background_spawn(async move {
            let analysis = FileAnalysis::from_str_chunks(rope.chunks());
            let result = save_atomic_chunks(&path, rope.chunks());
            (path, analysis, result)
        });
        let policy = self.policy;
        cx.notify();

        cx.spawn_in(window, async move |workspace, window| {
            let (path, analysis, result) = task.await;
            workspace
                .update_in(window, |workspace, window, cx| match result {
                    Ok(()) => {
                        let Some(index) = workspace.document_index(id) else {
                            return;
                        };
                        let metadata =
                            DocumentMetadata::new(Some(path), analysis, workspace.policy);
                        let parser = metadata.parser_name(policy);
                        let editor = workspace.documents[index].editor.clone();
                        let document = &mut workspace.documents[index];
                        document.metadata = metadata;
                        document.saving = false;
                        if document.revision == revision {
                            document.dirty = false;
                        }
                        editor.set_parser(parser, cx);
                        workspace.update_window_title(window);
                        cx.notify();
                    }
                    Err(error) => {
                        if let Some(index) = workspace.document_index(id) {
                            workspace.documents[index].saving = false;
                        }
                        Self::show_error("Could not save file", error, window, cx);
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
                cx.new(|cx| Root::new(workspace, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
        cx.activate(true);
    });
}

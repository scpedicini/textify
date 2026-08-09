//! The only module that knows how Textify configures GPUI Component's editor.

use gpui::{App, AppContext as _, Entity, Styled as _, Window};
use gpui_component::{
    highlighter::{Diagnostic, DiagnosticSeverity},
    input::{Input, InputState, Position, Rope, RopeExt as _, TabSize},
};

use crate::{document::FileMode, lsp::LspDiagnostic, settings::EditorBudgets};

#[derive(Clone)]
pub struct EditorBackend {
    state: Entity<InputState>,
}

impl EditorBackend {
    pub fn new(
        text: String,
        parser: Option<&'static str>,
        mode: FileMode,
        budgets: EditorBudgets,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        // `text` is GPUI Component's explicit plain-text language and never selects a parser.
        let language = parser.unwrap_or("text");
        let (undo_bytes, search_matches) = budgets.for_mode(mode);
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor(language)
                .undo_max_bytes(undo_bytes)
                .search_max_matches(search_matches)
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                // Wrapping is disabled in all modes initially; it is especially important for
                // pathological long lines in large-file mode.
                .soft_wrap(false)
                .default_value(text)
                .placeholder(if mode == FileMode::Large {
                    "Large-file mode"
                } else {
                    "Start writing…"
                })
        });

        Self { state }
    }

    pub fn state(&self) -> &Entity<InputState> {
        &self.state
    }

    pub fn rope(&self, cx: &App) -> Rope {
        self.state.read(cx).text().clone()
    }

    pub fn set_parser(&self, parser: Option<&'static str>, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_highlighter(parser.unwrap_or("text"), cx);
        });
    }

    pub fn set_text(&self, text: String, window: &mut Window, cx: &mut App) {
        self.state
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }

    pub fn set_budgets(&self, budgets: EditorBudgets, mode: FileMode, cx: &mut App) {
        let (undo_bytes, search_matches) = budgets.for_mode(mode);
        self.state.update(cx, |state, cx| {
            state.set_resource_budgets(undo_bytes, search_matches, cx)
        });
    }

    pub fn select_position(
        &self,
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.state.update(cx, |state, cx| {
            let start = state
                .text()
                .position_to_offset(&Position::new(line as u32, column as u32));
            let end = state
                .text()
                .position_to_offset(&Position::new(end_line as u32, end_column as u32));
            state.set_selections(std::iter::once(start..end).collect(), window, cx);
            state.focus(window, cx);
        });
    }

    pub fn set_diagnostics(&self, diagnostics: Vec<LspDiagnostic>, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            let text = state.text().clone();
            let Some(set) = state.diagnostics_mut() else {
                return;
            };
            set.reset(&text);
            set.extend(diagnostics.into_iter().map(|item| {
                Diagnostic::new(
                    Position::new(item.start_line as u32, item.start_character as u32)
                        ..Position::new(item.end_line as u32, item.end_character as u32),
                    item.message,
                )
                .with_severity(match item.severity {
                    1 => DiagnosticSeverity::Error,
                    2 => DiagnosticSeverity::Warning,
                    3 => DiagnosticSeverity::Info,
                    _ => DiagnosticSeverity::Hint,
                })
                .with_source("LSP")
            }));
            cx.notify();
        });
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn render(&self, font_family: &str, font_size: u16, _cx: &App) -> Input {
        Input::new(&self.state)
            .font_family(font_family.to_owned())
            .text_size(gpui::px(font_size as f32))
    }
}

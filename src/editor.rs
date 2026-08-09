//! The only module that knows how Textify configures GPUI Component's editor.

use gpui::{App, AppContext as _, Entity, Styled as _, Window};
use gpui_component::{
    highlighter::{Diagnostic, DiagnosticSeverity},
    input::{Input, InputState, Position, Rope, RopeExt as _, TabSize},
};

use crate::{
    document::FileMode,
    lsp::LspDiagnostic,
    settings::{EditorBudgets, IndentationSettings},
};

#[derive(Clone)]
pub struct EditorBackend {
    state: Entity<InputState>,
}

#[derive(Debug, Clone, Copy)]
pub struct EditorConfiguration {
    pub budgets: EditorBudgets,
    pub indentation: IndentationSettings,
    pub soft_wrap: bool,
}

impl EditorBackend {
    pub fn new(
        text: String,
        parser: Option<&'static str>,
        mode: FileMode,
        configuration: EditorConfiguration,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        // `text` is GPUI Component's explicit plain-text language and never selects a parser.
        let language = parser.unwrap_or("text");
        let (undo_bytes, search_matches) = configuration.budgets.for_mode(mode);
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor(language)
                .undo_max_bytes(undo_bytes)
                .search_max_matches(search_matches)
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: configuration.indentation.tab_width,
                    hard_tabs: configuration.indentation.hard_tabs,
                })
                // Large-file policy always wins over a restored per-tab preference.
                .soft_wrap(configuration.soft_wrap && mode == FileMode::Normal)
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

    pub fn set_indentation(&self, indentation: IndentationSettings, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_tab_size(
                TabSize {
                    tab_size: indentation.tab_width,
                    hard_tabs: indentation.hard_tabs,
                },
                cx,
            )
        });
    }

    pub fn set_soft_wrap(&self, wrap: bool, window: &mut Window, cx: &mut App) {
        self.state
            .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
    }

    pub fn preserve_cursor_anchor(&self, cx: &mut App) -> bool {
        self.state
            .update(cx, |state, _| state.preserve_cursor_anchor())
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
            .bordered(false)
            .focus_bordered(false)
            .font_family(font_family.to_owned())
            .text_size(gpui::px(font_size as f32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EditorHarness {
        editor: EditorBackend,
    }

    impl gpui::Render for EditorHarness {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            self.editor.render("SFMono-Regular", 14, cx)
        }
    }

    #[gpui::test]
    fn editor_surface_does_not_draw_generic_input_chrome(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (_, _) = cx.add_window_view(|window, cx| {
            let editor = EditorBackend::new(
                String::new(),
                None,
                FileMode::Normal,
                EditorConfiguration {
                    budgets: EditorBudgets::default(),
                    indentation: IndentationSettings::default(),
                    soft_wrap: false,
                },
                window,
                cx,
            );
            let input = editor.render("SFMono-Regular", 14, cx);
            assert!(!input.has_border());
            EditorHarness { editor }
        });
    }
}

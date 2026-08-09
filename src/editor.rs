//! The only module that knows how Textify configures GPUI Component's editor.

use gpui::{App, AppContext as _, Entity, Styled as _, Window};
use gpui_component::{
    ActiveTheme,
    input::{Input, InputState, Rope, TabSize},
};

use crate::{document::FileMode, settings::EditorBudgets};

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

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn render(&self, cx: &App) -> Input {
        Input::new(&self.state)
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(cx.theme().mono_font_size)
    }
}

//! A text input field that allows the user to enter text.
//!
//! Based on the `Input` example from the `gpui` crate.
//! https://github.com/zed-industries/zed/blob/main/crates/gpui/examples/input.rs
use anyhow::Result;
use gpui::{
    Action, App, AppContext, Bounds, ClipboardItem, Context, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, Render, ScrollHandle, ScrollWheelEvent, SharedString, Styled as _, Subscription,
    Task, UTF16Selection, Window, actions, div, point, prelude::FluentBuilder as _, px,
};
use ropey::{Rope, RopeSlice};
use serde::Deserialize;
use std::ops::Range;
use std::rc::Rc;
use sum_tree::Bias;
use unicode_segmentation::*;

use super::{
    blink_cursor::BlinkCursor,
    change::Change,
    element::TextElement,
    mask_pattern::MaskPattern,
    mode::InputMode,
    multicursor::{
        TaggedSelection, normalized_selections, rectangular_selections, replacement_plan,
    },
    number_input,
    text_wrapper::TextWrapper,
};
use crate::Size;
use crate::actions::{SelectDown, SelectLeft, SelectRight, SelectUp};
use crate::input::movement::MoveDirection;
use crate::input::{
    HoverDefinition, Lsp, Position,
    element::RIGHT_MARGIN,
    popovers::{ContextMenu, DiagnosticPopover, HoverPopover, MouseContextMenu},
    search::{self, SearchPanel},
    text_wrapper::LineLayout,
};
use crate::input::{InlineCompletion, RopeExt as _, Selection};
use crate::{Root, history::History};
use crate::{highlighter::DiagnosticSet, input::text_wrapper::LineItem};

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = input, no_json)]
pub struct Enter {
    /// Is confirm with secondary.
    pub secondary: bool,
}

actions!(
    input,
    [
        Backspace,
        Delete,
        DeleteToBeginningOfLine,
        DeleteToEndOfLine,
        DeleteToPreviousWordStart,
        DeleteToNextWordEnd,
        Indent,
        Outdent,
        IndentInline,
        OutdentInline,
        MoveUp,
        MoveDown,
        MoveLeft,
        MoveRight,
        MoveHome,
        MoveEnd,
        MovePageUp,
        MovePageDown,
        SelectAll,
        SelectToStartOfLine,
        SelectToEndOfLine,
        SelectToStart,
        SelectToEnd,
        SelectToPreviousWordStart,
        SelectToNextWordEnd,
        ShowCharacterPalette,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        MoveToStartOfLine,
        MoveToEndOfLine,
        MoveToStart,
        MoveToEnd,
        MoveToPreviousWord,
        MoveToNextWord,
        Escape,
        ToggleCodeActions,
        Search,
        GoToDefinition,
    ]
);

#[derive(Clone)]
pub enum InputEvent {
    Change,
    PressEnter { secondary: bool },
    Focus,
    Blur,
}

pub(super) const CONTEXT: &str = "Input";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-backspace", DeleteToBeginningOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-delete", DeleteToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-backspace", DeleteToPreviousWordStart, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-backspace", DeleteToPreviousWordStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-delete", DeleteToNextWordEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-delete", DeleteToNextWordEnd, Some(CONTEXT)),
        KeyBinding::new("enter", Enter { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("secondary-enter", Enter { secondary: true }, Some(CONTEXT)),
        KeyBinding::new("escape", Escape, Some(CONTEXT)),
        KeyBinding::new("up", MoveUp, Some(CONTEXT)),
        KeyBinding::new("down", MoveDown, Some(CONTEXT)),
        KeyBinding::new("left", MoveLeft, Some(CONTEXT)),
        KeyBinding::new("right", MoveRight, Some(CONTEXT)),
        KeyBinding::new("pageup", MovePageUp, Some(CONTEXT)),
        KeyBinding::new("pagedown", MovePageDown, Some(CONTEXT)),
        KeyBinding::new("tab", IndentInline, Some(CONTEXT)),
        KeyBinding::new("shift-tab", OutdentInline, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-]", Indent, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-]", Indent, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-[", Outdent, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-[", Outdent, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("home", MoveHome, Some(CONTEXT)),
        KeyBinding::new("end", MoveEnd, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectToStartOfLine, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-shift-a", SelectToStartOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-shift-e", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("shift-cmd-left", SelectToStartOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("shift-cmd-right", SelectToEndOfLine, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-shift-left", SelectToPreviousWordStart, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-left", SelectToPreviousWordStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-shift-right", SelectToNextWordEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-right", SelectToNextWordEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-a", SelectAll, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-x", Cut, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-x", Cut, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-v", Paste, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-a", MoveHome, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-left", MoveHome, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("ctrl-e", MoveEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-right", MoveEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-z", Undo, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-z", Redo, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-up", MoveToStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-down", MoveToEnd, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-left", MoveToPreviousWord, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-right", MoveToNextWord, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-left", MoveToPreviousWord, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-right", MoveToNextWord, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-up", SelectToStart, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-down", SelectToEnd, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-z", Undo, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-y", Redo, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-.", ToggleCodeActions, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-.", ToggleCodeActions, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-f", Search, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-f", Search, Some(CONTEXT)),
    ]);

    search::init(cx);
    number_input::init(cx);
}

#[derive(Clone)]
pub(super) struct LastLayout {
    /// The visible range (no wrap) of lines in the viewport, the value is row (0-based) index.
    pub(super) visible_range: Range<usize>,
    /// The first visible line top position in scroll viewport.
    pub(super) visible_top: Pixels,
    /// The range of byte offset of the visible lines.
    pub(super) visible_range_offset: Range<usize>,
    /// The last layout lines (Only have visible lines).
    pub(super) lines: Rc<Vec<LineLayout>>,
    /// The line_height of text layout, this will change will InputElement painted.
    pub(super) line_height: Pixels,
    /// The wrap width of text layout, this will change will InputElement painted.
    pub(super) wrap_width: Option<Pixels>,
    /// The line number area width of text layout, if not line number, this will be 0px.
    pub(super) line_number_width: Pixels,
    /// The cursor position (top, left) in pixels.
    pub(super) cursor_bounds: Option<Bounds<Pixels>>,
}

#[derive(Clone, Copy)]
pub(crate) struct ZoomAnchor {
    pub(crate) offset: usize,
    pub(crate) viewport_y: Pixels,
}

impl LastLayout {
    /// Get the line layout for the given row (0-based).
    ///
    /// 0 is the viewport first visible line.
    ///
    /// Returns None if the row is out of range.
    pub(crate) fn line(&self, row: usize) -> Option<&LineLayout> {
        if row < self.visible_range.start || row >= self.visible_range.end {
            return None;
        }

        self.lines.get(row.saturating_sub(self.visible_range.start))
    }
}

/// InputState to keep editing state of the [`super::Input`].
pub struct InputState {
    pub(super) focus_handle: FocusHandle,
    pub(super) mode: InputMode,
    pub(super) text: Rope,
    pub(super) text_wrapper: TextWrapper,
    pub(super) history: History<Change>,
    pub(super) blink_cursor: Entity<BlinkCursor>,
    pub(super) loading: bool,
    /// Range in UTF-8 length for the selected text.
    ///
    /// - "Hello 世界💝" = 16
    /// - "💝" = 4
    pub(super) selected_range: Selection,
    /// Additional disjoint selections. The primary selection remains `selected_range` so the
    /// platform input handler and upstream editor behavior continue to have one active IME range.
    pub(super) secondary_selected_ranges: Vec<Selection>,
    pub(super) search_panel: Option<Entity<SearchPanel>>,
    pub(super) searchable: bool,
    pub(super) search_max_matches: usize,
    /// Range for save the selected word, use to keep word range when drag move.
    pub(super) selected_word_range: Option<Selection>,
    pub(super) selection_reversed: bool,
    /// The marked range is the temporary insert text on IME typing.
    pub(super) ime_marked_range: Option<Selection>,
    pub(super) last_layout: Option<LastLayout>,
    pub(super) last_cursor: Option<usize>,
    /// The input container bounds
    pub(super) input_bounds: Bounds<Pixels>,
    /// The text bounds
    pub(super) last_bounds: Option<Bounds<Pixels>>,
    pub(super) last_selected_range: Option<Selection>,
    pub(super) selecting: bool,
    pub(super) rectangular_anchor: Option<usize>,
    pub(super) batch_editing: bool,
    pub(super) size: Size,
    pub(super) disabled: bool,
    pub(super) masked: bool,
    pub(super) clean_on_escape: bool,
    pub(super) soft_wrap: bool,
    pub(super) pattern: Option<regex::Regex>,
    pub(super) validate: Option<Box<dyn Fn(&str, &mut Context<Self>) -> bool + 'static>>,
    pub(crate) scroll_handle: ScrollHandle,
    /// The deferred scroll offset to apply on next layout.
    pub(crate) deferred_scroll_offset: Option<Point<Pixels>>,
    /// A one-shot document position to preserve across a font or layout scale change.
    pub(crate) zoom_anchor: Option<ZoomAnchor>,
    /// The size of the scrollable content.
    pub(crate) scroll_size: gpui::Size<Pixels>,

    /// The mask pattern for formatting the input text
    pub(crate) mask_pattern: MaskPattern,
    pub(super) placeholder: SharedString,

    /// Popover
    diagnostic_popover: Option<Entity<DiagnosticPopover>>,
    /// Completion/CodeAction context menu
    pub(super) context_menu: Option<ContextMenu>,
    pub(super) mouse_context_menu: Entity<MouseContextMenu>,
    /// A flag to indicate if we are currently inserting a completion item.
    pub(super) completion_inserting: bool,
    pub(super) hover_popover: Option<Entity<HoverPopover>>,
    /// The LSP definitions locations for "Go to Definition" feature.
    pub(super) hover_definition: HoverDefinition,

    pub lsp: Lsp,

    /// A flag to indicate if we have a pending update to the text.
    ///
    /// If true, will call some update (for example LSP, Syntax Highlight) before render.
    _pending_update: bool,
    /// A flag to indicate if we should ignore the next completion event.
    pub(super) silent_replace_text: bool,

    /// To remember the horizontal column (x-coordinate) of the cursor position for keep column for move up/down.
    ///
    /// The first element is the x-coordinate (Pixels), preferred to use this.
    /// The second element is the column (usize), fallback to use this.
    pub(super) preferred_column: Option<(Pixels, usize)>,
    _subscriptions: Vec<Subscription>,

    pub(super) _context_menu_task: Task<Result<()>>,
    pub(super) inline_completion: InlineCompletion,
}

impl EventEmitter<InputEvent> for InputState {}

impl InputState {
    /// Create a Input state with default [`InputMode::SingleLine`] mode.
    ///
    /// See also: [`Self::multi_line`], [`Self::auto_grow`] to set other mode.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle().tab_stop(true);
        let blink_cursor = cx.new(|_| BlinkCursor::new());
        let history = History::new().group_interval(std::time::Duration::from_secs(1));

        let _subscriptions = vec![
            // Observe the blink cursor to repaint the view when it changes.
            cx.observe(&blink_cursor, |_, _, cx| cx.notify()),
            // Blink the cursor when the window is active, pause when it's not.
            cx.observe_window_activation(window, |input, window, cx| {
                if window.is_window_active() {
                    let focus_handle = input.focus_handle.clone();
                    if focus_handle.is_focused(window) {
                        input.blink_cursor.update(cx, |blink_cursor, cx| {
                            blink_cursor.start(cx);
                        });
                    }
                }
            }),
            cx.on_focus(&focus_handle, window, Self::on_focus),
            cx.on_blur(&focus_handle, window, Self::on_blur),
        ];

        let text_style = window.text_style();
        let mouse_context_menu = MouseContextMenu::new(cx.entity(), window, cx);

        Self {
            focus_handle: focus_handle.clone(),
            text: "".into(),
            text_wrapper: TextWrapper::new(text_style.font(), window.rem_size(), None),
            blink_cursor,
            history,
            selected_range: Selection::default(),
            secondary_selected_ranges: Vec::new(),
            search_panel: None,
            searchable: false,
            search_max_matches: usize::MAX,
            selected_word_range: None,
            selection_reversed: false,
            ime_marked_range: None,
            input_bounds: Bounds::default(),
            selecting: false,
            rectangular_anchor: None,
            batch_editing: false,
            disabled: false,
            masked: false,
            clean_on_escape: false,
            soft_wrap: true,
            loading: false,
            pattern: None,
            validate: None,
            mode: InputMode::default(),
            last_layout: None,
            last_bounds: None,
            last_selected_range: None,
            last_cursor: None,
            scroll_handle: ScrollHandle::new(),
            scroll_size: gpui::size(px(0.), px(0.)),
            deferred_scroll_offset: None,
            zoom_anchor: None,
            preferred_column: None,
            placeholder: SharedString::default(),
            mask_pattern: MaskPattern::default(),
            lsp: Lsp::default(),
            diagnostic_popover: None,
            context_menu: None,
            mouse_context_menu,
            completion_inserting: false,
            hover_popover: None,
            hover_definition: HoverDefinition::default(),
            silent_replace_text: false,
            size: Size::default(),
            _subscriptions,
            _context_menu_task: Task::ready(Ok(())),
            _pending_update: false,
            inline_completion: InlineCompletion::default(),
        }
    }

    /// Set Input to use multi line mode.
    ///
    /// Default rows is 2.
    pub fn multi_line(mut self, multi_line: bool) -> Self {
        self.mode = self.mode.multi_line(multi_line);
        self
    }

    /// Set Input to use [`InputMode::AutoGrow`] mode with min, max rows limit.
    pub fn auto_grow(mut self, min_rows: usize, max_rows: usize) -> Self {
        self.mode = InputMode::auto_grow(min_rows, max_rows);
        self
    }

    /// Set Input to use [`InputMode::CodeEditor`] mode.
    ///
    /// Default options:
    ///
    /// - line_number: true
    /// - tab_size: 2
    /// - hard_tabs: false
    /// - height: 100%
    /// - multi_line: true
    /// - indent_guides: true
    ///
    /// If `highlighter` is None, will use the default highlighter.
    ///
    /// Code Editor aim for help used to simple code editing or display, not a full-featured code editor.
    ///
    /// ## Features
    ///
    /// - Syntax Highlighting
    /// - Auto Indent
    /// - Line Number
    /// - Large Text support, up to 50K lines.
    pub fn code_editor(mut self, language: impl Into<SharedString>) -> Self {
        let language: SharedString = language.into();
        self.mode = InputMode::code_editor(language);
        self.searchable = true;
        self
    }

    /// Set this input is searchable, default is false (Default true for Code Editor).
    pub fn searchable(mut self, searchable: bool) -> Self {
        debug_assert!(self.mode.is_multi_line());
        self.searchable = searchable;
        self
    }

    /// Bound the number of retained and decorated search matches.
    pub fn search_max_matches(mut self, max_matches: usize) -> Self {
        self.search_max_matches = max_matches;
        self
    }

    /// Bound the approximate heap memory retained by undo history.
    pub fn undo_max_bytes(mut self, max_bytes: usize) -> Self {
        self.history = self.history.max_bytes(max_bytes);
        self
    }

    /// Update undo and search budgets for a live editor.
    pub fn set_resource_budgets(
        &mut self,
        undo_max_bytes: usize,
        search_max_matches: usize,
        cx: &mut Context<Self>,
    ) {
        self.history.set_max_bytes(undo_max_bytes);
        self.search_max_matches = search_max_matches;
        self.search_panel = None;
        cx.notify();
    }

    /// Set placeholder
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set enable/disable line number, only for [`InputMode::CodeEditor`] mode.
    pub fn line_number(mut self, line_number: bool) -> Self {
        debug_assert!(self.mode.is_code_editor() && self.mode.is_multi_line());
        if let InputMode::CodeEditor { line_number: l, .. } = &mut self.mode {
            *l = line_number;
        }
        self
    }

    /// Set line number, only for [`InputMode::CodeEditor`] mode.
    pub fn set_line_number(&mut self, line_number: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_code_editor() && self.mode.is_multi_line());
        if let InputMode::CodeEditor { line_number: l, .. } = &mut self.mode {
            *l = line_number;
        }
        cx.notify();
    }

    /// Returns whether the code-editor gutter currently shows line numbers.
    pub fn line_numbers_visible(&self) -> bool {
        self.mode.line_number()
    }

    /// Set the number of rows for the multi-line Textarea.
    ///
    /// This is only used when `multi_line` is set to true.
    ///
    /// default: 2
    pub fn rows(mut self, rows: usize) -> Self {
        match &mut self.mode {
            InputMode::PlainText { rows: r, .. } | InputMode::CodeEditor { rows: r, .. } => {
                *r = rows
            }
            InputMode::AutoGrow {
                max_rows: max_r,
                rows: r,
                ..
            } => {
                *r = rows;
                *max_r = rows;
            }
        }
        self
    }

    /// Set highlighter language for for [`InputMode::CodeEditor`] mode.
    pub fn set_highlighter(
        &mut self,
        new_language: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        match &mut self.mode {
            InputMode::CodeEditor {
                language,
                highlighter,
                ..
            } => {
                *language = new_language.into();
                *highlighter.borrow_mut() = None;
                self._pending_update = true;
            }
            _ => {}
        }
        cx.notify();
    }

    /// Return the configured code-editor language, including `text` when highlighting is off.
    pub fn highlighter_language(&self) -> Option<&str> {
        match &self.mode {
            InputMode::CodeEditor { language, .. } => Some(language.as_ref()),
            _ => None,
        }
    }

    fn reset_highlighter(&mut self, cx: &mut Context<Self>) {
        match &mut self.mode {
            InputMode::CodeEditor { highlighter, .. } => {
                *highlighter.borrow_mut() = None;
            }
            _ => {}
        }
        cx.notify();
    }

    #[inline]
    pub fn diagnostics(&self) -> Option<&DiagnosticSet> {
        self.mode.diagnostics()
    }

    #[inline]
    pub fn diagnostics_mut(&mut self) -> Option<&mut DiagnosticSet> {
        self.mode.diagnostics_mut()
    }

    /// Set placeholder
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    /// Find which line and sub-line the given offset belongs to, along with the position within that sub-line.
    ///
    /// Returns:
    ///
    /// - The index of the line (zero-based) containing the offset.
    /// - The index of the sub-line (zero-based) within the line containing the offset.
    /// - The position of the offset.
    #[allow(unused)]
    pub(super) fn line_and_position_for_offset(
        &self,
        offset: usize,
    ) -> (usize, usize, Option<Point<Pixels>>) {
        let Some(last_layout) = &self.last_layout else {
            return (0, 0, None);
        };
        let line_height = last_layout.line_height;

        let mut prev_lines_offset = last_layout.visible_range_offset.start;
        let mut y_offset = last_layout.visible_top;
        for (line_index, line) in last_layout.lines.iter().enumerate() {
            let local_offset = offset.saturating_sub(prev_lines_offset);
            if let Some(pos) = line.position_for_index(local_offset, line_height) {
                let sub_line_index = (pos.y / line_height) as usize;
                let adjusted_pos = point(pos.x + last_layout.line_number_width, pos.y + y_offset);
                return (line_index, sub_line_index, Some(adjusted_pos));
            }

            y_offset += line.size(line_height).height;
            prev_lines_offset += line.len() + 1;
        }
        (0, 0, None)
    }

    /// Set the text of the input field.
    ///
    /// And the selection_range will be reset to 0..0.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.history.ignore = true;
        let was_disabled = self.disabled;
        self.disabled = false;
        self.replace_text(value, window, cx);
        self.disabled = was_disabled;
        self.history.ignore = false;

        // Ensure cursor to start when set text
        if self.mode.is_single_line() {
            self.selected_range = (self.text.len()..self.text.len()).into();
        } else {
            self.selected_range.clear();
        }
        self.secondary_selected_ranges.clear();

        if self.mode.is_code_editor() {
            self._pending_update = true;
            self.lsp.reset();
        }

        // Move scroll to top
        self.scroll_handle.set_offset(point(px(0.), px(0.)));

        cx.notify();
    }

    /// Insert text at the current cursor position.
    ///
    /// And the cursor will be moved to the end of inserted text.
    pub fn insert(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        let range_utf16 = self.range_to_utf16(&(self.cursor()..self.cursor()));
        self.replace_text_in_range_silent(Some(range_utf16), &text, window, cx);
        self.selected_range = (self.selected_range.end..self.selected_range.end).into();
    }

    /// Return the primary and secondary selections in document order.
    pub fn selections(&self) -> Vec<Selection> {
        normalized_selections(self.selected_range, &self.secondary_selected_ranges)
            .into_iter()
            .map(|selection| selection.range)
            .collect()
    }

    /// Replace the current selection set. The last supplied range is the primary selection.
    pub fn set_selections(
        &mut self,
        ranges: Vec<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut ranges = ranges
            .into_iter()
            .map(|range| {
                Selection::new(
                    self.text
                        .clip_offset(range.start.min(self.text.len()), Bias::Left),
                    self.text
                        .clip_offset(range.end.min(self.text.len()), Bias::Left),
                )
            })
            .collect::<Vec<_>>();
        let primary = ranges.pop().unwrap_or(self.selected_range);
        self.apply_selection_set(normalized_selections(primary, &ranges));
        self.ime_marked_range = None;
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    fn apply_selection_set(&mut self, selections: Vec<TaggedSelection>) {
        self.secondary_selected_ranges.clear();
        for selection in selections {
            if selection.primary {
                self.selected_range = selection.range;
            } else {
                self.secondary_selected_ranges.push(selection.range);
            }
        }
    }

    fn add_caret_at(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self
            .text
            .clip_offset(offset.min(self.text.len()), Bias::Left);
        let mut secondary = std::mem::take(&mut self.secondary_selected_ranges);
        secondary.push(self.selected_range);
        self.apply_selection_set(normalized_selections(
            Selection::new(offset, offset),
            &secondary,
        ));
        self.selection_reversed = false;
        self.rectangular_anchor = None;
        self.pause_blink_cursor(cx);
        cx.notify();
    }

    fn replace_selection_set(
        &mut self,
        replacement_values: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selections =
            normalized_selections(self.selected_range, &self.secondary_selected_ranges);
        if selections.is_empty() {
            return;
        }
        let plan = replacement_plan(&selections, &replacement_values);

        self.history.start_new_group();
        self.batch_editing = true;
        let mut resulting = Vec::with_capacity(plan.len());
        for replacement in plan {
            let range_utf16 = self.range_to_utf16(&replacement.range);
            self.replace_text_in_range_silent(Some(range_utf16), &replacement.value, window, cx);
            resulting.push(replacement.resulting_selection);
        }
        self.batch_editing = false;
        self.history.end_grouping();
        self.apply_selection_set(resulting);
        self.selection_reversed = false;
        self.update_preferred_column();
        self.scroll_to(self.cursor(), None, cx);
        cx.notify();
    }

    /// Replace text at the current cursor position.
    ///
    /// And the cursor will be moved to the end of replaced text.
    pub fn replace(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        self.replace_text_in_range_silent(None, &text, window, cx);
        self.selected_range = (self.selected_range.end..self.selected_range.end).into();
    }

    fn replace_text(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text: SharedString = text.into();
        let range = 0..self.text.chars().map(|c| c.len_utf16()).sum();
        self.replace_text_in_range_silent(Some(range), &text, window, cx);
        self.reset_highlighter(cx);
    }

    /// Set with disabled mode.
    ///
    /// See also: [`Self::set_disabled`], [`Self::is_disabled`].
    #[allow(unused)]
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set with password masked state.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn masked(mut self, masked: bool) -> Self {
        debug_assert!(self.mode.is_single_line());
        self.masked = masked;
        self
    }

    /// Set the password masked state of the input field.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn set_masked(&mut self, masked: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_single_line());
        self.masked = masked;
        cx.notify();
    }

    /// Set true to clear the input by pressing Escape key.
    pub fn clean_on_escape(mut self) -> Self {
        self.clean_on_escape = true;
        self
    }

    /// Set the soft wrap mode for multi-line input, default is true.
    pub fn soft_wrap(mut self, wrap: bool) -> Self {
        debug_assert!(self.mode.is_multi_line());
        self.soft_wrap = wrap;
        self
    }

    /// Update the soft wrap mode for multi-line input, default is true.
    pub fn set_soft_wrap(&mut self, wrap: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_multi_line());
        self.soft_wrap = wrap;
        if wrap {
            let wrap_width = self
                .last_layout
                .as_ref()
                .map(|layout| {
                    self.input_bounds.size.width - layout.line_number_width - RIGHT_MARGIN
                })
                .unwrap_or(self.input_bounds.size.width - RIGHT_MARGIN)
                .max(px(0.));

            self.text_wrapper.set_wrap_width(Some(wrap_width), cx);

            // Reset scroll to left 0
            let mut offset = self.scroll_handle.offset();
            offset.x = px(0.);
            self.scroll_handle.set_offset(offset);
        } else {
            self.text_wrapper.set_wrap_width(None, cx);
        }
        cx.notify();
    }

    /// Set the regular expression pattern of the input field.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn pattern(mut self, pattern: regex::Regex) -> Self {
        debug_assert!(self.mode.is_single_line());
        self.pattern = Some(pattern);
        self
    }

    /// Set the regular expression pattern of the input field with reference.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn set_pattern(
        &mut self,
        pattern: regex::Regex,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        debug_assert!(self.mode.is_single_line());
        self.pattern = Some(pattern);
    }

    /// Set the validation function of the input field.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn validate(mut self, f: impl Fn(&str, &mut Context<Self>) -> bool + 'static) -> Self {
        debug_assert!(self.mode.is_single_line());
        self.validate = Some(Box::new(f));
        self
    }

    /// Set true to show spinner at the input right.
    ///
    /// Only for [`InputMode::SingleLine`] mode.
    pub fn set_loading(&mut self, loading: bool, _: &mut Window, cx: &mut Context<Self>) {
        debug_assert!(self.mode.is_single_line());
        self.loading = loading;
        cx.notify();
    }

    /// Set the default value of the input field.
    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        let text: SharedString = value.into();
        self.text = Rope::from(text.as_str());
        if let Some(diagnostics) = self.mode.diagnostics_mut() {
            diagnostics.reset(&self.text)
        }
        self.text_wrapper.set_default_text(&self.text);
        self._pending_update = true;
        self
    }

    /// Return the value of the input field.
    pub fn value(&self) -> SharedString {
        SharedString::new(self.text.to_string())
    }

    /// Return the value without mask.
    pub fn unmask_value(&self) -> SharedString {
        self.mask_pattern.unmask(&self.text.to_string()).into()
    }

    /// Return the text [`Rope`] of the input field.
    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// Return the (0-based) [`Position`] of the cursor.
    pub fn cursor_position(&self) -> Position {
        let offset = self.cursor();
        self.text.offset_to_position(offset)
    }

    /// Set (0-based) [`Position`] of the cursor.
    ///
    /// This will move the cursor to the specified line and column, and update the selection range.
    pub fn set_cursor_position(
        &mut self,
        position: impl Into<Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position: Position = position.into();
        let offset = self.text.position_to_offset(&position);

        self.move_to(offset, None, cx);
        self.update_preferred_column();
        self.focus(window, cx);
    }

    /// Focus the input field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window);
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
        });
    }

    pub(super) fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor()), cx);
    }

    pub(super) fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }

    pub(super) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.select_vertical(-1, cx);
    }

    pub(super) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.select_vertical(1, cx);
    }

    pub(super) fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = (0..self.text.len()).into();
        self.secondary_selected_ranges.clear();
        cx.notify();
    }

    pub(super) fn select_to_start(
        &mut self,
        _: &SelectToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, cx);
    }

    pub(super) fn select_to_end(
        &mut self,
        _: &SelectToEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = self.text.len();
        self.select_to(end, cx);
    }

    pub(super) fn select_to_start_of_line(
        &mut self,
        _: &SelectToStartOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.start_of_line();
        self.select_to(offset, cx);
    }

    pub(super) fn select_to_end_of_line(
        &mut self,
        _: &SelectToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.end_of_line();
        self.select_to(offset, cx);
    }

    pub(super) fn select_to_previous_word(
        &mut self,
        _: &SelectToPreviousWordStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.previous_start_of_word();
        self.select_to(offset, cx);
    }

    pub(super) fn select_to_next_word(
        &mut self,
        _: &SelectToNextWordEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.next_end_of_word();
        self.select_to(offset, cx);
    }

    /// Return the start offset of the previous word.
    pub(super) fn previous_start_of_word(&mut self) -> usize {
        let offset = self.selected_range.start;
        let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
        // FIXME: Avoid to_string
        let left_part = self.text.slice(0..offset).to_string();

        UnicodeSegmentation::split_word_bound_indices(left_part.as_str())
            .rfind(|(_, s)| !s.trim_start().is_empty())
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Return the next end offset of the next word.
    pub(super) fn next_end_of_word(&mut self) -> usize {
        let offset = self.cursor();
        let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
        let right_part = self.text.slice(offset..self.text.len()).to_string();

        UnicodeSegmentation::split_word_bound_indices(right_part.as_str())
            .find(|(_, s)| !s.trim_start().is_empty())
            .map(|(i, s)| offset + i + s.len())
            .unwrap_or(self.text.len())
    }

    /// Get start of line byte offset of cursor
    pub(super) fn start_of_line(&self) -> usize {
        if self.mode.is_single_line() {
            return 0;
        }

        let row = self.text.offset_to_point(self.cursor()).row;
        self.text.line_start_offset(row)
    }

    /// Get end of line byte offset of cursor
    pub(super) fn end_of_line(&self) -> usize {
        if self.mode.is_single_line() {
            return self.text.len();
        }

        let row = self.text.offset_to_point(self.cursor()).row;
        self.text.line_end_offset(row)
    }

    /// Get start line of selection start or end (The min value).
    ///
    /// This is means is always get the first line of selection.
    pub(super) fn start_of_line_of_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        if self.mode.is_single_line() {
            return 0;
        }

        let mut offset =
            self.previous_boundary(self.selected_range.start.min(self.selected_range.end));
        if self.text.char_at(offset) == Some('\r') {
            offset += 1;
        }

        let line = self
            .text_for_range(self.range_to_utf16(&(0..offset + 1)), &mut None, window, cx)
            .unwrap_or_default()
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        line
    }

    /// Get indent string of next line.
    ///
    /// To get current and next line indent, to return more depth one.
    pub(super) fn indent_of_next_line(&mut self) -> String {
        if self.mode.is_single_line() {
            return "".into();
        }

        let mut current_indent = String::new();
        let mut next_indent = String::new();
        let current_line_start_pos = self.start_of_line();
        let next_line_start_pos = self.end_of_line();
        for c in self.text.slice(current_line_start_pos..).chars() {
            if !c.is_whitespace() {
                break;
            }
            if c == '\n' || c == '\r' {
                break;
            }
            current_indent.push(c);
        }

        for c in self.text.slice(next_line_start_pos..).chars() {
            if !c.is_whitespace() {
                break;
            }
            if c == '\n' || c == '\r' {
                break;
            }
            next_indent.push(c);
        }

        if next_indent.len() > current_indent.len() {
            return next_indent;
        } else {
            return current_indent;
        }
    }

    pub(super) fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        let secondary = std::mem::take(&mut self.secondary_selected_ranges);
        self.secondary_selected_ranges = secondary
            .into_iter()
            .map(|selection| {
                if selection.is_empty() {
                    Selection::new(self.previous_boundary(selection.start), selection.start)
                } else {
                    selection
                }
            })
            .collect();
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor()), cx)
        }
        self.replace_text_in_range(None, "", window, cx);
        self.pause_blink_cursor(cx);
    }

    pub(super) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        let secondary = std::mem::take(&mut self.secondary_selected_ranges);
        self.secondary_selected_ranges = secondary
            .into_iter()
            .map(|selection| {
                if selection.is_empty() {
                    Selection::new(selection.start, self.next_boundary(selection.start))
                } else {
                    selection
                }
            })
            .collect();
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor()), cx)
        }
        self.replace_text_in_range(None, "", window, cx);
        self.pause_blink_cursor(cx);
    }

    pub(super) fn delete_to_beginning_of_line(
        &mut self,
        _: &DeleteToBeginningOfLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_range.is_empty() {
            self.replace_text_in_range(None, "", window, cx);
            self.pause_blink_cursor(cx);
            return;
        }

        let mut offset = self.start_of_line();
        if offset == self.cursor() {
            offset = offset.saturating_sub(1);
        }
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(offset..self.cursor()))),
            "",
            window,
            cx,
        );
        self.pause_blink_cursor(cx);
    }

    pub(super) fn delete_to_end_of_line(
        &mut self,
        _: &DeleteToEndOfLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_range.is_empty() {
            self.replace_text_in_range(None, "", window, cx);
            self.pause_blink_cursor(cx);
            return;
        }

        let mut offset = self.end_of_line();
        if offset == self.cursor() {
            offset = (offset + 1).clamp(0, self.text.len());
        }
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(self.cursor()..offset))),
            "",
            window,
            cx,
        );
        self.pause_blink_cursor(cx);
    }

    pub(super) fn delete_previous_word(
        &mut self,
        _: &DeleteToPreviousWordStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_range.is_empty() {
            self.replace_text_in_range(None, "", window, cx);
            self.pause_blink_cursor(cx);
            return;
        }

        let offset = self.previous_start_of_word();
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(offset..self.cursor()))),
            "",
            window,
            cx,
        );
        self.pause_blink_cursor(cx);
    }

    pub(super) fn delete_next_word(
        &mut self,
        _: &DeleteToNextWordEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_range.is_empty() {
            self.replace_text_in_range(None, "", window, cx);
            self.pause_blink_cursor(cx);
            return;
        }

        let offset = self.next_end_of_word();
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(self.cursor()..offset))),
            "",
            window,
            cx,
        );
        self.pause_blink_cursor(cx);
    }

    pub(super) fn enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_action_for_context_menu(Box::new(action.clone()), window, cx) {
            return;
        }

        // Clear inline completion on enter (user chose not to accept it)
        if self.has_inline_completion() {
            self.clear_inline_completion(cx);
        }

        if self.mode.is_multi_line() {
            // Get current line indent
            let indent = if self.mode.is_code_editor() {
                self.indent_of_next_line()
            } else {
                "".to_string()
            };

            // Add newline and indent
            let new_line_text = format!("\n{}", indent);
            self.replace_text_in_range_silent(None, &new_line_text, window, cx);
            self.pause_blink_cursor(cx);
        } else {
            // Single line input, just emit the event (e.g.: In a dialog to confirm).
            cx.propagate();
        }

        cx.emit(InputEvent::PressEnter {
            secondary: action.secondary,
        });
    }

    pub(super) fn clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text("", window, cx);
        self.selected_range = (0..0).into();
        self.secondary_selected_ranges.clear();
        self.scroll_to(0, None, cx);
    }

    pub(super) fn escape(&mut self, action: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_action_for_context_menu(Box::new(action.clone()), window, cx) {
            return;
        }

        // Clear inline completion on escape
        if self.has_inline_completion() {
            self.clear_inline_completion(cx);
            return; // Consume the escape, don't propagate
        }

        if self.ime_marked_range.is_some() {
            self.unmark_text(window, cx);
        }

        if self.clean_on_escape {
            return self.clean(window, cx);
        }

        cx.propagate();
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Clear inline completion on any mouse interaction
        self.clear_inline_completion(cx);

        // If there have IME marked range and is empty (Means pressed Esc to abort IME typing)
        // Clear the marked range.
        if let Some(ime_marked_range) = &self.ime_marked_range {
            if ime_marked_range.len() == 0 {
                self.ime_marked_range = None;
            }
        }

        self.selecting = true;
        let offset = self.index_for_mouse_position(event.position);

        if self.handle_click_hover_definition(event, offset, window, cx) {
            return;
        }

        if event.button == MouseButton::Left {
            // Triple click selects the complete logical line, including its line ending.
            if event.click_count == 3 && self.mode.is_multi_line() {
                self.select_line(offset, window, cx);
                return;
            }

            // Double click selects a word.
            if event.click_count == 2 {
                self.select_word(offset, window, cx);
                return;
            }
        }

        // Show Mouse context menu
        if event.button == MouseButton::Right {
            self.handle_right_click_menu(event, offset, window, cx);
            return;
        }

        if event.button == MouseButton::Left && self.mode.is_multi_line() {
            if event.modifiers.platform {
                self.selecting = false;
                self.add_caret_at(offset, cx);
                return;
            }
            if event.modifiers.alt {
                self.secondary_selected_ranges.clear();
                self.selected_range = Selection::new(offset, offset);
                self.selection_reversed = false;
                self.rectangular_anchor = Some(offset);
                self.pause_blink_cursor(cx);
                cx.notify();
                return;
            }
        }

        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, None, cx)
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.selection_reversed = false;
        }
        self.selecting = false;
        self.rectangular_anchor = None;
        self.selected_word_range = None;
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Show diagnostic popover on mouse move
        let offset = self.index_for_mouse_position(event.position);
        self.handle_mouse_move(offset, event, window, cx);

        if self.mode.is_code_editor() {
            if let Some(diagnostic) = self
                .mode
                .diagnostics()
                .and_then(|set| set.for_offset(offset))
            {
                if let Some(diagnostic_popover) = self.diagnostic_popover.as_ref() {
                    if diagnostic_popover.read(cx).diagnostic.range == diagnostic.range {
                        diagnostic_popover.update(cx, |this, cx| {
                            this.show(cx);
                        });

                        return;
                    }
                }

                self.diagnostic_popover = Some(DiagnosticPopover::new(diagnostic, cx.entity(), cx));
                cx.notify();
            } else {
                if let Some(diagnostic_popover) = self.diagnostic_popover.as_mut() {
                    diagnostic_popover.update(cx, |this, cx| {
                        this.check_to_hide(event.position, cx);
                    })
                }
            }
        }
    }

    pub(super) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Textify reserves Command-scroll for per-document text zoom. Let the containing
        // application handle that gesture instead of consuming it as editor scrolling.
        if event.modifiers.secondary() {
            return;
        }
        let line_height = self
            .last_layout
            .as_ref()
            .map(|layout| layout.line_height)
            .unwrap_or(window.line_height());
        let delta = event.delta.pixel_delta(line_height);

        let old_offset = self.scroll_handle.offset();
        self.update_scroll_offset(Some(old_offset + delta), cx);

        // Only stop propagation if the offset actually changed
        if self.scroll_handle.offset() != old_offset {
            cx.stop_propagation();
        }

        self.diagnostic_popover = None;
    }

    pub(super) fn update_scroll_offset(
        &mut self,
        offset: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let mut offset = offset.unwrap_or(self.scroll_handle.offset());

        let safe_y_range =
            (-self.scroll_size.height + self.input_bounds.size.height).min(px(0.0))..px(0.);
        let safe_x_range =
            (-self.scroll_size.width + self.input_bounds.size.width).min(px(0.0))..px(0.);

        offset.y = if self.mode.is_single_line() {
            px(0.)
        } else {
            offset.y.clamp(safe_y_range.start, safe_y_range.end)
        };
        offset.x = offset.x.clamp(safe_x_range.start, safe_x_range.end);
        self.scroll_handle.set_offset(offset);
        cx.notify();
    }

    fn preserve_zoom_anchor_for_offset(&mut self, offset: usize) -> bool {
        let Some(layout) = self.last_layout.as_ref() else {
            return false;
        };
        if self.text_wrapper.lines.is_empty() {
            return false;
        }

        let offset = offset.min(self.text.len());
        let display_point = self.text_wrapper.offset_to_display_point(offset);
        let viewport_y =
            display_point.row as f32 * layout.line_height + self.scroll_handle.offset().y;
        let visible = viewport_y + layout.line_height >= px(0.)
            && viewport_y <= self.input_bounds.size.height;
        if visible {
            self.zoom_anchor = Some(ZoomAnchor { offset, viewport_y });
        }
        visible
    }

    /// Preserve the document row under the pointer through the next text layout.
    ///
    /// Returns false when the pointer is outside the input or layout has not completed yet.
    pub fn preserve_zoom_anchor_at(&mut self, position: Point<Pixels>) -> bool {
        if !self.input_bounds.contains(&position) {
            return false;
        }
        let offset = self.index_for_mouse_position(position);
        self.preserve_zoom_anchor_for_offset(offset)
    }

    /// Preserve the visible caret row through the next text layout.
    ///
    /// Returns false when the caret is outside the viewport or layout has not completed yet.
    pub fn preserve_cursor_anchor(&mut self) -> bool {
        self.preserve_zoom_anchor_for_offset(self.cursor())
    }

    /// Move the scroll offset so the pending zoom anchor's row stays at its
    /// captured viewport position under `line_height`.
    ///
    /// This must run before a frame's visible line range is computed: the range
    /// is derived from the scroll offset, so correcting the offset any later
    /// leaves that frame's shaped lines short of the anchored viewport and the
    /// uncovered region paints as a background-colored flash.
    pub(super) fn apply_zoom_anchor(&mut self, line_height: Pixels) {
        let Some(anchor) = self.zoom_anchor else {
            return;
        };
        let display_point = self.text_wrapper.offset_to_display_point(anchor.offset);
        let document_y = display_point.row as f32 * line_height;
        let mut offset = self.scroll_handle.offset();
        offset.y = (anchor.viewport_y - document_y).min(px(0.));
        self.scroll_handle.set_offset(offset);
    }

    /// The current scroll offset of the editor viewport.
    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_handle.offset()
    }

    /// Scroll to make the given offset visible.
    ///
    /// If `direction` is Some, will keep edges at the same side.
    pub(crate) fn scroll_to(
        &mut self,
        offset: usize,
        direction: Option<MoveDirection>,
        cx: &mut Context<Self>,
    ) {
        let Some(last_layout) = self.last_layout.as_ref() else {
            return;
        };
        let Some(bounds) = self.last_bounds.as_ref() else {
            return;
        };

        let mut scroll_offset = self.scroll_handle.offset();
        let was_offset = scroll_offset;
        let line_height = last_layout.line_height;

        let point = self.text.offset_to_point(offset);

        let row = point.row;
        let display_point = self.text_wrapper.offset_to_display_point(offset);
        let row_offset_y = display_point.row as f32 * line_height;

        if let Some(line) = last_layout
            .lines
            .get(row.saturating_sub(last_layout.visible_range.start))
        {
            // Check to scroll horizontally and soft wrap lines
            if let Some(pos) = line.position_for_index(point.column, line_height) {
                let bounds_width = bounds.size.width - last_layout.line_number_width;
                let col_offset_x = pos.x;
                if col_offset_x - RIGHT_MARGIN < -scroll_offset.x {
                    // If the position is out of the visible area, scroll to make it visible
                    scroll_offset.x = -col_offset_x + RIGHT_MARGIN;
                } else if col_offset_x + RIGHT_MARGIN > -scroll_offset.x + bounds_width {
                    scroll_offset.x = -(col_offset_x - bounds_width + RIGHT_MARGIN);
                }
            }
        }

        // Check if row_offset_y is out of the viewport
        // If row offset is not in the viewport, scroll to make it visible
        let edge_height = if direction.is_some() && self.mode.is_code_editor() {
            3 * line_height
        } else {
            line_height
        };
        if row_offset_y - edge_height + line_height < -scroll_offset.y {
            // Scroll up
            scroll_offset.y = -row_offset_y + edge_height - line_height;
        } else if row_offset_y + edge_height > -scroll_offset.y + bounds.size.height {
            // Scroll down
            scroll_offset.y = -(row_offset_y - bounds.size.height + edge_height);
        }

        // Avoid necessary scroll, when it was already in the correct position.
        if direction == Some(MoveDirection::Up) {
            scroll_offset.y = scroll_offset.y.max(was_offset.y);
        } else if direction == Some(MoveDirection::Down) {
            scroll_offset.y = scroll_offset.y.min(was_offset.y);
        }

        scroll_offset.x = scroll_offset.x.min(px(0.));
        scroll_offset.y = scroll_offset.y.min(px(0.));
        self.deferred_scroll_offset = Some(scroll_offset);
        cx.notify();
    }

    pub(super) fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let selected = self
            .selections()
            .into_iter()
            .filter(|selection| !selection.is_empty())
            .map(|selection| self.text.slice(selection).to_string())
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return;
        }

        cx.write_to_clipboard(ClipboardItem::new_string(selected.join("\n")));
    }

    pub(super) fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.selections().iter().all(Selection::is_empty) && self.mode.is_multi_line() {
            let line_selections =
                normalized_selections(self.selected_range, &self.secondary_selected_ranges)
                    .into_iter()
                    .map(|selection| {
                        let row = self.text.offset_to_point(selection.range.start).row;
                        let start = self.text.line_start_offset(row);
                        let end = if row + 1 < self.text.lines_len() {
                            self.text.line_start_offset(row + 1)
                        } else {
                            self.text.len()
                        };
                        TaggedSelection {
                            range: Selection::new(start, end),
                            primary: selection.primary,
                        }
                    })
                    .collect::<Vec<_>>();
            let primary = line_selections
                .iter()
                .find(|selection| selection.primary)
                .map(|selection| selection.range)
                .unwrap_or_default();
            let secondary = line_selections
                .iter()
                .filter(|selection| !selection.primary)
                .map(|selection| selection.range)
                .collect::<Vec<_>>();
            self.apply_selection_set(normalized_selections(primary, &secondary));
        }

        if self.selections().iter().all(Selection::is_empty) {
            return;
        }

        self.copy(&Copy, window, cx);
        self.replace_selection_set(vec![String::new()], window, cx);
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(clipboard) = cx.read_from_clipboard() {
            let mut new_text = clipboard.text().unwrap_or_default();
            if !self.mode.is_multi_line() {
                new_text = new_text.replace('\n', "");
            }

            let selection_count = self.selections().len();
            let distributed = new_text
                .split('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
                .collect::<Vec<_>>();
            if selection_count > 1 && distributed.len() == selection_count {
                self.replace_selection_set(distributed, window, cx);
            } else {
                self.replace_text_in_range_silent(None, &new_text, window, cx);
            }
            self.scroll_to(self.cursor(), None, cx);
        }
    }

    fn push_history(&mut self, text: &Rope, range: &Range<usize>, new_text: &str) {
        if self.history.ignore {
            return;
        }

        let old_text = text.slice(range.clone()).to_string();
        let new_range = range.start..range.start + new_text.len();

        self.history
            .push(Change::new(range.clone(), &old_text, new_range, new_text));
    }

    pub(super) fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.secondary_selected_ranges.clear();
        self.history.ignore = true;
        if let Some(changes) = self.history.undo() {
            for change in changes {
                let range_utf16 = self.range_to_utf16(&change.new_range.into());
                self.replace_text_in_range_silent(Some(range_utf16), &change.old_text, window, cx);
            }
        }
        self.history.ignore = false;
    }

    pub(super) fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.secondary_selected_ranges.clear();
        self.history.ignore = true;
        if let Some(changes) = self.history.redo() {
            for change in changes {
                let range_utf16 = self.range_to_utf16(&change.old_range.into());
                self.replace_text_in_range_silent(Some(range_utf16), &change.new_text, window, cx);
            }
        }
        self.history.ignore = false;
    }

    /// Get byte offset of the cursor.
    ///
    /// The offset is the UTF-8 offset.
    pub fn cursor(&self) -> usize {
        if let Some(ime_marked_range) = &self.ime_marked_range {
            return ime_marked_range.end;
        }

        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub(crate) fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        // If the text is empty, always return 0
        if self.text.len() == 0 {
            return 0;
        }

        let (Some(bounds), Some(last_layout)) =
            (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };

        let line_height = last_layout.line_height;
        let line_number_width = last_layout.line_number_width;

        // TIP: About the IBeam cursor
        //
        // If cursor style is IBeam, the mouse mouse position is in the middle of the cursor (This is special in OS)

        // The position is relative to the bounds of the text input
        //
        // bounds.origin:
        //
        // - included the input padding.
        // - included the scroll offset.
        let inner_position = position - bounds.origin - point(line_number_width, px(0.));

        let mut index = last_layout.visible_range_offset.start;
        let mut y_offset = last_layout.visible_top;
        for (ix, line) in self
            .text_wrapper
            .lines
            .iter()
            .skip(last_layout.visible_range.start)
            .enumerate()
        {
            let line_origin = self.line_origin_with_y_offset(&mut y_offset, line, line_height);
            let pos = inner_position - line_origin;

            let Some(line_layout) = last_layout.lines.get(ix) else {
                if pos.y < line_origin.y + line_height {
                    break;
                }

                continue;
            };

            // Return offset by use closest_index_for_x if is single line mode.
            if self.mode.is_single_line() {
                index = line_layout.closest_index_for_x(pos.x);
                break;
            }

            if let Some(v) = line_layout.closest_index_for_position(pos, line_height) {
                index += v;
                break;
            } else if pos.y < px(0.) {
                break;
            }

            // +1 for `\n`
            index += line_layout.len() + 1;
        }

        let index = if index > self.text.len() {
            self.text.len()
        } else {
            index
        };

        if self.masked {
            // When is masked, the index is char index, need convert to byte index.
            self.text.char_index_to_offset(index)
        } else {
            index
        }
    }

    /// Returns a y offsetted point for the line origin.
    fn line_origin_with_y_offset(
        &self,
        y_offset: &mut Pixels,
        line: &LineItem,
        line_height: Pixels,
    ) -> Point<Pixels> {
        // NOTE: About line.wrap_boundaries.len()
        //
        // If only 1 line, the value is 0
        // If have 2 line, the value is 1
        if self.mode.is_multi_line() {
            let p = point(px(0.), *y_offset);
            *y_offset += line.height(line_height);
            p
        } else {
            point(px(0.), px(0.))
        }
    }

    /// Select the text from the current cursor position to the given offset.
    ///
    /// The offset is the UTF-8 offset.
    ///
    /// Ensure the offset use self.next_boundary or self.previous_boundary to get the correct offset.
    pub(crate) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.clear_inline_completion(cx);

        let offset = offset.clamp(0, self.text.len());
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };

        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = (self.selected_range.end..self.selected_range.start).into();
        }

        // Ensure keep word selected range
        if let Some(word_range) = self.selected_word_range.as_ref() {
            if self.selected_range.start > word_range.start {
                self.selected_range.start = word_range.start;
            }
            if self.selected_range.end < word_range.end {
                self.selected_range.end = word_range.end;
            }
        }
        if self.selected_range.is_empty() {
            self.update_preferred_column();
        }
        cx.notify()
    }

    /// Unselects the currently selected text.
    pub fn unselect(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.cursor();
        self.selected_range = (offset..offset).into();
        self.secondary_selected_ranges.clear();
        cx.notify()
    }

    #[inline]
    pub(super) fn offset_from_utf16(&self, offset: usize) -> usize {
        self.text.offset_utf16_to_offset(offset)
    }

    #[inline]
    pub(super) fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text.offset_to_offset_utf16(offset)
    }

    #[inline]
    pub(super) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    #[inline]
    pub(super) fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    pub(super) fn previous_boundary(&self, offset: usize) -> usize {
        let mut offset = self.text.clip_offset(offset.saturating_sub(1), Bias::Left);
        if let Some(ch) = self.text.char_at(offset) {
            if ch == '\r' {
                offset -= 1;
            }
        }

        offset
    }

    pub(super) fn next_boundary(&self, offset: usize) -> usize {
        let mut offset = self.text.clip_offset(offset + 1, Bias::Right);
        if let Some(ch) = self.text.char_at(offset) {
            if ch == '\r' {
                offset += 1;
            }
        }

        offset
    }

    /// Returns the true to let InputElement to render cursor, when Input is focused and current BlinkCursor is visible.
    pub(crate) fn show_cursor(&self, window: &Window, cx: &App) -> bool {
        (self.focus_handle.is_focused(window) || self.is_context_menu_open(cx))
            && self.blink_cursor.read(cx).visible()
            && window.is_window_active()
    }

    fn on_focus(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
        });
        cx.emit(InputEvent::Focus);
    }

    fn on_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_context_menu_open(cx) {
            return;
        }

        // NOTE: Do not cancel select, when blur.
        // Because maybe user want to copy the selected text by AppMenuBar (will take focus handle).

        self.hover_popover = None;
        self.diagnostic_popover = None;
        self.context_menu = None;
        self.clear_inline_completion(cx);
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.stop(cx);
        });
        Root::update(window, cx, |root, _, _| {
            root.focused_input = None;
        });
        cx.emit(InputEvent::Blur);
        cx.notify();
    }

    pub(super) fn pause_blink_cursor(&mut self, cx: &mut Context<Self>) {
        self.blink_cursor.update(cx, |cursor, cx| {
            cursor.pause(cx);
        });
    }

    pub(super) fn on_key_down(&mut self, _: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pause_blink_cursor(cx);
    }

    pub(super) fn on_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.text.len() == 0 {
            return;
        }

        if self.last_layout.is_none() {
            return;
        }

        if !self.focus_handle.is_focused(window) {
            return;
        }

        if !self.selecting {
            return;
        }

        let offset = self.index_for_mouse_position(event.position);
        if let Some(anchor) = self.rectangular_anchor {
            let (ranges, primary) = rectangular_selections(&self.text, anchor, offset);
            self.apply_selection_set(
                ranges
                    .into_iter()
                    .enumerate()
                    .map(|(index, range)| TaggedSelection {
                        range,
                        primary: index == primary,
                    })
                    .collect(),
            );
            self.selection_reversed = false;
            self.pause_blink_cursor(cx);
            cx.notify();
            return;
        }
        self.select_to(offset, cx);
    }

    fn is_valid_input(&self, new_text: &str, cx: &mut Context<Self>) -> bool {
        if new_text.is_empty() {
            return true;
        }

        if let Some(validate) = &self.validate {
            if !validate(new_text, cx) {
                return false;
            }
        }

        if !self.mask_pattern.is_valid(new_text) {
            return false;
        }

        let Some(pattern) = &self.pattern else {
            return true;
        };

        pattern.is_match(new_text)
    }

    /// Set the mask pattern for formatting the input text.
    ///
    /// The pattern can contain:
    /// - 9: Any digit or dot
    /// - A: Any letter
    /// - *: Any character
    /// - Other characters will be treated as literal mask characters
    ///
    /// Example: "(999)999-999" for phone numbers
    pub fn mask_pattern(mut self, pattern: impl Into<MaskPattern>) -> Self {
        self.mask_pattern = pattern.into();
        if let Some(placeholder) = self.mask_pattern.placeholder() {
            self.placeholder = placeholder.into();
        }
        self
    }

    pub fn set_mask_pattern(
        &mut self,
        pattern: impl Into<MaskPattern>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mask_pattern = pattern.into();
        if let Some(placeholder) = self.mask_pattern.placeholder() {
            self.placeholder = placeholder.into();
        }
        cx.notify();
    }

    pub(super) fn set_input_bounds(&mut self, new_bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let wrap_width_changed = self.input_bounds.size.width != new_bounds.size.width;
        self.input_bounds = new_bounds;

        // Update text_wrapper wrap_width if changed.
        if let Some(last_layout) = self.last_layout.as_ref() {
            if wrap_width_changed {
                let wrap_width = if !self.soft_wrap {
                    // None to disable wrapping (will use Pixels::MAX)
                    None
                } else {
                    last_layout.wrap_width
                };

                self.text_wrapper.set_wrap_width(wrap_width, cx);
                self.mode.update_auto_grow(&self.text_wrapper);
                cx.notify();
            }
        }
    }

    pub(super) fn selected_text(&self) -> RopeSlice<'_> {
        let range_utf16 = self.range_to_utf16(&self.selected_range.into());
        let range = self.range_from_utf16(&range_utf16);
        self.text.slice(range)
    }

    pub(crate) fn range_to_bounds(&self, range: &Range<usize>) -> Option<Bounds<Pixels>> {
        let Some(last_layout) = self.last_layout.as_ref() else {
            return None;
        };

        let Some(last_bounds) = self.last_bounds else {
            return None;
        };

        let (_, _, start_pos) = self.line_and_position_for_offset(range.start);
        let (_, _, end_pos) = self.line_and_position_for_offset(range.end);

        let Some(start_pos) = start_pos else {
            return None;
        };
        let Some(end_pos) = end_pos else {
            return None;
        };

        Some(Bounds::from_corners(
            last_bounds.origin + start_pos,
            last_bounds.origin + end_pos + point(px(0.), last_layout.line_height),
        ))
    }

    /// Replace text by [`lsp_types::Range`].
    ///
    /// See also: [`EntityInputHandler::replace_text_in_range`]
    #[allow(unused)]
    pub(crate) fn replace_text_in_lsp_range(
        &mut self,
        lsp_range: &lsp_types::Range,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = self.text.position_to_offset(&lsp_range.start);
        let end = self.text.position_to_offset(&lsp_range.end);
        self.replace_text_in_range_silent(
            Some(self.range_to_utf16(&(start..end))),
            new_text,
            window,
            cx,
        );
    }

    /// Replace text in range in silent.
    ///
    /// This will not trigger any UI interaction, such as auto-completion.
    pub(crate) fn replace_text_in_range_silent(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.silent_replace_text = true;
        self.replace_text_in_range(range_utf16, new_text, window, cx);
        self.silent_replace_text = false;
    }
}

impl EntityInputHandler for InputState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        adjusted_range.replace(self.range_to_utf16(&range));
        Some(self.text.slice(range).to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range.into()),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime_marked_range
            .map(|range| self.range_to_utf16(&range.into()))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.ime_marked_range = None;
    }

    /// Replace text in range.
    ///
    /// - If the new text is invalid, it will not be replaced.
    /// - If `range_utf16` is not provided, the current selected range will be used.
    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }

        if range_utf16.is_none()
            && self.ime_marked_range.is_none()
            && !self.batch_editing
            && !self.secondary_selected_ranges.is_empty()
        {
            self.replace_selection_set(vec![new_text.to_owned()], window, cx);
            return;
        }

        self.pause_blink_cursor(cx);

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or(self.selected_range.into());

        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);

        let mut new_offset = (range.start + new_text.len()).min(self.text.len());

        if self.mode.is_single_line() {
            let pending_text = self.text.to_string();
            // Check if the new text is valid
            if !self.is_valid_input(&pending_text, cx) {
                self.text = old_text;
                return;
            }

            if !self.mask_pattern.is_none() {
                let mask_text = self.mask_pattern.mask(&pending_text);
                self.text = Rope::from(mask_text.as_str());
                let new_text_len =
                    (new_text.len() + mask_text.len()).saturating_sub(pending_text.len());
                new_offset = (range.start + new_text_len).min(mask_text.len());
            }
        }

        self.push_history(&old_text, &range, &new_text);
        if !self.batch_editing {
            self.history.end_grouping();
        }
        if let Some(diagnostics) = self.mode.diagnostics_mut() {
            diagnostics.reset(&self.text)
        }
        self.text_wrapper
            .update(&self.text, &range, &Rope::from(new_text), cx);
        self.mode
            .update_highlighter(&range, &self.text, &new_text, true, cx);
        self.lsp.update(&self.text, window, cx);
        self.selected_range = (new_offset..new_offset).into();
        self.ime_marked_range.take();
        self.update_preferred_column();
        self.update_search(cx);
        self.mode.update_auto_grow(&self.text_wrapper);
        if !self.silent_replace_text {
            self.handle_completion_trigger(&range, &new_text, window, cx);
        }
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    /// Mark text is the IME temporary insert on typing.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }

        // macOS exposes one marked-text range. Composition therefore continues only at the
        // primary selection; secondary selections are intentionally collapsed before provisional
        // IME text is inserted. A committed non-composing insertion still fans out normally.
        if self.ime_marked_range.is_none() {
            self.secondary_selected_ranges.clear();
        }

        self.lsp.reset();

        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.ime_marked_range.map(|range| {
                let range = self.range_to_utf16(&(range.start..range.end));
                self.range_from_utf16(&range)
            }))
            .unwrap_or(self.selected_range.into());

        let old_text = self.text.clone();
        self.text.replace(range.clone(), new_text);

        if self.mode.is_single_line() {
            let pending_text = self.text.to_string();
            if !self.is_valid_input(&pending_text, cx) {
                self.text = old_text;
                return;
            }
        }

        if let Some(diagnostics) = self.mode.diagnostics_mut() {
            diagnostics.reset(&self.text)
        }
        self.text_wrapper
            .update(&self.text, &range, &Rope::from(new_text), cx);
        self.mode
            .update_highlighter(&range, &self.text, &new_text, true, cx);
        self.lsp.update(&self.text, window, cx);
        if new_text.is_empty() {
            // Cancel selection, when cancel IME input.
            self.selected_range = (range.start..range.start).into();
            self.ime_marked_range = None;
        } else {
            self.ime_marked_range = Some((range.start..range.start + new_text.len()).into());
            self.selected_range = new_selected_range_utf16
                .as_ref()
                .map(|range_utf16| self.range_from_utf16(range_utf16))
                .map(|new_range| new_range.start + range.start..new_range.end + range.end)
                .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len())
                .into();
        }
        self.mode.update_auto_grow(&self.text_wrapper);
        self.history.start_grouping();
        self.push_history(&old_text, &range, new_text);
        cx.notify();
    }

    /// Used to position IME candidates.
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let line_height = last_layout.line_height;
        let line_number_width = last_layout.line_number_width;
        let range = self.range_from_utf16(&range_utf16);

        let mut start_origin = None;
        let mut end_origin = None;
        let line_number_origin = point(line_number_width, px(0.));
        let mut y_offset = last_layout.visible_top;
        let mut index_offset = last_layout.visible_range_offset.start;

        for line in last_layout.lines.iter() {
            if start_origin.is_some() && end_origin.is_some() {
                break;
            }

            if start_origin.is_none() {
                if let Some(p) =
                    line.position_for_index(range.start.saturating_sub(index_offset), line_height)
                {
                    start_origin = Some(p + point(px(0.), y_offset));
                }
            }

            if end_origin.is_none() {
                if let Some(p) =
                    line.position_for_index(range.end.saturating_sub(index_offset), line_height)
                {
                    end_origin = Some(p + point(px(0.), y_offset));
                }
            }

            index_offset += line.len() + 1;
            y_offset += line.size(line_height).height;
        }

        let start_origin = start_origin.unwrap_or_default();
        let mut end_origin = end_origin.unwrap_or_default();
        // Ensure at same line.
        end_origin.y = start_origin.y;

        Some(Bounds::from_corners(
            bounds.origin + line_number_origin + start_origin,
            // + line_height for show IME panel under the cursor line.
            bounds.origin + line_number_origin + point(end_origin.x, end_origin.y + line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let last_layout = self.last_layout.as_ref()?;
        let line_height = last_layout.line_height;
        let line_point = self.last_bounds?.localize(&point)?;
        let offset = last_layout.visible_range_offset.start;

        for line in last_layout.lines.iter() {
            if let Some(utf8_index) = line.index_for_position(line_point, line_height) {
                return Some(self.offset_to_utf16(offset + utf8_index));
            }
        }

        None
    }
}

impl Focusable for InputState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InputState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._pending_update {
            self.mode
                .update_highlighter(&(0..0), &self.text, "", false, cx);
            self.lsp.update(&self.text, window, cx);
            self._pending_update = false;
        }

        div()
            .id("input-state")
            .flex_1()
            .when(self.mode.is_multi_line(), |this| this.h_full())
            .flex_grow()
            .overflow_x_hidden()
            .child(TextElement::new(cx.entity().clone()).placeholder(self.placeholder.clone()))
            .children(self.diagnostic_popover.clone())
            .children(self.context_menu.as_ref().map(|menu| menu.render()))
            .children(self.hover_popover.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{ClipboardItem, IntoElement, Render, Styled as _, div};

    use super::*;

    fn has_active_highlighter(input: &InputState) -> bool {
        match &input.mode {
            InputMode::CodeEditor { highlighter, .. } => highlighter.borrow().is_some(),
            _ => false,
        }
    }

    #[gpui::test]
    fn changing_highlighter_language_rebuilds_syntax_highlighting(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            InputState::new(window, cx)
                .code_editor("json")
                .default_value(r#"{"enabled":true}"#)
        });
        cx.run_until_parked();
        input.update(&mut cx.cx, |input, _| {
            assert!(has_active_highlighter(input));
        });

        cx.update(|_, cx| {
            input.update(cx, |input, cx| input.set_highlighter("text", cx));
        });
        cx.run_until_parked();
        input.update(&mut cx.cx, |input, _| {
            assert_eq!(input.highlighter_language(), Some("text"));
            assert!(!has_active_highlighter(input));
        });

        cx.update(|_, cx| {
            input.update(cx, |input, cx| input.set_highlighter("json", cx));
        });
        cx.run_until_parked();
        input.update(&mut cx.cx, |input, _| {
            assert_eq!(input.highlighter_language(), Some("json"));
            assert!(has_active_highlighter(input));
        });
    }

    #[gpui::test]
    fn multicursor_edit_paste_undo_and_ime_policy(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value("alpha beta gamma")
        });

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_selections(vec![0..5, 6..10], window, cx);
                input.replace_selection_set(vec!["A".to_owned(), "B".to_owned()], window, cx);
                assert_eq!(input.value().as_ref(), "A B gamma");
                assert_eq!(
                    input.selections(),
                    vec![Selection::new(1, 1), Selection::new(3, 3)]
                );

                input.undo(&Undo, window, cx);
                assert_eq!(input.value().as_ref(), "alpha beta gamma");

                input.set_selections(vec![0..5, 6..10], window, cx);
                cx.write_to_clipboard(ClipboardItem::new_string("left\nright".to_owned()));
                input.paste(&Paste, window, cx);
                assert_eq!(input.value().as_ref(), "left right gamma");

                input.set_selections(vec![0..0, 5..5], window, cx);
                input.replace_and_mark_text_in_range(None, "あ", None, window, cx);
                assert!(input.secondary_selected_ranges.is_empty());
                assert!(input.ime_marked_range.is_some());
            });
        });
    }

    #[gpui::test]
    fn cut_without_a_selection_cuts_the_logical_row_and_can_undo(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            InputState::new(window, cx)
                .code_editor("text")
                .default_value("first\nsecond\nthird")
        });

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let caret = input.text.line_start_offset(1) + 3;
                input.set_selections(vec![caret..caret], window, cx);
                input.cut(&Cut, window, cx);
                assert_eq!(input.value().as_ref(), "first\nthird");
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some("second\n".to_owned())
                );

                input.undo(&Undo, window, cx);
                assert_eq!(input.value().as_ref(), "first\nsecond\nthird");

                let caret = input.text.line_start_offset(2) + 2;
                input.set_selections(vec![caret..caret], window, cx);
                input.cut(&Cut, window, cx);
                assert_eq!(input.value().as_ref(), "first\nsecond\n");
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some("third".to_owned())
                );
            });
        });
    }

    #[gpui::test]
    fn enabling_soft_wrap_uses_the_rendered_text_width(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            InputState::new(window, cx)
                .code_editor("text")
                .soft_wrap(false)
                .default_value("a long line that should reflow cleanly at the editor edge")
        });

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let layout = input.last_layout.as_ref().expect("initial text layout");
                let expected =
                    (input.input_bounds.size.width - layout.line_number_width - RIGHT_MARGIN)
                        .max(px(0.));

                input.set_soft_wrap(true, window, cx);

                assert_eq!(input.text_wrapper.wrap_width(), Some(expected));
            });
        });
    }

    #[gpui::test]
    fn shift_up_and_down_select_one_soft_wrapped_row(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let value = "word ".repeat(400);
        let (input, cx) = cx.add_window_view(move |window, cx| {
            InputState::new(window, cx)
                .code_editor("text")
                .soft_wrap(false)
                .default_value(value)
        });
        cx.simulate_resize(gpui::size(px(320.), px(300.)));
        cx.run_until_parked();

        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.set_soft_wrap(true, window, cx));
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                assert!(input.text_wrapper.lines[0].lines_len() >= 3);
                input.move_to(3, None, cx);
                let anchor = input.cursor();
                let first_row = input.text_wrapper.offset_to_display_point(anchor).row;

                input.select_down(&SelectDown, window, cx);
                let first_target = input.cursor();
                assert_eq!(
                    input.text_wrapper.offset_to_display_point(first_target).row,
                    first_row + 1
                );
                assert_eq!(
                    input.selections(),
                    vec![Selection::new(anchor, first_target)]
                );

                input.select_down(&SelectDown, window, cx);
                let second_target = input.cursor();
                assert_eq!(
                    input
                        .text_wrapper
                        .offset_to_display_point(second_target)
                        .row,
                    first_row + 2
                );

                input.select_up(&SelectUp, window, cx);
                assert_eq!(input.cursor(), first_target);
                assert_eq!(
                    input.selections(),
                    vec![Selection::new(anchor, first_target)]
                );
            });
        });
    }

    struct ZoomHarness {
        input: Entity<InputState>,
        font_size: Pixels,
    }

    impl Render for ZoomHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(crate::input::Input::new(&self.input).text_size(self.font_size))
        }
    }

    #[gpui::test]
    fn font_scale_change_keeps_the_visible_caret_anchored(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let input_slot = Rc::new(RefCell::new(None));
        let capture = input_slot.clone();
        let text = (0..120)
            .map(|index| format!("line {index}: enough text to exercise layout"))
            .collect::<Vec<_>>()
            .join("\n");
        let (harness, cx) = cx.add_window_view(move |window, cx| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("text")
                    .soft_wrap(false)
                    .default_value(text)
            });
            *capture.borrow_mut() = Some(input.clone());
            ZoomHarness {
                input,
                font_size: px(14.),
            }
        });
        let input = input_slot.borrow().clone().expect("input");

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let offset = input.text.line_start_offset(70) + 6;
                input.set_selections(vec![offset..offset], window, cx);
                input.scroll_to(offset, None, cx);
            });
        });
        cx.run_until_parked();

        let before = input.update(&mut cx.cx, |input, _| {
            let layout = input.last_layout.as_ref().expect("caret layout");
            let display_point = input.text_wrapper.offset_to_display_point(input.cursor());
            let y = display_point.row as f32 * layout.line_height + input.scroll_handle.offset().y;
            assert!(input.preserve_cursor_anchor());
            y
        });
        harness.update(&mut cx.cx, |harness, cx| {
            harness.font_size = px(22.);
            cx.notify();
        });
        cx.run_until_parked();

        input.update(&mut cx.cx, |input, _| {
            let layout = input.last_layout.as_ref().expect("scaled caret layout");
            let display_point = input.text_wrapper.offset_to_display_point(input.cursor());
            let after =
                display_point.row as f32 * layout.line_height + input.scroll_handle.offset().y;
            assert!((after - before).abs() <= px(1.));
            assert!(input.zoom_anchor.is_none());
        });
    }

    #[gpui::test]
    fn font_scale_change_keeps_hovered_scrolled_text_anchored(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let input_slot = Rc::new(RefCell::new(None));
        let capture = input_slot.clone();
        let text = (0..180)
            .map(|index| format!("line {index}: enough text to exercise pointer zoom anchoring"))
            .collect::<Vec<_>>()
            .join("\n");
        let (harness, cx) = cx.add_window_view(move |window, cx| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("text")
                    .soft_wrap(false)
                    .default_value(text)
            });
            *capture.borrow_mut() = Some(input.clone());
            ZoomHarness {
                input,
                font_size: px(14.),
            }
        });
        let input = input_slot.borrow().clone().expect("input");

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                let cursor = input.text.line_start_offset(5);
                input.set_selections(vec![cursor..cursor], window, cx);
            });
        });
        cx.run_until_parked();
        input.update(&mut cx.cx, |input, cx| {
            let scrolled_target = input.text.line_start_offset(100);
            input.scroll_to(scrolled_target, None, cx);
        });
        cx.run_until_parked();

        let (anchor_offset, before) = input.update(&mut cx.cx, |input, _| {
            let pointer = point(
                input.input_bounds.left() + px(140.),
                input.input_bounds.top() + input.input_bounds.size.height / 2.,
            );
            let anchor_offset = input.index_for_mouse_position(pointer);
            assert!(input.text.offset_to_position(anchor_offset).line > 50);
            assert_eq!(input.cursor_position().line, 5);
            let layout = input.last_layout.as_ref().expect("initial layout");
            let display_point = input.text_wrapper.offset_to_display_point(anchor_offset);
            let viewport_y =
                display_point.row as f32 * layout.line_height + input.scroll_handle.offset().y;
            assert!(input.preserve_zoom_anchor_at(pointer));
            (anchor_offset, viewport_y)
        });

        harness.update(&mut cx.cx, |harness, cx| {
            harness.font_size = px(24.);
            cx.notify();
        });
        cx.run_until_parked();

        input.update(&mut cx.cx, |input, _| {
            let layout = input.last_layout.as_ref().expect("scaled layout");
            let display_point = input.text_wrapper.offset_to_display_point(anchor_offset);
            let after =
                display_point.row as f32 * layout.line_height + input.scroll_handle.offset().y;
            assert!((after - before).abs() <= px(1.));
            assert_eq!(input.cursor_position().line, 5);
            assert!(input.zoom_anchor.is_none());
        });
    }

    #[gpui::test]
    fn zoom_anchor_moves_the_scroll_offset_before_the_next_visible_range(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::init);
        let input_slot = Rc::new(RefCell::new(None));
        let capture = input_slot.clone();
        let text = (0..200)
            .map(|index| format!("line {index}: scrolled text under the pointer"))
            .collect::<Vec<_>>()
            .join("\n");
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("text")
                    .soft_wrap(false)
                    .default_value(text)
            });
            *capture.borrow_mut() = Some(input.clone());
            ZoomHarness {
                input,
                font_size: px(14.),
            }
        });
        let input = input_slot.borrow().clone().expect("input");

        input.update(&mut cx.cx, |input, cx| {
            input.scroll_to(input.text.line_start_offset(120), None, cx);
        });
        cx.run_until_parked();

        input.update(&mut cx.cx, |input, _| {
            let pointer = point(
                input.input_bounds.left() + px(120.),
                input.input_bounds.top() + input.input_bounds.size.height / 2.,
            );
            let anchor_offset = input.index_for_mouse_position(pointer);
            assert!(input.preserve_zoom_anchor_at(pointer));
            let anchor_viewport_y = input.zoom_anchor.expect("anchor").viewport_y;
            let old_line_height = input.last_layout.as_ref().expect("layout").line_height;

            // The offset must be anchored synchronously: the next frame computes
            // its visible line range from this offset, and any later correction
            // paints a frame whose shaped lines do not cover the viewport.
            let new_line_height = old_line_height * 1.5;
            input.apply_zoom_anchor(new_line_height);

            let display_row = input
                .text_wrapper
                .offset_to_display_point(anchor_offset)
                .row as f32;
            let expected = (anchor_viewport_y - display_row * new_line_height).min(px(0.));
            assert_eq!(input.scroll_handle.offset().y, expected);
            // The anchored row still sits at its captured viewport position.
            let after = display_row * new_line_height + input.scroll_handle.offset().y;
            assert!((after - anchor_viewport_y).abs() <= px(1.));
        });
    }
}

use std::cell::Cell;
use std::ops::Range;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, DefiniteLength, DragMoveEvent, Edges,
    EdgesRefinement, Empty, Entity, EntityId, InteractiveElement as _, IntoElement, IsZero,
    MouseButton, ParentElement as _, Pixels, Rems, Render, RenderOnce,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div, point, px, relative,
};

use crate::button::{Button, ButtonVariants as _};
use crate::input::clear_button;
use crate::input::element::{LINE_NUMBER_RIGHT_MARGIN, RIGHT_MARGIN};
use crate::scroll::Scrollbar;
use crate::spinner::Spinner;
use crate::{ActiveTheme, v_flex};
use crate::{IconName, Size};
use crate::{Selectable, StyledExt, h_flex};
use crate::{Sizable, StyleSized};

use super::{InputState, RopeExt as _};

const MINIMAP_WIDTH: f32 = 88.;
const MINIMAP_MAX_SAMPLES: usize = 120;
const MINIMAP_MAX_LINE_LENGTH: usize = 160;
const MINIMAP_WINDOW_ROWS: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq)]
struct MinimapSample {
    width: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct MinimapWindow {
    rows: Range<usize>,
    line_count: usize,
    viewport_top: f32,
    viewport_height: f32,
}

#[derive(Clone)]
struct MinimapViewportDrag {
    input: EntityId,
    grab_y: Cell<Pixels>,
}

impl Render for MinimapViewportDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

fn minimap_sample_rows(rows: Range<usize>) -> Vec<usize> {
    let start = rows.start;
    let line_count = rows.end.saturating_sub(start).max(1);
    let sample_count = line_count.min(MINIMAP_MAX_SAMPLES);
    (0..sample_count)
        .map(|index| {
            if sample_count == 1 {
                start
            } else {
                start + index.saturating_mul(line_count - 1) / (sample_count - 1)
            }
        })
        .collect()
}

fn minimap_samples(state: &InputState, rows: Range<usize>) -> Vec<MinimapSample> {
    minimap_sample_rows(rows)
        .into_iter()
        .map(|row| {
            let line_length = state.text.line_len(row).min(MINIMAP_MAX_LINE_LENGTH);
            let width =
                2. + line_length as f32 / MINIMAP_MAX_LINE_LENGTH as f32 * (MINIMAP_WIDTH - 10.);
            MinimapSample { width }
        })
        .collect()
}

fn minimap_document_viewport(state: &InputState) -> (f32, f32) {
    let content_height = state.scroll_size.height;
    let viewport_height = state.input_bounds.size.height;
    if content_height <= px(0.) || viewport_height >= content_height {
        return (0., 1.);
    }

    let height = (viewport_height / content_height).clamp(0.03, 1.);
    let scroll_top = (-state.scroll_handle.offset().y).max(px(0.));
    let top = (scroll_top / content_height).clamp(0., 1. - height);
    (top, height)
}

fn minimap_window_for(
    line_count: usize,
    document_viewport_top: f32,
    document_viewport_height: f32,
) -> MinimapWindow {
    let line_count = line_count.max(1);
    let window_len = line_count.min(MINIMAP_WINDOW_ROWS);
    let visible_center =
        ((document_viewport_top + document_viewport_height / 2.) * line_count as f32) as usize;
    let start = visible_center
        .saturating_sub(window_len / 2)
        .min(line_count - window_len);
    let end = start + window_len;
    let viewport_height =
        (document_viewport_height * line_count as f32 / window_len as f32).clamp(0.03, 1.);
    let viewport_top = ((document_viewport_top * line_count as f32 - start as f32)
        / window_len as f32)
        .clamp(0., 1. - viewport_height);

    MinimapWindow {
        rows: start..end,
        line_count,
        viewport_top,
        viewport_height,
    }
}

fn minimap_window(state: &InputState) -> MinimapWindow {
    let (top, height) = minimap_document_viewport(state);
    minimap_window_for(state.text.lines_len(), top, height)
}

fn minimap_document_scroll_fraction(window: &MinimapWindow, local_fraction: f32) -> f32 {
    if window.line_count <= 1 {
        return 0.;
    }
    let row = window.rows.start as f32 + window.rows.len() as f32 * local_fraction.clamp(0., 1.);
    (row / (window.line_count - 1) as f32).clamp(0., 1.)
}

fn minimap_drag_scroll_fraction(
    pointer_y: Pixels,
    minimap_top: Pixels,
    minimap_height: Pixels,
    viewport_height: f32,
    grab_y: Pixels,
) -> f32 {
    let viewport_pixels = minimap_height * viewport_height;
    let track = minimap_height - viewport_pixels;
    if track <= px(0.) {
        return 0.;
    }
    ((pointer_y - minimap_top - grab_y) / track).clamp(0., 1.)
}

/// A text input element bind to an [`InputState`].
#[derive(IntoElement)]
pub struct Input {
    state: Entity<InputState>,
    style: StyleRefinement,
    size: Size,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    height: Option<DefiniteLength>,
    appearance: bool,
    cleanable: bool,
    mask_toggle: bool,
    disabled: bool,
    bordered: bool,
    focus_bordered: bool,
    minimap: bool,
    tab_index: isize,
    selected: bool,
}

impl Sizable for Input {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Selectable for Input {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl Input {
    /// Create a new [`Input`] element bind to the [`InputState`].
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            size: Size::default(),
            style: StyleRefinement::default(),
            prefix: None,
            suffix: None,
            height: None,
            appearance: true,
            cleanable: false,
            mask_toggle: false,
            disabled: false,
            bordered: true,
            focus_bordered: true,
            minimap: false,
            tab_index: 0,
            selected: false,
        }
    }

    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    /// Set full height of the input (Multi-line only).
    pub fn h_full(mut self) -> Self {
        self.height = Some(relative(1.));
        self
    }

    /// Set height of the input (Multi-line only).
    pub fn h(mut self, height: impl Into<DefiniteLength>) -> Self {
        self.height = Some(height.into());
        self
    }

    /// Set the appearance of the input field, if false the input field will no border, background.
    pub fn appearance(mut self, appearance: bool) -> Self {
        self.appearance = appearance;
        self
    }

    /// Set the bordered for the input, default: true
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Returns whether the input draws its outer border.
    pub fn has_border(&self) -> bool {
        self.bordered
    }

    /// Set focus border for the input, default is true.
    pub fn focus_bordered(mut self, bordered: bool) -> Self {
        self.focus_bordered = bordered;
        self
    }

    /// Show a compact, clickable document overview beside a code editor.
    pub fn minimap(mut self, minimap: bool) -> Self {
        self.minimap = minimap;
        self
    }

    /// Returns whether the editor minimap is enabled for this element.
    pub fn has_minimap(&self) -> bool {
        self.minimap
    }

    /// Set whether to show the clear button when the input field is not empty, default is false.
    pub fn cleanable(mut self, cleanable: bool) -> Self {
        self.cleanable = cleanable;
        self
    }

    /// Set to enable toggle button for password mask state.
    pub fn mask_toggle(mut self) -> Self {
        self.mask_toggle = true;
        self
    }

    /// Set to disable the input field.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the tab index for the input, default is 0.
    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = index;
        self
    }

    fn render_toggle_mask_button(state: Entity<InputState>) -> impl IntoElement {
        Button::new("toggle-mask")
            .icon(IconName::Eye)
            .xsmall()
            .ghost()
            .tab_stop(false)
            .on_mouse_down(MouseButton::Left, {
                let state = state.clone();
                move |_, window, cx| {
                    state.update(cx, |state, cx| {
                        state.set_masked(false, window, cx);
                    })
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let state = state.clone();
                move |_, window, cx| {
                    state.update(cx, |state, cx| {
                        state.set_masked(true, window, cx);
                    })
                }
            })
    }

    /// This method must after the refine_style.
    fn render_editor(
        paddings: EdgesRefinement<DefiniteLength>,
        input_state: &Entity<InputState>,
        state: &InputState,
        minimap: bool,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement {
        let base_size = window.text_style().font_size;
        let rem_size = window.rem_size();

        let paddings = Edges {
            left: paddings
                .left
                .map(|v| v.to_pixels(base_size, rem_size))
                .unwrap_or(px(0.)),
            right: paddings
                .right
                .map(|v| v.to_pixels(base_size, rem_size))
                .unwrap_or(px(0.)),
            top: paddings
                .top
                .map(|v| v.to_pixels(base_size, rem_size))
                .unwrap_or(px(0.)),
            bottom: paddings
                .bottom
                .map(|v| v.to_pixels(base_size, rem_size))
                .unwrap_or(px(0.)),
        };

        let editor = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(input_state.clone())
            .map(|this| {
                if let Some(last_layout) = state.last_layout.as_ref() {
                    let left = if last_layout.line_number_width.is_zero() {
                        px(0.)
                    } else {
                        // Align left edge to the Line number.
                        paddings.left + last_layout.line_number_width - LINE_NUMBER_RIGHT_MARGIN
                    };

                    let scroll_size = gpui::Size {
                        width: state.scroll_size.width - left + paddings.right + RIGHT_MARGIN,
                        height: state.scroll_size.height,
                    };

                    let scrollbar = if !state.soft_wrap {
                        Scrollbar::new(&state.scroll_handle)
                    } else {
                        Scrollbar::vertical(&state.scroll_handle)
                    };

                    this.relative().child(
                        div()
                            .absolute()
                            .top(-paddings.top)
                            .left(left)
                            .right(-paddings.right)
                            .bottom(-paddings.bottom)
                            .child(scrollbar.scroll_size(scroll_size)),
                    )
                } else {
                    this
                }
            });

        let minimap_element = minimap.then(|| {
            let minimap_window = minimap_window(state);
            let samples = minimap_samples(state, minimap_window.rows.clone());
            let sample_count = samples.len();
            let viewport_top = minimap_window.viewport_top;
            let viewport_height = minimap_window.viewport_height;
            let state_for_track = input_state.clone();
            let input_id = input_state.entity_id();
            let window_for_track = minimap_window.clone();
            let window_for_drag = minimap_window;

            div()
                .id("editor-minimap")
                .debug_selector(|| "editor-minimap".to_owned())
                .relative()
                .h_full()
                .w(px(MINIMAP_WIDTH))
                .flex_none()
                .overflow_hidden()
                .border_l_1()
                .border_color(cx.theme().border.opacity(0.45))
                .bg(cx.theme().editor_background())
                .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                    state_for_track.update(cx, |state, cx| {
                        let scrollable =
                            (state.scroll_size.height - state.input_bounds.size.height).max(px(0.));
                        let local_fraction = ((event.position.y - state.input_bounds.top())
                            / state.input_bounds.size.height)
                            .clamp(0., 1.);
                        let fraction =
                            minimap_document_scroll_fraction(&window_for_track, local_fraction);
                        let mut offset = state.scroll_handle.offset();
                        offset.y = -scrollable * fraction;
                        state.update_scroll_offset(Some(point(offset.x, offset.y)), cx);
                    });
                    cx.stop_propagation();
                })
                .on_drag_move(window.listener_for(
                    input_state,
                    move |state, event: &DragMoveEvent<MinimapViewportDrag>, _, cx| {
                        let drag = event.drag(cx);
                        if drag.input != input_id {
                            return;
                        }
                        let local_fraction = minimap_drag_scroll_fraction(
                            event.event.position.y,
                            event.bounds.top(),
                            event.bounds.size.height,
                            window_for_drag.viewport_height,
                            drag.grab_y.get(),
                        );
                        let fraction =
                            minimap_document_scroll_fraction(&window_for_drag, local_fraction);
                        let scrollable =
                            (state.scroll_size.height - state.input_bounds.size.height).max(px(0.));
                        let mut offset = state.scroll_handle.offset();
                        offset.y = -scrollable * fraction;
                        state.update_scroll_offset(Some(point(offset.x, offset.y)), cx);
                        cx.stop_propagation();
                    },
                ))
                .children(samples.into_iter().enumerate().map(move |(index, sample)| {
                    let top = index as f32 / sample_count as f32;
                    let height = 1. / sample_count as f32;
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(relative(top))
                        .h(relative(height))
                        .pl(px(4.))
                        .cursor_pointer()
                        .child(
                            div()
                                .mt(px(1.))
                                .h(px(2.))
                                .w(px(sample.width))
                                .bg(cx.theme().muted_foreground.opacity(0.58)),
                        )
                }))
                .child(
                    div()
                        .id("editor-minimap-viewport")
                        .debug_selector(|| "editor-minimap-viewport".to_owned())
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(relative(viewport_top))
                        .h(relative(viewport_height))
                        .cursor_ns_resize()
                        .bg(cx.theme().muted_foreground.opacity(0.12))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_drag(
                            MinimapViewportDrag {
                                input: input_id,
                                grab_y: Cell::new(px(0.)),
                            },
                            move |drag, cursor_offset, _, cx| {
                                drag.grab_y.set(cursor_offset.y);
                                cx.stop_propagation();
                                cx.new(|_| drag.clone())
                            },
                        ),
                )
        });

        v_flex()
            .size_full()
            .children(state.search_panel.clone())
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(editor)
                    .children(minimap_element),
            )
    }
}

impl Styled for Input {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Input {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        const LINE_HEIGHT: Rems = Rems(1.25);

        self.state.update(cx, |state, _| {
            state.disabled = self.disabled;
            state.size = self.size;
        });

        let state = self.state.read(cx);
        let focused = state.focus_handle.is_focused(window);
        let gap_x = match self.size {
            Size::Small => px(4.),
            Size::Large => px(8.),
            _ => px(6.),
        };

        let bg = if state.disabled {
            cx.theme().muted
        } else {
            if state.mode.is_code_editor() {
                cx.theme().editor_background()
            } else {
                cx.theme().background
            }
        };

        let prefix = self.prefix;
        let suffix = self.suffix;
        let show_clear_button =
            self.cleanable && !state.loading && state.text.len() > 0 && state.mode.is_single_line();
        let has_suffix = suffix.is_some() || state.loading || self.mask_toggle || show_clear_button;

        div()
            .id(("input", self.state.entity_id()))
            .flex()
            .key_context(crate::input::CONTEXT)
            .track_focus(&state.focus_handle.clone())
            .tab_index(self.tab_index)
            .when(!state.disabled, |this| {
                this.on_action(window.listener_for(&self.state, InputState::backspace))
                    .on_action(window.listener_for(&self.state, InputState::delete))
                    .on_action(
                        window.listener_for(&self.state, InputState::delete_to_beginning_of_line),
                    )
                    .on_action(window.listener_for(&self.state, InputState::delete_to_end_of_line))
                    .on_action(window.listener_for(&self.state, InputState::delete_previous_word))
                    .on_action(window.listener_for(&self.state, InputState::delete_next_word))
                    .on_action(window.listener_for(&self.state, InputState::enter))
                    .on_action(window.listener_for(&self.state, InputState::escape))
                    .on_action(window.listener_for(&self.state, InputState::paste))
                    .on_action(window.listener_for(&self.state, InputState::cut))
                    .on_action(window.listener_for(&self.state, InputState::undo))
                    .on_action(window.listener_for(&self.state, InputState::redo))
                    .when(state.mode.is_multi_line(), |this| {
                        this.on_action(window.listener_for(&self.state, InputState::indent_inline))
                            .on_action(window.listener_for(&self.state, InputState::outdent_inline))
                            .on_action(window.listener_for(&self.state, InputState::indent_block))
                            .on_action(window.listener_for(&self.state, InputState::outdent_block))
                    })
                    .on_action(
                        window.listener_for(&self.state, InputState::on_action_toggle_code_actions),
                    )
            })
            .on_action(window.listener_for(&self.state, InputState::left))
            .on_action(window.listener_for(&self.state, InputState::right))
            .on_action(window.listener_for(&self.state, InputState::select_left))
            .on_action(window.listener_for(&self.state, InputState::select_right))
            .when(state.mode.is_multi_line(), |this| {
                this.on_action(window.listener_for(&self.state, InputState::up))
                    .on_action(window.listener_for(&self.state, InputState::down))
                    .on_action(window.listener_for(&self.state, InputState::select_up))
                    .on_action(window.listener_for(&self.state, InputState::select_down))
                    .on_action(window.listener_for(&self.state, InputState::page_up))
                    .on_action(window.listener_for(&self.state, InputState::page_down))
                    .on_action(
                        window.listener_for(&self.state, InputState::on_action_go_to_definition),
                    )
            })
            .on_action(window.listener_for(&self.state, InputState::select_all))
            .on_action(window.listener_for(&self.state, InputState::select_to_start_of_line))
            .on_action(window.listener_for(&self.state, InputState::select_to_end_of_line))
            .on_action(window.listener_for(&self.state, InputState::select_to_previous_word))
            .on_action(window.listener_for(&self.state, InputState::select_to_next_word))
            .on_action(window.listener_for(&self.state, InputState::home))
            .on_action(window.listener_for(&self.state, InputState::end))
            .on_action(window.listener_for(&self.state, InputState::move_to_start))
            .on_action(window.listener_for(&self.state, InputState::move_to_end))
            .on_action(window.listener_for(&self.state, InputState::move_to_previous_word))
            .on_action(window.listener_for(&self.state, InputState::move_to_next_word))
            .on_action(window.listener_for(&self.state, InputState::select_to_start))
            .on_action(window.listener_for(&self.state, InputState::select_to_end))
            .on_action(window.listener_for(&self.state, InputState::show_character_palette))
            .on_action(window.listener_for(&self.state, InputState::copy))
            .on_action(window.listener_for(&self.state, InputState::on_action_search))
            .on_key_down(window.listener_for(&self.state, InputState::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&self.state, InputState::on_mouse_down),
            )
            .on_mouse_down(
                MouseButton::Right,
                window.listener_for(&self.state, InputState::on_mouse_down),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&self.state, InputState::on_mouse_up),
            )
            .on_mouse_up(
                MouseButton::Right,
                window.listener_for(&self.state, InputState::on_mouse_up),
            )
            .on_mouse_move(window.listener_for(&self.state, InputState::on_mouse_move))
            .on_scroll_wheel(window.listener_for(&self.state, InputState::on_scroll_wheel))
            .size_full()
            .line_height(LINE_HEIGHT)
            .input_px(self.size)
            .input_py(self.size)
            .input_h(self.size)
            .input_text_size(self.size)
            .cursor_text()
            .items_center()
            .when(state.mode.is_multi_line(), |this| {
                this.h_auto()
                    .when_some(self.height, |this, height| this.h(height))
            })
            .when(self.appearance, |this| {
                this.bg(bg)
                    .rounded(cx.theme().radius)
                    .when(self.bordered, |this| {
                        this.border_color(cx.theme().input)
                            .border_1()
                            .when(cx.theme().shadow, |this| this.shadow_xs())
                            .when(focused && self.focus_bordered, |this| {
                                this.focused_border(cx)
                            })
                    })
            })
            .items_center()
            .gap(gap_x)
            .refine_style(&self.style)
            .children(prefix)
            .when(state.mode.is_multi_line(), |mut this| {
                let paddings = this.style().padding.clone();
                this.child(Self::render_editor(
                    paddings,
                    &self.state,
                    &state,
                    self.minimap && state.mode.is_code_editor(),
                    window,
                    cx,
                ))
            })
            .when(!state.mode.is_multi_line(), |this| {
                this.child(self.state.clone())
            })
            .when(has_suffix, |this| {
                this.pr(self.size.input_px()).child(
                    h_flex()
                        .id("suffix")
                        .gap(gap_x)
                        .when(self.appearance, |this| this.bg(bg))
                        .items_center()
                        .when(state.loading, |this| {
                            this.child(Spinner::new().color(cx.theme().muted_foreground))
                        })
                        .when(self.mask_toggle, |this| {
                            this.child(Self::render_toggle_mask_button(self.state.clone()))
                        })
                        .when(show_clear_button, |this| {
                            this.child(clear_button(cx).on_click({
                                let state = self.state.clone();
                                move |_, window, cx| {
                                    state.update(cx, |state, cx| {
                                        state.clean(window, cx);
                                        state.focus(window, cx);
                                    })
                                }
                            }))
                        })
                        .children(suffix),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MinimapHarness {
        state: Entity<InputState>,
    }

    impl gpui::Render for MinimapHarness {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            Input::new(&self.state).minimap(true).size_full()
        }
    }

    #[test]
    fn minimap_sampling_is_bounded_and_spans_its_window() {
        assert_eq!(minimap_sample_rows(0..0), vec![0]);
        assert_eq!(minimap_sample_rows(0..1), vec![0]);
        assert_eq!(minimap_sample_rows(10..13), vec![10, 11, 12]);

        let rows = minimap_sample_rows(4_500..5_500);
        assert_eq!(rows.len(), MINIMAP_MAX_SAMPLES);
        assert_eq!(rows.first(), Some(&4_500));
        assert_eq!(rows.last(), Some(&5_499));
        assert!(rows.windows(2).all(|rows| rows[0] < rows[1]));
    }

    #[test]
    fn minimap_window_tracks_a_bounded_region_of_long_documents() {
        let top = minimap_window_for(10_000, 0., 0.01);
        assert_eq!(top.rows, 0..1_000);
        assert_eq!(top.viewport_top, 0.);

        let middle = minimap_window_for(10_000, 0.495, 0.01);
        assert_eq!(middle.rows, 4_500..5_500);
        assert!(middle.viewport_top > 0.4);
        assert!(middle.viewport_top < 0.6);

        let bottom = minimap_window_for(10_000, 0.99, 0.01);
        assert_eq!(bottom.rows, 9_000..10_000);
        assert!((bottom.viewport_top - 0.9).abs() < 0.0001);
        assert_eq!(
            minimap_document_scroll_fraction(&middle, 0.5),
            5_000. / 9_999.
        );
    }

    #[test]
    fn minimap_drag_preserves_the_grab_point_and_clamps_to_the_track() {
        let minimap_top = px(40.);
        let minimap_height = px(400.);
        let viewport_height = 0.2;
        let grab_y = px(30.);

        assert_eq!(
            minimap_drag_scroll_fraction(
                minimap_top + grab_y,
                minimap_top,
                minimap_height,
                viewport_height,
                grab_y,
            ),
            0.
        );
        assert_eq!(
            minimap_drag_scroll_fraction(
                minimap_top + minimap_height - minimap_height * viewport_height + grab_y,
                minimap_top,
                minimap_height,
                viewport_height,
                grab_y,
            ),
            1.
        );
    }

    #[gpui::test]
    fn clicking_the_minimap_navigates_the_editor(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let text = (0..500)
            .map(|line| format!("line {line}: minimap navigation target\n"))
            .collect::<String>();
        let state_slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let capture = state_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("text")
                    .soft_wrap(false)
                    .default_value(text)
            });
            *capture.borrow_mut() = Some(state.clone());
            let harness = cx.new(|_| MinimapHarness { state });
            crate::Root::new(harness, window, cx)
        });
        let state = state_slot.borrow().clone().expect("input state");
        cx.simulate_resize(gpui::size(px(800.), px(420.)));
        cx.run_until_parked();

        let minimap = cx.debug_bounds("editor-minimap").expect("minimap");
        let before = cx.update(|_, cx| state.read(cx).scroll_handle.offset().y);
        cx.simulate_click(
            point(minimap.center().x, minimap.bottom() - px(3.)),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        let after = cx.update(|_, cx| state.read(cx).scroll_handle.offset().y);

        assert_eq!(before, px(0.));
        assert!(after < before);
    }

    #[gpui::test]
    fn dragging_the_minimap_viewport_scrolls_the_editor(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let text = (0..500)
            .map(|line| format!("line {line}: draggable minimap viewport\n"))
            .collect::<String>();
        let state_slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let capture = state_slot.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("text")
                    .soft_wrap(false)
                    .default_value(text)
            });
            *capture.borrow_mut() = Some(state.clone());
            let harness = cx.new(|_| MinimapHarness { state });
            crate::Root::new(harness, window, cx)
        });
        let state = state_slot.borrow().clone().expect("input state");
        cx.simulate_resize(gpui::size(px(800.), px(420.)));
        cx.run_until_parked();

        let minimap = cx.debug_bounds("editor-minimap").expect("minimap");
        let viewport = cx
            .debug_bounds("editor-minimap-viewport")
            .expect("minimap viewport");
        let grab = viewport.center();
        cx.simulate_mouse_down(grab, MouseButton::Left, gpui::Modifiers::none());
        cx.simulate_mouse_move(
            point(grab.x, grab.y + px(8.)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        cx.simulate_mouse_move(
            point(grab.x, minimap.bottom() - px(4.)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        cx.simulate_mouse_up(
            point(grab.x, minimap.bottom() - px(4.)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();

        let after = cx.update(|_, cx| state.read(cx).scroll_handle.offset().y);
        assert!(after < px(0.));
    }
}

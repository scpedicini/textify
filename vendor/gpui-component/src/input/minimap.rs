use std::{cell::Cell, ops::Range, rc::Rc};

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, Entity, HighlightStyle, HitboxBehavior,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, RenderOnce, ShapedLine, Styled, TextRun, Window,
    canvas, div, fill, point, px, size,
};

use crate::{ActiveTheme, input::mode::InputMode};

use super::{InputState, RopeExt as _};

const WIDTH: Pixels = px(88.);
const LINE_HEIGHT: Pixels = px(3.);
const FONT_SIZE: Pixels = px(2.5);
const LEFT_PADDING: Pixels = px(4.);
const MAX_LINE_CHARS: usize = 160;

/// Published by the editor's prepaint before its sibling minimap is laid out.
/// Keep these measurements together: `last_layout` and `scroll_size` still refer
/// to the previous frame at that point, especially during zoom and rewrapping.
#[derive(Clone, Copy)]
pub(super) struct MinimapMetrics {
    pub line_height: Pixels,
    pub viewport_height: Pixels,
    pub content_height: Pixels,
    pub scroll_top: Pixels,
}

/// Like VS Code's MinimapLayout, the slider spans the editor's full scroll range.
/// The miniature text scrolls underneath it when the document doesn't fit.
/// Reference: src/vs/editor/browser/viewParts/minimap/minimap.ts in VS Code.
#[derive(Clone, Copy, Debug)]
struct MinimapLayout {
    scroll_top: Pixels,
    max_scroll_top: Pixels,
    viewport_height: Pixels,
    scale: f32,
    slider_top: Pixels,
    slider_height: Pixels,
    slider_ratio: f32,
    content_top: Pixels,
}

impl MinimapLayout {
    fn new(
        height: Pixels,
        line_height: Pixels,
        viewport_height: Pixels,
        content_height: Pixels,
        scroll_top: Pixels,
    ) -> Self {
        let height = height.max(px(0.));
        let viewport_height = viewport_height.max(px(0.));
        let content_height = content_height.max(viewport_height);
        let scale = LINE_HEIGHT / line_height.max(LINE_HEIGHT);
        let max_scroll_top = (content_height - viewport_height).max(px(0.));
        let scroll_top = scroll_top.clamp(px(0.), max_scroll_top);
        let slider_height = (viewport_height * scale).min(height);
        let track = (content_height * scale - slider_height)
            .min(height - slider_height)
            .max(px(0.));
        let slider_ratio = if max_scroll_top > px(0.) {
            track / max_scroll_top
        } else {
            0.
        };
        let slider_top = scroll_top * slider_ratio;

        Self {
            scroll_top,
            max_scroll_top,
            viewport_height,
            scale,
            slider_top,
            slider_height,
            slider_ratio,
            content_top: (scroll_top * scale - slider_top).max(px(0.)),
        }
    }

    fn for_input(state: &InputState, height: Pixels, window: &Window) -> Self {
        let metrics = state.minimap_metrics.unwrap_or(MinimapMetrics {
            line_height: window.line_height(),
            viewport_height: height,
            content_height: height,
            scroll_top: px(0.),
        });
        Self::new(
            height,
            metrics.line_height,
            metrics.viewport_height,
            metrics.content_height,
            metrics.scroll_top,
        )
    }

    fn visible_rows(&self, height: Pixels, row_count: usize) -> Range<usize> {
        let start = (self.content_top / LINE_HEIGHT).floor() as usize;
        let end = ((self.content_top + height) / LINE_HEIGHT).ceil() as usize;
        start.min(row_count)..end.min(row_count)
    }

    fn scroll_top_for_click(&self, local_y: Pixels) -> Pixels {
        ((self.content_top + local_y) / self.scale - self.viewport_height / 2.)
            .clamp(px(0.), self.max_scroll_top)
    }
}

/// Snapshot on mouse-down, before any movement or redraw. Recomputing this from
/// the moving preview (or GPUI's drag threshold position) makes the thumb jump.
#[derive(Clone, Copy)]
struct MinimapDrag {
    pointer_y: Pixels,
    scroll_top: Pixels,
    max_scroll_top: Pixels,
    slider_ratio: f32,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum MinimapVisualState {
    #[default]
    Idle,
    MapHovered,
    SliderHovered,
    Dragging,
}

impl MinimapVisualState {
    fn for_pointer(map_hovered: bool, slider_hovered: bool, dragging: bool) -> Self {
        if dragging {
            Self::Dragging
        } else if map_hovered && slider_hovered {
            Self::SliderHovered
        } else if map_hovered {
            Self::MapHovered
        } else {
            Self::Idle
        }
    }

    fn opacity(self) -> f32 {
        match self {
            Self::Idle => 0.10,
            Self::MapHovered => 0.17,
            Self::SliderHovered => 0.24,
            Self::Dragging => 0.32,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct MinimapState {
    drag: Option<MinimapDrag>,
    visual: MinimapVisualState,
}

impl MinimapDrag {
    fn rebase_for_layout(&mut self, layout: MinimapLayout, pointer_y: Pixels) {
        // Ordinary scroll redraws must retain the mouse-down anchor. Only a
        // changed mapping (zoom, wrapping, resize, document length) starts a new
        // segment at the current pointer and viewport, with no synthetic scroll.
        if self.max_scroll_top != layout.max_scroll_top || self.slider_ratio != layout.slider_ratio
        {
            self.pointer_y = pointer_y;
            self.scroll_top = layout.scroll_top;
            self.max_scroll_top = layout.max_scroll_top;
            self.slider_ratio = layout.slider_ratio;
        }
    }

    fn scroll_top(&self, pointer_y: Pixels) -> Pixels {
        if self.slider_ratio <= 0. {
            return self.scroll_top;
        }
        (self.scroll_top + (pointer_y - self.pointer_y) / self.slider_ratio)
            .clamp(px(0.), self.max_scroll_top)
    }
}

fn set_scroll_top(input: &Entity<InputState>, scroll_top: Pixels, cx: &mut App) {
    input.update(cx, |state, cx| {
        let offset = state.scroll_handle.offset();
        state.update_scroll_offset(Some(point(offset.x, -scroll_top)), cx);
    });
}

/// Expand tabs without losing UTF-8 byte alignment with syntax colors. Runs use
/// only syntax foreground colors: editor decorations don't belong in the map.
fn preview_text(
    source: &str,
    offset: usize,
    styles: &[(Range<usize>, HighlightStyle)],
    base: &TextRun,
    tab_size: usize,
) -> (String, Vec<TextRun>) {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut column = 0;
    let mut style_index = 0;
    for (index, ch) in source.char_indices() {
        if column >= MAX_LINE_CHARS {
            break;
        }
        let index = offset + index;
        while styles
            .get(style_index)
            .is_some_and(|(range, _)| range.end <= index)
        {
            style_index += 1;
        }
        let color = styles
            .get(style_index)
            .filter(|(range, _)| range.contains(&index))
            .and_then(|(_, style)| style.color)
            .unwrap_or(base.color);
        let start = text.len();
        if ch == '\t' {
            let count = (tab_size.max(1) - column % tab_size.max(1)).min(MAX_LINE_CHARS - column);
            text.extend(std::iter::repeat_n(' ', count));
            column += count;
        } else {
            text.push(if ch.is_control() { ' ' } else { ch });
            column += 1;
        }
        let len = text.len() - start;
        if let Some(run) = runs.last_mut().filter(|run| run.color == color) {
            run.len += len;
        } else {
            runs.push(TextRun {
                len,
                color,
                ..base.clone()
            });
        }
    }
    (text, runs)
}

fn preview_lines(
    state: &InputState,
    layout: MinimapLayout,
    height: Pixels,
    window: &Window,
    cx: &App,
) -> Vec<(Pixels, ShapedLine)> {
    let rows = layout.visible_rows(height, state.text_wrapper.len());
    let mut result = Vec::with_capacity(rows.len());
    let base = TextRun {
        len: 0,
        font: window.text_style().font(),
        color: cx.theme().foreground,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let (highlighter, tab_size) = match &state.mode {
        InputMode::CodeEditor {
            highlighter, tab, ..
        } => (Some(highlighter.borrow()), tab.tab_size),
        _ => (None, 2),
    };
    let highlighter = highlighter.as_deref().and_then(|value| value.as_ref());
    let mut display_row = 0;
    for (row, line) in state.text_wrapper.lines.iter().enumerate() {
        let line_end = display_row + line.wrapped_lines.len();
        if display_row >= rows.end {
            break;
        }
        if line_end > rows.start {
            let line_offset = state.text.line_start_offset(row);
            for local_row in rows.start.saturating_sub(display_row)..line.wrapped_lines.len() {
                let row = display_row + local_row;
                if row >= rows.end {
                    break;
                }
                let range = &line.wrapped_lines[local_row];
                let start = line_offset + range.start;
                // Never materialize a whole pathological line or query its entire syntax tree.
                let source: String = state
                    .text
                    .slice(start..line_offset + range.end)
                    .chars()
                    .take(MAX_LINE_CHARS)
                    .collect();
                if source.is_empty() {
                    continue;
                }
                let styles = highlighter.map_or_else(Vec::new, |highlighter| {
                    highlighter.styles(&(start..start + source.len()), &cx.theme().highlight_theme)
                });
                let (text, runs) = preview_text(&source, start, &styles, &base, tab_size);
                let shaped = window
                    .text_system()
                    .shape_line(text.into(), FONT_SIZE, &runs, None);
                result.push((row as f32 * LINE_HEIGHT - layout.content_top, shaped));
            }
        }
        display_row = line_end;
    }
    result
}

#[derive(IntoElement)]
pub(super) struct Minimap {
    input: Entity<InputState>,
}

impl Minimap {
    pub(super) fn new(input: &Entity<InputState>) -> Self {
        Self {
            input: input.clone(),
        }
    }
}

impl RenderOnce for Minimap {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let interaction = window
            .use_keyed_state(("minimap-state", self.input.entity_id()), cx, |_, _| {
                Rc::new(Cell::new(MinimapState::default()))
            })
            .read(cx)
            .clone();
        let input = self.input;
        let input_for_layout = input.clone();
        div()
            .id("editor-minimap")
            .debug_selector(|| "editor-minimap".to_owned())
            .h_full()
            .w(WIDTH)
            .flex_none()
            .overflow_hidden()
            .border_l_1()
            .border_color(cx.theme().border.opacity(0.45))
            .bg(cx.theme().editor_background())
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let state = input_for_layout.read(cx);
                        let layout = MinimapLayout::for_input(state, bounds.size.height, window);
                        let lines = preview_lines(state, layout, bounds.size.height, window, cx);
                        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                        let slider_bounds = Bounds::new(
                            bounds.origin + point(px(0.), layout.slider_top),
                            size(bounds.size.width, layout.slider_height),
                        );
                        let mut slider = div()
                            .id("editor-minimap-viewport")
                            .debug_selector(|| "editor-minimap-viewport".to_owned())
                            .w(slider_bounds.size.width)
                            .h(slider_bounds.size.height)
                            .cursor_ns_resize()
                            .into_any_element();
                        slider.prepaint_as_root(
                            slider_bounds.origin,
                            slider_bounds.size.map(AvailableSpace::Definite),
                            window,
                            cx,
                        );
                        (layout, lines, hitbox, slider_bounds, slider)
                    },
                    move |bounds,
                          (layout, lines, hitbox, slider_bounds, mut slider),
                          window,
                          cx| {
                        let drag = interaction.get().drag.map(|mut drag| {
                            drag.rebase_for_layout(layout, window.mouse_position().y);
                            drag
                        });
                        let visual = MinimapVisualState::for_pointer(
                            hitbox.is_hovered(window),
                            slider_bounds.contains(&window.mouse_position()),
                            drag.is_some(),
                        );
                        interaction.set(MinimapState { drag, visual });
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            for (y, line) in lines {
                                let _ = line.paint(
                                    bounds.origin + point(LEFT_PADDING, y),
                                    LINE_HEIGHT,
                                    window,
                                    cx,
                                );
                            }
                            window.paint_quad(fill(
                                slider_bounds,
                                cx.theme().muted_foreground.opacity(visual.opacity()),
                            ));
                            slider.paint(window, cx);
                        });
                        window.on_mouse_event({
                            let input = input.clone();
                            let interaction = interaction.clone();
                            let hitbox = hitbox.clone();
                            move |event: &MouseDownEvent, phase, window, cx| {
                                if !phase.bubble()
                                    || event.button != MouseButton::Left
                                    || !hitbox.is_hovered(window)
                                {
                                    return;
                                }
                                let on_slider = slider_bounds.contains(&event.position);
                                let scroll_top = if on_slider {
                                    layout.scroll_top
                                } else {
                                    layout.scroll_top_for_click(event.position.y - bounds.top())
                                };
                                let drag = (on_slider && layout.slider_ratio > 0.).then_some(
                                    MinimapDrag {
                                        pointer_y: event.position.y,
                                        scroll_top,
                                        max_scroll_top: layout.max_scroll_top,
                                        slider_ratio: layout.slider_ratio,
                                    },
                                );
                                interaction.set(MinimapState {
                                    drag,
                                    ..interaction.get()
                                });
                                set_scroll_top(&input, scroll_top, cx);
                                cx.stop_propagation();
                            }
                        });
                        // Capture at window level, including movement/release outside the map.
                        window.on_mouse_event({
                            let input = input.clone();
                            let interaction = interaction.clone();
                            move |event: &MouseMoveEvent, phase, window, cx| {
                                if !phase.capture() {
                                    return;
                                }
                                let visual = MinimapVisualState::for_pointer(
                                    hitbox.is_hovered(window),
                                    slider_bounds.contains(&event.position),
                                    interaction.get().drag.is_some(),
                                );
                                if visual != interaction.get().visual {
                                    interaction.set(MinimapState {
                                        visual,
                                        ..interaction.get()
                                    });
                                    cx.notify(input.entity_id());
                                }
                                let Some(initial) = interaction.get().drag else {
                                    return;
                                };
                                if event.pressed_button != Some(MouseButton::Left) {
                                    interaction.set(MinimapState {
                                        drag: None,
                                        ..interaction.get()
                                    });
                                    cx.notify(input.entity_id());
                                    return;
                                }
                                set_scroll_top(&input, initial.scroll_top(event.position.y), cx);
                                cx.stop_propagation();
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase.capture() && event.button == MouseButton::Left {
                                if let Some(initial) = interaction.get().drag {
                                    interaction.set(MinimapState {
                                        drag: None,
                                        ..interaction.get()
                                    });
                                    set_scroll_top(
                                        &input,
                                        initial.scroll_top(event.position.y),
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlighter::{HighlightTheme, SyntaxHighlighter};
    use gpui::AppContext as _;

    struct ZoomHarness {
        tabs: Vec<(Entity<InputState>, Pixels)>,
        active: usize,
    }

    impl gpui::Render for ZoomHarness {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let (input, font_size) = &self.tabs[self.active];
            super::super::Input::new(input)
                .minimap(true)
                .text_size(*font_size)
                .size_full()
        }
    }

    fn draw_frame(cx: &mut gpui::VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
    }

    fn assert_current_frame(input: &Entity<InputState>, cx: &mut gpui::VisualTestContext) {
        let map = cx.debug_bounds("editor-minimap").unwrap();
        let slider = cx.debug_bounds("editor-minimap-viewport").unwrap();
        let expected = cx.update(|_, cx| {
            let state = input.read(cx);
            MinimapLayout::new(
                map.size.height,
                state.last_layout.as_ref().unwrap().line_height,
                state.input_bounds.size.height,
                state.scroll_size.height,
                -state.scroll_handle.offset().y,
            )
        });
        assert!(
            (slider.size.height - expected.slider_height).abs() <= px(1.),
            "painted height {:?}, current editor requires {:?}",
            slider.size.height,
            expected.slider_height
        );
        assert!(
            (slider.top() - map.top() - expected.slider_top).abs() <= px(1.),
            "painted top {:?}, current editor requires {:?}",
            slider.top() - map.top(),
            expected.slider_top
        );
    }

    #[gpui::test]
    fn minimap_uses_current_frame_metrics_through_zoom_and_tab_switches(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let tabs = [24., 8., 16.]
                .into_iter()
                .map(|font_size| {
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .code_editor("text")
                            .soft_wrap(false)
                            .default_value(format!("{}\n", "word ".repeat(30)).repeat(50_000))
                    });
                    (input, px(font_size))
                })
                .collect();
            ZoomHarness { tabs, active: 0 }
        });
        cx.simulate_resize(size(px(800.), px(420.)));
        cx.run_until_parked();
        // Each tab keeps its own zoom, wrap layout, and scroll position. Include first
        // activation and revisits, and never allow a catch-up frame before checking.
        for active in [0, 1, 2, 0, 1, 2] {
            harness.update(&mut cx.cx, |harness, cx| {
                harness.active = active;
                cx.notify();
            });
            draw_frame(cx);
            let input = cx.update(|_, cx| harness.read(cx).tabs[active].0.clone());
            assert_current_frame(&input, cx);
            for wrap in [false, true] {
                cx.update(|window, cx| {
                    input.update(cx, |input, cx| input.set_soft_wrap(wrap, window, cx));
                });
                draw_frame(cx);
                input.update(&mut cx.cx, |input, cx| {
                    let max_scroll = input.scroll_size.height - input.input_bounds.size.height;
                    input.update_scroll_offset(Some(point(px(0.), -max_scroll * 0.65)), cx);
                });
                draw_frame(cx);
                for font_size in [24., 8., 16., 17., 18., 19., 18., 17., 40., 8.] {
                    cx.update(|_, cx| {
                        input.update(cx, |input, _| {
                            assert!(input.preserve_zoom_anchor_at(input.input_bounds.center()));
                        });
                        harness.update(cx, |harness, cx| {
                            harness.tabs[active].1 = px(font_size);
                            cx.notify();
                        });
                    });
                    draw_frame(cx);
                    assert_current_frame(&input, cx);
                }
            }
        }
    }

    #[gpui::test]
    fn minimap_drag_rebases_when_zoom_changes_while_gripping(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (harness, cx) = cx.add_window_view(|window, cx| ZoomHarness {
            tabs: vec![(
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .code_editor("text")
                        .soft_wrap(false)
                        .default_value("long document line\n".repeat(50_000))
                }),
                px(8.),
            )],
            active: 0,
        });
        cx.simulate_resize(size(px(800.), px(420.)));
        cx.run_until_parked();
        let input = cx.update(|_, cx| harness.read(cx).tabs[0].0.clone());
        input.update(&mut cx.cx, |input, cx| {
            let max_scroll = input.scroll_size.height - input.input_bounds.size.height;
            input.update_scroll_offset(Some(point(px(0.), -max_scroll * 0.25)), cx);
        });
        draw_frame(cx);
        let slider = cx.debug_bounds("editor-minimap-viewport").unwrap();
        let grab = point(slider.center().x, slider.top() + px(3.));
        cx.simulate_mouse_down(grab, MouseButton::Left, gpui::Modifiers::none());
        let mut pointer = grab;
        for font_size in [40., 8., 24., 16.] {
            harness.update(&mut cx.cx, |harness, cx| {
                harness.tabs[0].1 = px(font_size);
                cx.notify();
            });
            draw_frame(cx);
            // This test isolates drag rebasing from the separate first-frame metric check.
            draw_frame(cx);
            let before = cx.debug_bounds("editor-minimap-viewport").unwrap();
            let scroll_before = cx.update(|_, cx| input.read(cx).scroll_offset());
            cx.simulate_mouse_move(pointer, MouseButton::Left, gpui::Modifiers::none());
            assert_eq!(
                cx.update(|_, cx| input.read(cx).scroll_offset()),
                scroll_before
            );
            for delta in [40., 40., -20., 0.] {
                let destination = point(pointer.x, pointer.y + px(delta));
                cx.simulate_mouse_move(destination, MouseButton::Left, gpui::Modifiers::none());
                draw_frame(cx);
                let moved = cx.debug_bounds("editor-minimap-viewport").unwrap();
                assert!(
                    (moved.top() - before.top() - px(delta)).abs() <= px(1.),
                    "font {font_size}, pointer moved {delta}, handle moved {:?}",
                    moved.top() - before.top()
                );
            }
            pointer.y += px(10.);
            cx.simulate_mouse_move(pointer, MouseButton::Left, gpui::Modifiers::none());
        }
        let before_release = cx.update(|_, cx| input.read(cx).scroll_offset());
        cx.simulate_mouse_up(pointer, MouseButton::Left, gpui::Modifiers::none());
        assert_eq!(
            cx.update(|_, cx| input.read(cx).scroll_offset()),
            before_release
        );
    }

    #[test]
    fn minimap_slider_and_preview_stay_aligned_across_the_full_document() {
        for content_height in [0., 200., 1_000., 200_000., 20_000_000.] {
            let max_scroll = (content_height - 400_f32).max(0.);
            for progress in [0., 0.01, 0.25, 0.5, 0.75, 0.99, 1.] {
                let layout = MinimapLayout::new(
                    px(400.),
                    px(20.),
                    px(400.),
                    px(content_height),
                    px(max_scroll * progress),
                );
                assert!(layout.slider_top >= px(0.));
                assert!(layout.slider_top + layout.slider_height <= px(400.));
                assert!(
                    (layout.scroll_top * layout.scale - layout.content_top - layout.slider_top)
                        .abs()
                        < px(0.5)
                );
                let rows = layout.visible_rows(px(400.), (content_height / 20.) as usize);
                assert!(rows.len() <= 135);
                if content_height > 400. {
                    assert!((layout.slider_height - px(60.)).abs() < px(0.001));
                    let initial = MinimapDrag {
                        pointer_y: px(75.),
                        scroll_top: layout.scroll_top,
                        max_scroll_top: layout.max_scroll_top,
                        slider_ratio: layout.slider_ratio,
                    };
                    assert_eq!(initial.scroll_top(px(75.)), layout.scroll_top);
                    assert!(
                        (initial.scroll_top(px(75.) - layout.slider_top) - px(0.)).abs() < px(1.)
                    );
                    let bottom =
                        px(75.) + (layout.max_scroll_top - layout.scroll_top) * layout.slider_ratio;
                    assert!((initial.scroll_top(bottom) - layout.max_scroll_top).abs() < px(2.));
                }
            }
        }
    }

    #[test]
    fn minimap_click_centers_the_previewed_location_including_bottom_padding() {
        let layout = MinimapLayout::new(px(400.), px(20.), px(400.), px(200_200.), px(100_000.));
        let click_y = px(300.);
        let clicked_document_y = (layout.content_top + click_y) / layout.scale;
        assert_eq!(
            layout.scroll_top_for_click(click_y) + px(200.),
            clicked_document_y
        );
        assert_eq!(layout.scroll_top_for_click(px(-1_000_000.)), px(0.));
        assert_eq!(
            layout.scroll_top_for_click(px(1_000_000.)),
            layout.max_scroll_top
        );
    }

    #[test]
    fn minimap_preview_preserves_indentation_unicode_and_syntax_colors() {
        const SOURCE: &str = "{\n\t\"café\": true,\n\t\"count\": 42\n}";
        let rope = ropey::Rope::from_str(SOURCE);
        let mut highlighter = SyntaxHighlighter::new("json");
        highlighter.update(None, &rope);
        let theme = HighlightTheme::default_dark();
        let start = 2;
        let line = "\t\"café\": true,";
        let styles = highlighter.styles(&(start..start + line.len()), &theme);
        let base = gpui::TextStyle::default().to_run(0);
        let (text, runs) = preview_text(line, start, &styles, &base, 4);
        assert_eq!(text, "    \"café\": true,");
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        let color_at = |needle: &str| {
            let target = text.find(needle).unwrap();
            let mut offset = 0;
            runs.iter()
                .find_map(|run| {
                    let contains = (offset..offset + run.len).contains(&target);
                    offset += run.len;
                    contains.then_some(run.color)
                })
                .unwrap()
        };
        assert_ne!(color_at("café"), color_at("true"));
        for needle in ["café", "true"] {
            let source_offset = SOURCE.find(needle).unwrap();
            let expected = styles
                .iter()
                .find(|(range, _)| range.contains(&source_offset))
                .unwrap()
                .1
                .color
                .unwrap_or(base.color);
            assert_eq!(color_at(needle), expected);
        }
        let (long, _) = preview_text(&"界".repeat(10_000), 0, &[], &base, 4);
        assert_eq!(long.chars().count(), MAX_LINE_CHARS);
        assert_eq!(preview_text("", 0, &[], &base, 4).0, "");
    }

    #[gpui::test]
    fn minimap_shapes_actual_wrapped_rows_and_keeps_blank_lines(cx: &mut gpui::TestAppContext) {
        struct Harness {
            input: Entity<InputState>,
        }
        impl gpui::Render for Harness {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                super::super::Input::new(&self.input)
                    .minimap(true)
                    .size_full()
            }
        }
        cx.update(crate::init);
        let source = format!("    {}\n\n尾行", "word ".repeat(70));
        let initial = source.clone();
        let (harness, cx) = cx.add_window_view(|window, cx| Harness {
            input: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("text")
                    .soft_wrap(true)
                    .default_value(initial)
            }),
        });
        cx.simulate_resize(size(px(400.), px(420.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let state = harness.read(cx).input.read(cx);
            assert!(state.text_wrapper.len() > state.text.lines_len());
            let layout = MinimapLayout::for_input(state, px(400.), window);
            let lines = preview_lines(state, layout, px(400.), window, cx);
            let actual: String = lines.iter().map(|(_, line)| line.text.as_ref()).collect();
            assert_eq!(actual, source.replace('\n', ""));
            assert!(
                lines
                    .windows(2)
                    .any(|pair| pair[1].0 - pair[0].0 == LINE_HEIGHT * 2.)
            );
            assert!(lines.iter().all(|(_, line)| line.font_size == FONT_SIZE));
        });
    }
}

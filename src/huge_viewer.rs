use std::{ops::Range, path::PathBuf};

use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, ScrollStrategy, StatefulInteractiveElement as _,
    Styled, Subscription, UniformListScrollHandle, Window, div, prelude::FluentBuilder as _,
    uniform_list,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::huge_file::{CancellationToken, HugeFile, LineIndex, MAX_COPY_BYTES, TextPage};

#[derive(Debug, Clone)]
pub enum HugeViewerEvent {
    EditRange { path: PathBuf, range: Range<u64> },
}

pub struct HugeFileView {
    file: HugeFile,
    page: TextPage,
    page_start_line: Option<u64>,
    selection: Option<Range<u64>>,
    history: Vec<u64>,
    line_index: Option<LineIndex>,
    index_cancel: CancellationToken,
    search_cancel: CancellationToken,
    navigation_cancel: CancellationToken,
    search_input: Entity<InputState>,
    goto_input: Entity<InputState>,
    last_search_query: Option<String>,
    last_match_end: Option<u64>,
    scroll_handle: UniformListScrollHandle,
    status: String,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HugeViewerEvent> for HugeFileView {}

impl HugeFileView {
    pub fn new_entity(
        file: HugeFile,
        window: &mut Window,
        cx: &mut App,
    ) -> anyhow::Result<Entity<Self>> {
        let page = file.read_page(0)?;
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in file"));
        let goto_input = cx.new(|cx| InputState::new(window, cx).placeholder("Line or b:byte"));
        let entity = cx.new(|cx| {
            let _subscriptions = vec![cx.subscribe(
                &search_input,
                |viewer: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        viewer.start_search(cx);
                    }
                },
            )];
            Self {
                file,
                page,
                page_start_line: Some(1),
                selection: None,
                history: Vec::new(),
                line_index: None,
                index_cancel: CancellationToken::default(),
                search_cancel: CancellationToken::default(),
                navigation_cancel: CancellationToken::default(),
                search_input,
                goto_input,
                last_search_query: None,
                last_match_end: None,
                scroll_handle: UniformListScrollHandle::default(),
                status: "Indexing lines in the background…".to_owned(),
                _subscriptions,
            }
        });
        entity.update(cx, |viewer, cx| viewer.start_index(cx));
        Ok(entity)
    }

    pub fn path(&self) -> &std::path::Path {
        self.file.path()
    }

    pub fn visible_range(&self) -> Range<u64> {
        self.page.byte_range.clone()
    }

    pub fn line_count(&self) -> Option<u64> {
        self.line_index.as_ref().map(LineIndex::total_lines)
    }

    fn start_index(&mut self, cx: &mut Context<Self>) {
        self.index_cancel.cancel();
        self.index_cancel = CancellationToken::default();
        let cancel = self.index_cancel.clone();
        let file = self.file.clone();
        let task = cx.background_spawn(async move { file.build_line_index(&cancel) });
        cx.spawn(async move |viewer, cx| {
            let result = task.await;
            let Some(viewer) = viewer.upgrade() else {
                return;
            };
            viewer
                .update(cx, |viewer, cx| match result {
                    Ok(Some(index)) => {
                        if viewer.page.byte_range.start == 0 {
                            viewer.page_start_line = Some(1);
                        }
                        viewer.status = format!("{} lines indexed", index.total_lines());
                        viewer.line_index = Some(index);
                        cx.notify();
                    }
                    Ok(None) => {}
                    Err(error) => {
                        viewer.status = error.to_string();
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }

    fn load_offset(&mut self, offset: u64, remember: bool, cx: &mut Context<Self>) {
        match self.file.read_page(offset) {
            Ok(page) => {
                if remember && page.byte_range.start != self.page.byte_range.start {
                    self.history.push(self.page.byte_range.start);
                }
                self.page_start_line = (page.byte_range.start == 0).then_some(1);
                self.page = page;
                self.selection = None;
                self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
            }
        }
    }

    fn next_page(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let next = self.page.byte_range.end;
        if next < self.file.len() {
            self.load_offset(next, true, cx);
        }
    }

    fn previous_page(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.history.pop().unwrap_or(0);
        self.load_offset(offset, false, cx);
    }

    fn copy_page(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let range = self
            .selection
            .clone()
            .unwrap_or_else(|| self.page.byte_range.clone());
        match self.file.read_utf8_range(range, MAX_COPY_BYTES) {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.status = "Visible page copied".to_owned();
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }

    fn edit_page(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(HugeViewerEvent::EditRange {
            path: self.file.path().to_path_buf(),
            range: self
                .selection
                .clone()
                .unwrap_or_else(|| self.page.byte_range.clone()),
        });
    }

    fn select_line(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(line) = self.page.lines.get(index) else {
            return;
        };
        self.selection = Some(match self.selection.take() {
            Some(selection) => {
                selection.start.min(line.byte_range.start)..selection.end.max(line.byte_range.end)
            }
            None => line.byte_range.clone(),
        });
        self.status = "Click another line to extend the selection".to_owned();
        cx.notify();
    }

    fn start_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().trim().to_owned();
        if query.is_empty() {
            self.status = "Enter a search term".to_owned();
            cx.notify();
            return;
        }

        self.search_cancel.cancel();
        self.search_cancel = CancellationToken::default();
        let cancel = self.search_cancel.clone();
        let file = self.file.clone();
        let start = if self.last_search_query.as_deref() == Some(query.as_str()) {
            self.last_match_end.unwrap_or(self.page.byte_range.start)
        } else {
            self.page.byte_range.start
        };
        self.last_search_query = Some(query.clone());
        self.status = format!("Searching for “{query}”…");
        cx.notify();
        let task = cx.background_spawn(async move {
            let mut first = None;
            let summary = file.stream_find(&query, start, &cancel, |item| {
                first = Some(item.byte_range);
                false
            });
            (first, summary)
        });
        cx.spawn(async move |viewer, cx| {
            let (found, summary) = task.await;
            let Some(viewer) = viewer.upgrade() else {
                return;
            };
            viewer
                .update(cx, |viewer, cx| {
                    match (found, summary) {
                        (Some(range), Ok(_)) => {
                            viewer.last_match_end = Some(range.end);
                            viewer.load_offset(range.start, true, cx);
                            viewer.status = format!("Match at byte {}", range.start);
                        }
                        (None, Ok(summary)) if !summary.completed => {
                            viewer.status = "Search cancelled".to_owned();
                        }
                        (None, Ok(_)) => {
                            viewer.last_match_end = None;
                            viewer.status = "No further match".to_owned();
                        }
                        (_, Err(error)) => viewer.status = error.to_string(),
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    fn on_search(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.start_search(cx);
    }

    fn on_goto(&mut self, _: &gpui::ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let value = self.goto_input.read(cx).value();
        let value = value.trim();
        if let Some(byte) = value
            .strip_prefix("b:")
            .or_else(|| value.strip_prefix("byte:"))
            .and_then(|value| value.trim().parse::<u64>().ok())
        {
            self.load_offset(byte.min(self.file.len()), true, cx);
            return;
        }

        let Some(line) = value
            .strip_prefix("line:")
            .unwrap_or(value)
            .trim()
            .parse::<u64>()
            .ok()
        else {
            self.status = "Use a line number or b:byte".to_owned();
            cx.notify();
            return;
        };
        let Some(index) = self.line_index.clone() else {
            self.status = "Line index is still building".to_owned();
            cx.notify();
            return;
        };

        let file = self.file.clone();
        self.navigation_cancel.cancel();
        self.navigation_cancel = CancellationToken::default();
        let cancel = self.navigation_cancel.clone();
        self.status = format!("Going to line {line}…");
        cx.notify();
        let task = cx
            .background_spawn(async move { file.byte_for_line_cancellable(&index, line, &cancel) });
        cx.spawn(async move |viewer, cx| {
            let result = task.await;
            let Some(viewer) = viewer.upgrade() else {
                return;
            };
            viewer
                .update(cx, |viewer, cx| {
                    match result {
                        Ok(Some(byte)) => viewer.load_offset(byte, true, cx),
                        Ok(None) => viewer.status = format!("Line {line} is outside the file"),
                        Err(error) => viewer.status = error.to_string(),
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }
}

impl Drop for HugeFileView {
    fn drop(&mut self) {
        self.index_cancel.cancel();
        self.search_cancel.cancel();
        self.navigation_cancel.cancel();
    }
}

impl Render for HugeFileView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let first_line = self.page_start_line;
        let byte_start = self.page.byte_range.start;
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .h_10()
                    .px_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("huge-previous")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label("Previous")
                            .disabled(self.history.is_empty())
                            .on_click(cx.listener(Self::previous_page)),
                    )
                    .child(
                        Button::new("huge-next")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowRight)
                            .label("Next")
                            .disabled(self.page.byte_range.end >= self.file.len())
                            .on_click(cx.listener(Self::next_page)),
                    )
                    .child(Input::new(&self.search_input).small().w_48())
                    .child(
                        Button::new("huge-search")
                            .small()
                            .label("Find")
                            .on_click(cx.listener(Self::on_search)),
                    )
                    .child(Input::new(&self.goto_input).small().w_32())
                    .child(
                        Button::new("huge-goto")
                            .small()
                            .label("Go")
                            .on_click(cx.listener(Self::on_goto)),
                    )
                    .child(
                        Button::new("huge-copy")
                            .ghost()
                            .small()
                            .label("Copy Page")
                            .on_click(cx.listener(Self::copy_page)),
                    )
                    .child(
                        Button::new("huge-edit")
                            .ghost()
                            .small()
                            .label("Edit Page")
                            .on_click(cx.listener(Self::edit_page)),
                    ),
            )
            .child(
                uniform_list(
                    "huge-file-lines",
                    self.page.lines.len(),
                    cx.processor(move |viewer, range: Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                let line = &viewer.page.lines[index];
                                let selected = viewer.selection.as_ref().is_some_and(|selection| {
                                    selection.start < line.byte_range.end
                                        && selection.end > line.byte_range.start
                                });
                                let label = first_line
                                    .map(|first| format!("{:>8}", first + index as u64))
                                    .unwrap_or_else(|| format!("b{:>7}", line.byte_range.start));
                                h_flex()
                                    .id(("huge-line", index))
                                    .h_6()
                                    .min_w_full()
                                    .cursor_pointer()
                                    .whitespace_nowrap()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(cx.theme().mono_font_size)
                                    .when(selected, |row| {
                                        row.bg(cx.theme().selection.opacity(0.45))
                                    })
                                    .on_click(cx.listener(move |viewer, _, _, cx| {
                                        viewer.select_line(index, cx)
                                    }))
                                    .child(
                                        div()
                                            .w_20()
                                            .px_2()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(label),
                                    )
                                    .child(div().px_2().child(line.text.clone()))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.scroll_handle.clone())
                .flex_1()
                .min_h_0(),
            )
            .child(
                h_flex()
                    .h_7()
                    .px_3()
                    .justify_between()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.status.clone())
                    .child(format!(
                        "Bytes {}–{} of {}",
                        byte_start,
                        self.page.byte_range.end,
                        self.file.len()
                    )),
            )
    }
}

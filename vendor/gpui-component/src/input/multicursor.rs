use ropey::Rope;
use sum_tree::Bias;

use super::{RopeExt as _, Selection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaggedSelection {
    pub range: Selection,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlannedReplacement {
    pub range: std::ops::Range<usize>,
    pub value: String,
    pub resulting_selection: TaggedSelection,
}

/// Return selections in document order, removing duplicates and coalescing overlaps.
pub(super) fn normalized_selections(
    primary: Selection,
    secondary: &[Selection],
) -> Vec<TaggedSelection> {
    let mut selections = secondary
        .iter()
        .copied()
        .map(|range| TaggedSelection {
            range,
            primary: false,
        })
        .chain(std::iter::once(TaggedSelection {
            range: primary,
            primary: true,
        }))
        .collect::<Vec<_>>();
    selections.sort_by_key(|selection| (selection.range.start, selection.range.end));

    let mut normalized: Vec<TaggedSelection> = Vec::with_capacity(selections.len());
    for selection in selections {
        let Some(previous) = normalized.last_mut() else {
            normalized.push(selection);
            continue;
        };
        let duplicate_caret = previous.range.is_empty()
            && selection.range.is_empty()
            && previous.range.start == selection.range.start;
        let overlaps = match (previous.range.is_empty(), selection.range.is_empty()) {
            (false, false) => selection.range.start < previous.range.end,
            (false, true) => previous.range.contains(selection.range.start),
            (true, false) => selection.range.contains(previous.range.start),
            (true, true) => false,
        };
        if duplicate_caret || overlaps {
            previous.range.start = previous.range.start.min(selection.range.start);
            previous.range.end = previous.range.end.max(selection.range.end);
            previous.primary |= selection.primary;
        } else {
            normalized.push(selection);
        }
    }
    normalized
}

/// Adjust a document-ordered selection set as earlier replacements shift later offsets.
pub(super) fn replacement_plan(
    selections: &[TaggedSelection],
    replacement_values: &[String],
) -> Vec<PlannedReplacement> {
    let fallback = replacement_values.first().cloned().unwrap_or_default();
    let values = if replacement_values.len() == selections.len() {
        replacement_values.to_vec()
    } else {
        vec![fallback; selections.len()]
    };
    let mut shift = 0isize;
    selections
        .iter()
        .copied()
        .zip(values)
        .map(|(selection, value)| {
            let start = selection.range.start.saturating_add_signed(shift);
            let end = selection.range.end.saturating_add_signed(shift);
            let caret = start + value.len();
            shift += value.len() as isize - (end - start) as isize;
            PlannedReplacement {
                range: start..end,
                value,
                resulting_selection: TaggedSelection {
                    range: Selection::new(caret, caret),
                    primary: selection.primary,
                },
            }
        })
        .collect()
}

/// Build one selection per physical line between two byte offsets.
pub(super) fn rectangular_selections(
    text: &Rope,
    anchor: usize,
    current: usize,
) -> (Vec<Selection>, usize) {
    let anchor = anchor.min(text.len());
    let current = current.min(text.len());
    let anchor_point = text.offset_to_point(anchor);
    let current_point = text.offset_to_point(current);
    let anchor_row = anchor_point.row;
    let current_row = current_point.row;
    let anchor_column = anchor_point.column;
    let current_column = current_point.column;
    let start_row = anchor_row.min(current_row);
    let end_row = anchor_row.max(current_row);
    let start_column = anchor_column.min(current_column);
    let end_column = anchor_column.max(current_column);

    let mut ranges = Vec::with_capacity(end_row - start_row + 1);
    for row in start_row..=end_row {
        let line_start = text.line_start_offset(row);
        let line = text.slice_line(row);
        let mut content_len = line.len();
        if content_len > 0 && line.chars().last() == Some('\r') {
            content_len -= 1;
        }
        let start = text.clip_offset(line_start + start_column.min(content_len), Bias::Left);
        let end = text.clip_offset(line_start + end_column.min(content_len), Bias::Left);
        ranges.push(Selection::new(start, end));
    }

    (ranges, current_row - start_row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_orders_deduplicates_and_marks_the_primary() {
        let selections = normalized_selections(
            Selection::new(8, 10),
            &[
                Selection::new(2, 2),
                Selection::new(8, 10),
                Selection::new(4, 7),
                Selection::new(6, 9),
            ],
        );

        assert_eq!(selections.len(), 2);
        assert_eq!(selections[0].range, Selection::new(2, 2));
        assert_eq!(selections[1].range, Selection::new(4, 10));
        assert!(selections[1].primary);
    }

    #[test]
    fn replacement_plan_shifts_later_ranges_and_preserves_primary() {
        let selections = vec![
            TaggedSelection {
                range: Selection::new(1, 2),
                primary: false,
            },
            TaggedSelection {
                range: Selection::new(5, 7),
                primary: true,
            },
        ];
        let plan = replacement_plan(&selections, &["long".to_owned(), "x".to_owned()]);

        assert_eq!(plan[0].range, 1..2);
        assert_eq!(plan[0].resulting_selection.range, Selection::new(5, 5));
        assert_eq!(plan[1].range, 8..10);
        assert_eq!(plan[1].resulting_selection.range, Selection::new(9, 9));
        assert!(plan[1].resulting_selection.primary);
    }

    #[test]
    fn rectangle_clamps_short_lines_and_keeps_utf8_boundaries() {
        let text = Rope::from("ab世界\na\nabcdef\n");
        let anchor = text.line_start_offset(0) + 2;
        let current = text.line_start_offset(2) + 5;
        let (ranges, primary) = rectangular_selections(&text, anchor, current);

        assert_eq!(primary, 2);
        assert_eq!(ranges.len(), 3);
        assert_eq!(text.slice(ranges[0]).to_string(), "世");
        assert!(ranges[1].is_empty());
        assert_eq!(text.slice(ranges[2]).to_string(), "cde");
    }
}

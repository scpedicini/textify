# Multicursor architecture decision

Textify continues with GPUI after the Milestone 4 architecture gate. Upstream GPUI Component 0.5.1
stores one selection, so Textify's pinned fork adds a contained selection-set layer without leaking
fork-specific APIs into the workspace.

## Interaction model

- Command-click makes the clicked caret primary and retains existing selections as secondary.
- Option-drag creates one range per physical line. Short lines clamp to their end, and every range
  is clipped to UTF-8 boundaries.
- Typing, newline insertion, Backspace, Delete, cut, and paste replace all disjoint selections.
- Copy joins non-empty selections in document order with newlines.
- Paste distributes newline-separated clipboard items when their count matches the selection count;
  otherwise it repeats the complete clipboard text at every selection.
- Navigation without selection modifiers collapses back to one caret.

## Correctness boundaries

Selections are sorted, deduplicated, and coalesced when they overlap. Batch replacements are applied
in document order with shifted offsets, so Unicode and different replacement lengths remain valid.
All changes from one multicursor operation receive a distinct history version and undo together.

macOS has one active marked-text range. While there is no IME composition, committed text is
replicated normally. When composition starts, Textify intentionally collapses secondary selections
and keeps provisional marked text only at the primary selection. This avoids duplicating incomplete
phonetic input or presenting multiple candidate-window anchors.

The fork's headless GPUI interaction test covers disjoint replacement, distributed paste, one-step
undo, and the IME collapse policy. Pure tests cover normalization, UTF-8 rectangular selection, and
replacement offset shifts.

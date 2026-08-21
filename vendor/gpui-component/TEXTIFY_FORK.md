# Textify GPUI Component fork

This directory vendors `gpui-component` 0.5.1 from upstream commit
`0f0ab35233212f8f3277028995caf0c41e13ee6c` (`crates/ui`). It is intentionally small and
keeps the upstream Apache-2.0 license.

Textify's changes are limited to its editor requirements:

- `tree-sitter-languages` bundles only JSON and Markdown; the original complete set remains
  available as `tree-sitter-all-languages`.
- unknown and explicit `text` languages never initialize a Tree-sitter parser.
- editor builders expose an approximate undo-memory budget and a search-decoration limit.
- Command-scroll is passed to the host application for Textify's per-tab text zoom.
- text zoom keeps the document row under the pointer (or the caret row) anchored across
  font-size changes: `InputState::preserve_zoom_anchor_at` / `preserve_cursor_anchor` capture
  the anchor, and `prepaint` in `input/element.rs` applies it to the scroll offset *before*
  the visible line range is computed. That ordering is load-bearing — the visible range is
  derived from the scroll offset, so correcting the offset any later shapes lines for the
  stale position and the anchored frame paints a blank, flashing gap mid-document
  (`zoom_anchor_moves_the_scroll_offset_before_the_next_visible_range` in `input/state.rs`
  guards the contract).
- `InputState::scroll_offset` is public so the host application can assert editor scroll
  behavior (modal occlusion, zoom) in its own tests.
- selection sets provide contained multicursor editing, rectangular selection, and grouped undo.

## Upgrading upstream

Do not hand-copy upstream sources over this directory. The complete Textify delta against
pristine upstream is captured as a reapplyable diff in
`vendor/patches/gpui-component-textify.patch`, and
`scripts/upgrade-gpui-component.sh` automates the whole cycle:

```sh
scripts/upgrade-gpui-component.sh upgrade <new-upstream-sha>   # replace sources, reapply patch
# resolve any .rej files, then:
cargo test --workspace                                         # guard + full corpus
# update the pinned commit at the top of this file, then:
scripts/upgrade-gpui-component.sh regenerate-patch <new-upstream-sha>
```

The script normalizes upstream with `rustfmt --edition 2024` before diffing or applying, so
formatting noise never hides the real changes.

These changes are guarded by `tests/vendor_fork.rs` in the repository root: `cargo test` fails
with a pointer back to this document if a re-vendor drops any of them, so a lost fork change
cannot slip through CI unnoticed. Update that guard, the patch, and this document together.

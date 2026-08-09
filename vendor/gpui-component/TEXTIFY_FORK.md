# Textify GPUI Component fork

This directory vendors `gpui-component` 0.5.1 from upstream commit
`0f0ab35233212f8f3277028995caf0c41e13ee6c` (`crates/ui`). It is intentionally small and
keeps the upstream Apache-2.0 license.

Textify's changes are limited to its editor requirements:

- `tree-sitter-languages` bundles only JSON and Markdown; the original complete set remains
  available as `tree-sitter-all-languages`.
- unknown and explicit `text` languages never initialize a Tree-sitter parser.
- editor builders expose an approximate undo-memory budget and a search-decoration limit.

Upgrade by replacing the upstream sources, reapplying these contained changes, and running the
full Textify performance and interaction corpus before committing both the fork and lockfile.

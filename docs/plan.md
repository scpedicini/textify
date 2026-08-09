# Textify build plan

## Product direction

Textify should feel instant and trustworthy before it becomes broad. The first release is a
native text editor with IDE-quality ergonomics, not a partial clone of a mature IDE. Every later
subsystem—projects, language servers, Git, and extensions—must preserve fast launch, bounded
memory, and predictable file handling.

## Architecture

```text
Textify Application
├── Workspace (commands, active tab, window state)
├── Document[] (path, language, file mode, dirty revision)
│   └── EditorBackend (the GPUI Component boundary)
├── FilePolicy (normal / large / huge, parser budgets)
└── File I/O (UTF-8 loading and chunked atomic saving)
```

Each tab owns one editor entity. Switching tabs therefore retains selections, scroll position,
folds, search state, and undo history. Only `editor.rs` configures GPUI Component, keeping its
pre-1.0 API out of the rest of the application.

Dependencies are exact-version pinned and `Cargo.lock` is committed. The published 0.5.1
GPUI Component release exposes Markdown through its all-languages feature; Textify selects only
JSON and Markdown at runtime. Moving to a tested fork with per-language features is part of the
dependency-hardening milestone so the final binary bundles only those grammars.

## Milestone 1 — editor MVP

Status: implemented in the initial build.

- Native GPUI window with a focused dark theme, tab strip, editor surface, and status bar.
- New, Open, Save, Save As, Close, next-tab, and previous-tab commands.
- One rope-backed editor entity per tab with dirty-state and revision tracking.
- Markdown and JSON detection; unknown extensions always select plain text.
- 24 MiB JSON parser budget and parser-free large-file mode.
- UTF-8 validation and a clear error for unsupported encodings.
- Chunked same-directory atomic saves without flattening the rope.
- Guard against clearing dirty state when edits race an in-progress save.
- Confirmation before discarding an edited tab.
- Unit tests for language detection, thresholds, line-ending analysis, UTF-8 errors, and saves.

Exit criteria:

- `cargo fmt`, `cargo check`, tests, and Clippy pass.
- Open/edit/save/reopen works for plain text, JSON, and Markdown.
- Unknown extensions never initialize a syntax parser.

## Milestone 2 — reliability and performance corpus

Status: implemented and baselined on 2026-08-08.

- Add external-change watching with Reload, Keep Mine, and Compare choices.
- Restore the previous window and tabs asynchronously after first paint.
- Preserve file permissions during atomic replacement.
- Add configurable undo-memory and search-decoration budgets.
- Add benchmark fixtures from `research.md`: 1/25/100 MiB JSON, 200,000 lines, a 5 MiB line,
  Unicode graphemes, IME input, repeated undo, and 100 tabs.
- Record cold launch, first paint, open/save time, peak RSS, typing latency, and scroll frame time.
- Create a tested GPUI Component fork and pin both it and its compatible GPUI revision.
- Bundle only JSON and Markdown grammars.

Exit criteria:

- A 100 MiB JSON file opens with no parser and remains responsive.
- A 5 MiB line cannot accidentally enable wrapping or parsing.
- Save does not allocate another document-sized contiguous string.
- External edits cannot silently overwrite local edits.

## Milestone 3 — huge-file viewer

Status: implemented and bounded-memory smoke tested on 2026-08-08.

- Add a read-only memory-mapped/paged viewer for files at or above 512 MiB.
- Virtualize visible lines and expose streaming find, go-to-line/byte, and copy.
- Allow reopening a selected range as an editable temporary document.
- Keep huge-file code separate from the normal rope-backed editor.

Exit criteria:

- Opening a multi-gigabyte UTF-8 log has bounded resident memory.
- Search and navigation are cancellable and do not block the UI thread.

Implementation notes:

- Files are read with positional, fixed-size page buffers; their contents never enter the normal
  rope-backed editor unless the user explicitly chooses an editable range.
- A sparse line index, streaming search, and line navigation run on background executors with
  cancellation tokens.
- The UI renders at most 512 page lines through a virtualized uniform list. Copy and temporary
  edit ranges have explicit byte limits.

## Milestone 4 — multicursor architecture gate

Status: implemented in the pinned GPUI Component fork and interaction tested on 2026-08-08.

Before building IDE features, prototype the requirement most likely to force an editor fork:

- Command-click to add carets.
- Edit, copy, and paste disjoint selections.
- Rectangular selection.
- One-step undo for a multicursor edit.
- IME behavior with multiple selections.

Exit criteria: either upstream support passes the interaction suite, or the required changes are
small and contained in Textify's GPUI Component fork. If neither is true, revisit STTextView
before further coupling the application to the editor core.

Decision: continue with GPUI. The fork now contains the complete selection-set implementation,
rendering, mouse gestures, batch replacement planning, and undo grouping. The platform retains one
primary marked-text range; starting IME composition intentionally collapses secondary selections,
while ordinary committed text fans out to every selection. Textify's application/editor adapter
remains unchanged.

## Milestone 5 — IDE workflows

- Open Folder and a virtualized file explorer.
- Command palette and quick-open.
- Workspace search with cancellable streaming results.
- Settings and keymap files with live reload.
- Optional language-server support, starting with diagnostics and go-to-definition.
- Optional Git status decorations after launch and large-file budgets are stable.

These services must initialize lazily. None may delay the first window or parse/index files that
the user has not opened without an explicit workspace action.

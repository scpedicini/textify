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

Status: implemented and headless-native smoke tested on 2026-08-08.

- Open Folder and a virtualized file explorer.
- Command palette and quick-open.
- Workspace search with cancellable streaming results.
- Settings and keymap files with live reload.
- Optional language-server support, starting with diagnostics and go-to-definition.
- Optional Git status decorations after launch and large-file budgets are stable.

These services must initialize lazily. None may delay the first window or parse/index files that
the user has not opened without an explicit workspace action.

Implementation notes:

- Open Folder builds a capped project index on the background executor. The explorer and all
  palette/search result surfaces use virtualized lists.
- Quick-open scores the existing project index. Workspace search streams bounded, cancellable
  matches and skips binary or over-budget files.
- `settings.json` and `keymap.json` are created in Textify's data directory and watched for live
  reload. Runtime editor budgets are applied to existing tabs.
- Git porcelain status runs only after a folder is opened or explicitly refreshed.
- LSP is disabled by default. A configured server is spawned only for an open workspace; document
  synchronization is debounced and capped, diagnostics decorate the editor, and F12 requests a
  definition. The client implements standard Content-Length framing and common server requests.
- The restored session now includes the workspace root. None of these services is constructed in
  the first-window path; the headless native render test asserts that the initial shell has no
  project or language-server state.

## Milestone 6 — native editor polish and recovery

Status: implemented, regression tested, and committed on 2026-08-08.

- A native settings panel (`Command-,`) controls font, default size, independent Untitled/named-file
  recovery policies, and the local recovery directory.
- Atomic revisioned snapshots continuously protect eligible dirty buffers. Session version 2
  restores their content, tab order, active tab, per-tab zoom, and word-wrap state. Graceful quit
  waits for a final snapshot and session manifest.
- Command-T opens a tab. Command-scroll changes only that tab's text size, with bounded trackpad
  accumulation and safe size limits.
- Untitled labels follow a bounded, normalized prefix of the first line. Saved filenames and
  explicit labels remain authoritative.
- OS file drops use the same asynchronous UTF-8 and large/huge-file policy as every other open path.
- Command-Shift-P provides ranked natural-language command matching over document and IDE actions.
- Textify installs native macOS application/File/Edit/View/Window menus. Word wrap is per-tab and
  cannot override large-file safety.
- Dirty state is visible in tabs, the native window title, and the status bar.
- Textify uses GPUI's null HTTP client and has no telemetry, analytics, crash uploads, or update
  checker. A source/dependency regression test enforces the offline application boundary; optional
  local language servers retain their own separate privacy responsibility.

Exit criteria:

- Every item in `TODO.md` is checked and backed by a focused test.
- At Milestone 6 completion, the workspace suite passed 52 Textify tests and 111 pinned-fork tests.
- Formatting, all-target compilation, and Clippy with warnings denied are clean.
- Native shell, settings, menu, wrap, palette, zoom, and recovery tests run headlessly.

## Milestone 7 — interaction hardening

Status: implemented and regression tested on 2026-08-08.

- The tab ribbon scrolls horizontally, reveals every newly activated tab, exposes an all-tabs
  chevron, and adds a fuzzy/wildcard Open Tabs switcher on Command-Option-P.
- Command overlays dismiss from their focused input with Escape or from the backdrop and restore
  editor focus.
- The GPUI Component fork reflows a newly wrapped document against the exact painted text width.
- Command-scroll records and preserves the painted caret position across the next font layout.
- Settings uses a searchable installed-font picker instead of accepting arbitrary text.
- A persisted View command hides the complete Textify title bar; a separate persisted Settings
  switch hides only the tagline.

## Milestone 8 — close safety, navigation, and local history

Status: implemented and regression tested on 2026-08-08.

- Command-W and tab close buttons share a three-choice dirty-document flow. Save As cancellation,
  write errors, and edits racing a save all keep the tab open; recovery data is discarded only when
  the document actually closes.
- The pinned component root mounts its recorded modal layer. An interaction test performs Command-T,
  types into an untitled tab, invokes Command-W, verifies the rendered dialog, and exercises Cancel,
  Save/Save As cancellation, and Don't Save.
- The Open Tabs overlay is a focused, virtualized, ten-row navigator with live fuzzy/wildcard
  filtering, visible keyboard selection, Enter activation, and automatic tab-ribbon reveal.
- Open Recent stores a serialized, newest-first, duplicate-free local path list. Settings controls
  its enable state and 1–100 item limit; writes are serialized so rapid opens cannot overwrite newer
  history with an older snapshot.
- GPUI Component's generic Input border is disabled only at Textify's code-editor adapter boundary.
- Command-P and the magnifying glass now search cancellable snapshots of every open editable rope,
  including unsaved buffers. Exact phrases outrank all-word line matches, and accepting a result
  selects its precise location after activating the tab.
- Hidden custom-title mode reserves the same 80-pixel macOS leading area used by the visible title
  row, preventing native traffic lights from covering toolbar controls.
- Legacy `quick_open_results` and `quick_open` configuration keys deserialize as aliases for
  `open_tab_search_results` and `search_open_tabs`.

## Feature-complete verification

Status: all planned milestones implemented, tested, documented, and committed on 2026-08-08.

- 74 Textify tests and 113 pinned-fork tests pass.
- All targets compile; formatting and Clippy with warnings denied are clean.
- The final optimized performance corpus remains within the Milestone 2 baseline envelope.
- The 512 MiB paged-viewer smoke test remains bounded at 63.4 MiB RSS.
- Native UI construction, project explorer virtualization, settings, menus, command overlay,
  per-tab wrap/zoom, and recovery behavior pass in headless GPUI tests, so verification does not
  create duplicate on-screen application instances.

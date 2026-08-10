# Textify IDE

Textify is a fast, focused native text editor for macOS, built in Rust with GPUI and
GPUI Component.

Textify provides safe file handling, CSS, HTML, Markdown, JSON, and shell highlighting, multiple tabs,
external-change detection, session restore, multicursor editing, measured large-file behavior, and
a bounded-memory viewer for files at or above 512 MiB. Its IDE layer adds a virtualized project
explorer, live search across open tabs, a command palette, cancellable workspace search,
configurable recent-file history, live settings/keymap reload, lazy Git decorations, optional
language-server diagnostics and go-to-definition, native macOS menus, crash recovery, per-tab zoom
and wrapping, searchable open-tab navigation, natural-language commands, and file drag-and-drop.
Overflowed tabs scroll horizontally and the active tab is always revealed.

All milestones in the current build plan are implemented. The feature-complete verification record
and optimized measurements are in [docs/plan.md](docs/plan.md) and
[docs/performance.md](docs/performance.md).

## Huge files

Files at or above 512 MiB open in a separate read-only paged viewer. Its toolbar supports page
navigation, streaming find, go-to-line or `b:byte`, copying a page or selected lines, and reopening
the visible/selected range as a temporary editable tab. The sparse line index, search, and line
navigation run in cancellable background tasks; the complete file is never loaded into the editor
rope.

## Multiple cursors

- Command-click adds carets.
- Option-drag creates a rectangular selection across physical lines.
- Typing, Enter, Backspace, Delete, cut, copy, and paste operate on disjoint selections.
- A clipboard with one line per selection distributes those lines; other clipboard text is repeated
  at every selection.
- A multicursor edit is one undo step. Starting an IME composition keeps the primary selection and
  intentionally collapses secondary selections because macOS exposes one marked-text range.

## IDE workflows

Open a folder with Command-Shift-O. Textify indexes it in the background, restores it with the next
session, and shows the virtualized explorer. Use File → Close Folder or the explorer's close button
when that workspace is no longer needed; open tabs stay open. `target`, `.git`, `node_modules`,
symlinks, unsupported binary/media files, and over-budget entries are excluded. Command-P searches
the live contents of every editable tab, including unsaved drafts; Command-Shift-F streams
folder-wide workspace matches without blocking
the UI; Command-Shift-P opens the command palette. Command-Option-P lists every open tab, filters as
you type, and activates and reveals the chosen tab. File → Open Recent provides a separately bounded
local history that can be disabled or cleared in Settings.

Textify creates `settings.json` and `keymap.json` under `~/Library/Application Support/Textify` (or
`TEXTIFY_DATA_DIR`) after first paint. Both files reload when saved. Git decorations are lazy and can
be disabled. Language-server support is opt-in; configure an executable and extensions before using
F12 for go-to-definition. See [docs/ide-workflows.md](docs/ide-workflows.md) for the full schema.
Textify installs no application network client or telemetry; see [docs/privacy.md](docs/privacy.md)
for local storage and the explicit optional-LSP boundary.

Open File, Open Folder, and Save As begin in the active saved tab's directory; an untitled tab falls
back to the open workspace. Textify automatically opens valid UTF-8 and recognizes text-like CP437
DOS files. The clickable UTF-8 / CP437 status control can reopen a clean saved file with an explicit
decoder. Saves retain the tab's encoding and fail safely if a CP437 edit contains an unrepresentable
character.

## Run it

The project currently targets Apple silicon macOS.

```sh
cargo run
```

Textify enables GPUI's `runtime_shaders` feature so development builds work with either full
Xcode or Apple's standalone Command Line Tools. Release packaging should use full Xcode and
precompile Metal shaders.

For the optimized local app, terminal symlink, and Raycast command, run:

```sh
./scripts/install-local.sh
```

Later `cargo build --release --bin textify` builds update the installed symlink targets in place.
See [docs/deployment.md](docs/deployment.md) for the one-time Raycast setup and packaging details.

## Verify it

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --all-targets -- -D warnings
```

Run the generated large-file performance corpus with:

```sh
cargo run --release --bin textify-perf
```

The current measurements and methodology are in [docs/performance.md](docs/performance.md).

## Keyboard shortcuts

| Command | Shortcut |
| --- | --- |
| New file | Command-N |
| New tab | Command-T |
| Open file | Command-O |
| Save | Command-S |
| Save As | Command-Shift-S |
| Close tab | Command-W |
| Next tab | Control-Tab |
| Previous tab | Control-Shift-Tab |
| Search open tabs | Command-Option-P |
| Open folder | Command-Shift-O |
| Close folder | File menu, palette, or explorer close button |
| Toggle explorer | Command-B |
| Command palette | Command-Shift-P |
| Settings | Command-, |
| Toggle word wrap | Option-Z |
| Zoom active tab | Command-Scroll |
| Search text across open tabs | Command-P |
| Workspace search | Command-Shift-F |
| Go to definition | F12 |

The tab ribbon accepts wheel/trackpad scrolling and its chevron lists every open tab. Right-click a
saved-file tab to copy its full path or reveal the file in Finder. Click the status bar's WRAP / NO
WRAP control to toggle wrapping for the active tab. Click the language label (for example `JSON`)
or use the command palette to toggle syntax highlighting independently for the active tab. The status bar
progressively hides secondary details and truncates long paths as the window narrows. The View menu
can hide the Textify title bar or the editor's line-number gutter; Settings persists both choices,
can independently hide the tagline, and provides a
searchable dropdown of installed editor fonts plus explicit tab-width and tabs-versus-spaces
controls. Dirty tabs close through Save / Don't Save / Cancel; the editor surface is frameless,
and hiding the title row reserves the native macOS window-control area.

See [docs/plan.md](docs/plan.md) for the build plan and [docs/research.md](docs/research.md) for
the technology evaluation.

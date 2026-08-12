# Textify IDE

Textify is a fast, focused native text editor for macOS, Linux, and Windows, built in Rust with GPUI
and GPUI Component.

Textify is an open-source project licensed under MIT. You do not need Rust or a development setup
to use it: ready-to-run Linux x64, Windows x64, and universal macOS builds are published on the
[GitHub Releases page](https://github.com/scpedicini/textify/releases) whenever a production update
is pushed. Each release includes SHA-256 checksums. The current builds are unsigned, so macOS
Gatekeeper or Windows SmartScreen may show a warning after download.

Textify provides safe file handling, CSS, HTML, Markdown, JSON, and shell highlighting, multiple tabs,
external-change detection, session restore, multicursor editing, measured large-file behavior, and
a bounded-memory viewer for files at or above 512 MiB. Its IDE layer adds a virtualized project
explorer, live search across open tabs, a command palette, cancellable workspace search,
configurable recent-file history, live settings/keymap reload, lazy Git decorations, optional
language-server diagnostics and go-to-definition, native menus, crash recovery, per-tab zoom
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

- Command-click on macOS or Control-click on Windows/Linux adds carets.
- Option-drag creates a rectangular selection across physical lines.
- Typing, Enter, Backspace, Delete, cut, copy, and paste operate on disjoint selections.
- Command-X on macOS or Control-X on Windows/Linux with no selected text cuts the complete logical
  row under each caret.
- A clipboard with one line per selection distributes those lines; other clipboard text is repeated
  at every selection.
- A multicursor edit is one undo step. Starting an IME composition keeps the primary selection and
  intentionally collapses secondary selections because macOS exposes one marked-text range.

## IDE workflows

Open a folder with Command-Shift-O on macOS or Control-Shift-O elsewhere. Textify indexes it in the
background, restores it with the next session, and shows the virtualized explorer. Use File → Close
Folder or the explorer's close button
when that workspace is no longer needed; open tabs stay open. `target`, `.git`, `node_modules`,
symlinks, unsupported binary/media files, and over-budget entries are excluded. Command-P on macOS
or Control-P elsewhere searches the live contents of every editable tab, including unsaved drafts;
Command-Shift-F or Control-Shift-F streams
folder-wide workspace matches without blocking
the UI; Command-Shift-P or Control-Shift-P opens the command palette. Command-Option-P or
Control-Alt-P lists every open tab, filters as
you type, and activates and reveals the chosen tab. File → Open Recent provides a separately bounded
local history that can be disabled or cleared in Settings.

Textify creates `settings.json` and `keymap.json` in the operating system's application-data
directory (or `TEXTIFY_DATA_DIR`) after first paint. Both files reload when saved. Git decorations are lazy and can
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

Rust 1.93.1 is pinned for reproducible builds. GPUI renders through native Metal on macOS, Vulkan on
Linux, and Direct3D on Windows.

```sh
cargo run
```

Textify enables GPUI's `runtime_shaders` feature. Linux builds need the Wayland/X11, Fontconfig, and
Vulkan development libraries listed in `scripts/install-linux-build-deps.sh`; Windows builds need
MSVC and the Windows SDK.

For the optimized macOS local app and terminal symlink, run:

```sh
./scripts/install-local.sh
```

Later `cargo build --release --bin textify` builds update the installed symlink targets in place.
Quit a running Textify process and launch it again from Raycast to load the rebuilt executable.
Raycast can launch the indexed `Textify.app` directly; no separate Script Command or Raycast folder
is needed. Every push to `prod` builds, tests, and publishes native Linux x64, Windows x64, and
universal macOS artifacts. See [docs/deployment.md](docs/deployment.md) for local packaging and the
complete GitHub Releases flow.

## Verify it

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-targets -- --test-threads=1
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Run the generated large-file performance corpus with:

```sh
cargo run --release --bin textify-perf
```

The current measurements and methodology are in [docs/performance.md](docs/performance.md).

## Keyboard shortcuts

| Command | Shortcut |
| --- | --- |
| New file | Command-N / Control-N |
| New tab | Command-T / Control-T |
| Open file | Command-O / Control-O |
| Save | Command-S / Control-S |
| Save As | Command-Shift-S / Control-Shift-S |
| Close tab | Command-W / Control-W |
| Next tab | Control-Tab |
| Previous tab | Control-Shift-Tab |
| Search open tabs | Command-Option-P / Control-Alt-P |
| Open folder | Command-Shift-O / Control-Shift-O |
| Close folder | File menu, palette, or explorer close button |
| Toggle explorer | Command-B / Control-B |
| Command palette | Command-Shift-P / Control-Shift-P |
| Settings | Command-, / Control-, |
| Toggle word wrap | Option-Z / Alt-Z |
| Zoom active tab | Command-Scroll / Control-Scroll |
| Search text across open tabs | Command-P / Control-P |
| Workspace search | Command-Shift-F / Control-Shift-F |
| Go to definition | F12 |

The tab ribbon accepts wheel/trackpad scrolling and its chevron lists every open tab. Right-click a
saved-file tab to copy its full path or reveal the file in Finder, File Explorer, or the Linux file
manager. Click the status bar's WRAP / NO
WRAP control to toggle wrapping for the active tab. Click the language label (for example `JSON`)
or use the command palette to toggle syntax highlighting independently for the active tab. The status bar
progressively hides secondary details and truncates long paths as the window narrows. The View menu
can hide the Textify title bar or the editor's line-number gutter; Settings persists both choices,
can independently hide the tagline, and provides a
searchable dropdown of installed editor fonts plus explicit tab-width and tabs-versus-spaces
controls. Dirty tabs close through Save / Don't Save / Cancel; the editor surface is frameless,
and hiding the title row reserves native window-control space where required.

See [docs/plan.md](docs/plan.md) for the build plan and [docs/research.md](docs/research.md) for
the technology evaluation.

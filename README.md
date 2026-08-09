# Textify IDE

Textify is a fast, focused native text editor for macOS, built in Rust with GPUI and
GPUI Component.

Textify currently provides safe file handling, Markdown and JSON highlighting, multiple tabs,
external-change detection, session restore, measured large-file behavior, and a bounded-memory
viewer for files at or above 512 MiB. Project, command, search, keymap, Git, and language-server
workflows are tracked in the remaining milestones.

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

## Run it

The project currently targets Apple silicon macOS.

```sh
cargo run
```

Textify enables GPUI's `runtime_shaders` feature so development builds work with either full
Xcode or Apple's standalone Command Line Tools. Release packaging should use full Xcode and
precompile Metal shaders.

## Verify it

```sh
cargo fmt --all -- --check
cargo test --all-targets
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
| Open file | Command-O |
| Save | Command-S |
| Save As | Command-Shift-S |
| Close tab | Command-W |
| Next tab | Control-Tab |
| Previous tab | Control-Shift-Tab |

See [docs/plan.md](docs/plan.md) for the build plan and [docs/research.md](docs/research.md) for
the technology evaluation.

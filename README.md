# Textify IDE

Textify is a fast, focused native text editor for macOS, built in Rust with GPUI and
GPUI Component.

The first milestone is deliberately narrow: excellent text editing, safe file handling,
Markdown and JSON highlighting, multiple tabs, and explicit behavior for large files. Project
indexing, extensions, Git integration, and LSP features come after the editor foundation has a
measured performance baseline.

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

## Recommendation

Build it in **Rust using GPUI + GPUI Component**, with GPUI Component pinned to your own tested fork.

This is currently the closest match to a reusable, modern Rust code-editor foundation rather than merely a multiline text box. Its editor already includes a rope-backed document model, cursor and selection handling, undo history, display mapping, line numbers, search, folding, Tree-sitter highlighting, diagnostics support, themes, tabs, and docking components. The repository is Apache-2.0 licensed, has roughly 12,000 GitHub stars and around 2,000 commits, and was actively receiving fixes in July 2026. ([Longbridge][1])

GPUI itself is Zed’s Rust UI framework. On macOS it renders through Metal, provides its own integrated async executor, and has both high-level declarative UI and lower-level primitives intended for specialized interfaces such as code editors. ([GitHub][2])

### The important qualification

There is no current Rust library that is simultaneously:

* As battle-tested as Sublime Text or Scintilla
* A clean drop-in editor widget
* Entirely modern Rust
* Highly starred
* API-stable
* Proven on arbitrarily huge files

**GPUI Component is the best balance, but not yet Sublime-level mature.** GPUI is explicitly pre-1.0 and frequently makes breaking changes. GPUI Component also recently fixed a bug where unrecognized plain-text files could accidentally use the JSON parser; opening a 500 MB file reportedly consumed around 5 GB and became unresponsive. The fix was merged on July 15, 2026. ([GitHub][2])

That means the correct architecture is:

> **Use GPUI Component, pin it, isolate it behind your own interface, and implement an explicit large-file mode.**

## Candidate comparison

| Foundation                  | Strength                                                                                                                                                                                                  | Principal drawback                                                                                                                 |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| **GPUI Component**          | Best reusable modern Rust editor component; attractive native UI; Ropey and Tree-sitter already integrated                                                                                                | Pre-1.0 ecosystem; large-file behavior still needs defensive engineering                                                           |
| **Fork Zed**                | Most mature and heavily exercised Rust GUI editor code; very active and roughly 88,000 stars                                                                                                              | Large codebase, tightly coupled internals, primarily GPL-licensed application code rather than a clean editor widget ([GitHub][3]) |
| **Lapce + Floem**           | Pure Rust, permissive licensing, established editing architecture, GPU-rendered GUI                                                                                                                       | More assembly work; editor internals are less cleanly packaged as one polished drop-in component ([GitHub][4])                     |
| **Helix core + custom GUI** | Excellent editing engine and multiple-selection model                                                                                                                                                     | You would still have to build most of the GUI editor surface, text layout, hit testing, IME integration, scrolling, and rendering  |
| **Scintilla**               | The genuinely battle-hardened reusable editor component; supports Cocoa, syntax styling, folding, search, multiple selections, undo, and large documents; still maintained with an August 4, 2026 release | C++, older architecture, custom Rust/Objective-C bridge required, and less aligned with the memory-safe Rust goal ([Scintilla][5]) |
| **Swift + STTextView**      | Best native Swift route; TextKit 2, multi-cursor, line numbers, search, undo, and syntax plugin support                                                                                                   | Much smaller ecosystem and less complete as a syntax-aware editor foundation ([GitHub][6])                                         |
| **CodeEditSourceEditor**    | Swift, Tree-sitter-based and explicitly intended for code editing                                                                                                                                         | Its own documentation says it is not ready for production use ([GitHub][7])                                                        |

Strictly interpreted, **Scintilla wins “battle-hardened component,” while GPUI Component wins “modern, attractive, Rust-dominant component.”** For your particular priorities, I would choose GPUI Component.

---

# Proposed stack

| Layer               | Choice                                                              |
| ------------------- | ------------------------------------------------------------------- |
| Language            | Rust 2024 edition                                                   |
| Target              | `aarch64-apple-darwin` initially                                    |
| GUI/rendering       | GPUI                                                                |
| Editor component    | GPUI Component `Input` / `InputState` code-editor mode              |
| Text storage        | The Ropey-based storage already used by GPUI Component              |
| Syntax highlighting | Tree-sitter, initially only JSON and Markdown                       |
| Tabs                | Lightweight custom tab strip using GPUI Component primitives        |
| File watching       | `notify`                                                            |
| Settings/session    | `serde` + `serde_json`                                              |
| Logging/profiling   | `tracing`; Apple Instruments for CPU and allocation profiling       |
| Packaging           | Native `.app`, arm64-only initially                                 |
| Testing             | Rust unit tests plus GPUI interaction tests and large-file fixtures |

Do not introduce another buffer implementation alongside GPUI Component’s rope. Two competing document models would produce copying, synchronization problems, and confusing undo ownership.

## Dependency strategy

Do not follow floating `main` branches in a real application. GPUI is pre-1.0, and GPUI Component’s official material currently favors Git dependencies, so exact revision pinning matters. ([GitHub][2])

```toml
[package]
name = "my-editor"
version = "0.1.0"
edition = "2024"

[dependencies]
# These must use the same tested Zed revision.
gpui = {
    git = "https://github.com/zed-industries/zed",
    rev = "<tested-zed-commit>"
}

gpui_platform = {
    git = "https://github.com/zed-industries/zed",
    rev = "<tested-zed-commit>",
    features = ["font-kit"]
}

# Use your fork, pinned after the July 15 large-file fix.
gpui-component = {
    git = "https://github.com/<your-account>/gpui-component",
    rev = "<tested-component-commit>",
    features = ["tree-sitter-markdown"]
}

anyhow = "1"
notify = "7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
```

Commit `Cargo.lock`. Upgrade GPUI and GPUI Component deliberately, together, after running the editor test corpus.

I would maintain a very small fork rather than modifying upstream code throughout the application. Your fork should initially contain only:

1. Dependency revision pins.
2. Large-file/parser protections.
3. Any APIs needed for chunked loading and saving.
4. Eventually, multiple-selection support if upstream does not provide what you need.

---

# Application architecture

```text
EditorApplication
├── CommandRegistry
├── SettingsStore
├── SessionStore
└── Workspace
    ├── TabStrip
    ├── StatusBar
    └── DocumentStore
        ├── Document A
        │   ├── InputState
        │   ├── FileMetadata
        │   ├── Language
        │   ├── FileMode
        │   └── DirtyState
        └── Document B
            └── ...
```

A document would look approximately like:

```rust
struct Document {
    path: Option<PathBuf>,
    editor: Entity<InputState>,
    language: Language,
    mode: FileMode,
    encoding: TextEncoding,
    line_ending: LineEnding,
    dirty: bool,
    disk_revision: Option<FileRevision>,
}
```

Keep one `InputState` per open document so switching tabs preserves cursor position, scroll position, undo history, folding, and search state. Only render the active editor. Clean, inactive documents can eventually be evicted and reloaded if tab counts become unusually large.

Put GPUI Component behind your own adapter:

```rust
trait EditorBackend {
    fn insert_text(&mut self, text: &str);
    fn selected_text(&self) -> String;
    fn undo(&mut self);
    fn redo(&mut self);
    fn find(&mut self, query: &str);
    fn set_language(&mut self, language: Language);
}
```

The rest of your application should not directly manipulate dozens of GPUI Component types. This makes upstream API changes manageable and preserves the option to replace the component later.

---

# Large-file design

GPUI Component advertises stable editor behavior up to approximately 200,000 lines, but that is a project claim rather than a guarantee for pathological JSON, extremely long lines, or several-hundred-megabyte files. ([GitHub][8])

Use three modes. These are sensible initial thresholds to benchmark and tune, not inherent library limits.

### Normal mode

For approximately:

* Up to 64 MiB
* Up to 200,000 lines
* No exceptionally long lines

Enable:

* Tree-sitter
* Folding
* Word wrapping
* Full undo
* Search highlighting
* Diagnostics
* Markdown or JSON highlighting

For JSON specifically, I would stop full Tree-sitter parsing considerably earlier—perhaps around 16–32 MiB—because deeply nested or minified JSON can create a disproportionately large syntax tree.

### Large-file mode

Activate when any condition is met:

* File exceeds 64 MiB
* File exceeds 200,000 lines
* A line exceeds approximately 1 MiB
* The parser exceeds a time or memory budget

Disable:

* Tree-sitter
* Folding
* Word wrapping
* LSP or diagnostics
* Whitespace visualization
* Expensive search-result decoration
* Unbounded undo history

Keep:

* Editing
* Line numbers
* Basic find
* Go-to-line
* Save
* A clear `PLAIN TEXT — LARGE FILE MODE` status indicator

Most importantly, an unknown extension must map to **no parser**, never to a default JSON grammar. The recent 500 MB incident demonstrates why that must be enforced at your own application boundary. ([GitHub][9])

### Huge-file viewer

For something like 512 MiB or larger, use a separate read-only, paged or memory-mapped viewer initially. Arbitrary gigabyte-scale editing is a different problem from a normal rope-based editor.

A huge-file viewer can provide:

* Virtualized visible lines
* Streaming search
* Go-to-byte and go-to-line
* Copy selections
* Reopen a selected range as an editable temporary document

That prevents “open file” from meaning “decode, copy, rope-build, syntax-parse, and retain the whole thing several times.”

---

# Startup and memory rules

For Sublime-like responsiveness:

* Open the first window before restoring previous documents.
* Restore tabs asynchronously after first paint.
* Do not initialize every syntax grammar at startup.
* Bundle only JSON and Markdown grammars initially.
* Do not initialize a plugin host, project indexer, Git engine, or LSP system in version 1.
* Use GPUI’s executor rather than adding a full Tokio runtime unless networking later requires it.
* Read files away from the UI thread.
* Cancel stale syntax and search work when the document revision changes.
* Avoid converting the complete rope to a contiguous `String` on every save.
* Save through rope chunks to a temporary file in the same directory, then atomically replace the original.
* Put a configurable byte budget on undo history.
* Watch for external file changes only after the document is loaded.
* Compile arm64-only for your M1 until you actually need an Intel-compatible Universal binary.

For aesthetics, GPUI Component already gives you theme infrastructure, syntax theme support, tab/dock primitives, dialogs, keyboard navigation, and native-inspired controls. Use a restrained custom theme and a simple tab bar rather than adopting its entire docking/panel system immediately. ([GitHub][10])

---

# One feature to validate immediately

The present GPUI Component editor state appears organized around a singular selection object. It is a substantial editor core, but I would not assume Sublime-style arbitrary multicursor editing is already complete. Prototype these before committing the architecture:

* Command-click to add carets
* Edit all selections
* Rectangular selection
* Undo a multicursor operation
* Copy and paste disjoint selections

The internal state currently includes the rope, display map, undo history, search state, diagnostics, highlights, and a singular `Selection`, so full multicursor may require an upstream contribution or a contained fork modification. ([GitHub][11])

STTextView already advertises multicursor support, so this is the one requirement that could materially favor the Swift route. ([GitHub][6])

---

# Initial performance corpus

Before adding features, continually test:

| Test file                            | Purpose                                  |
| ------------------------------------ | ---------------------------------------- |
| 1 MiB JSON                           | Ordinary syntax highlighting             |
| 25 MiB JSON                          | Parser and syntax-tree memory            |
| 100 MiB JSON                         | Confirm large-file mode prevents parsing |
| 100–500 MiB plain text               | Load, scroll, search, and save behavior  |
| 200,000 short lines                  | Line indexing and scrollbar behavior     |
| One 5 MiB line                       | Long-line rendering and wrapping         |
| 100 open tabs                        | Per-document overhead                    |
| Chinese, emoji, combining marks      | Grapheme movement and selection          |
| Chinese and Japanese IME composition | macOS input correctness                  |
| Repeated undo/redo                   | History memory behavior                  |
| External modification                | File watcher and conflict behavior       |

Measure cold launch, first window, open time, peak resident memory, typing latency, scroll frame time, tab-switch latency, search latency, and save-time memory duplication.

## Final stack decision

**Use Rust, GPUI, and a pinned fork of GPUI Component.** It gives you the best combination of native Metal rendering, a real existing editor core, attractive UI components, permissive licensing, active development, and direct control over the entire application.

Use **Scintilla** only if proven handling of very large files and decades of editor maturity outweigh the desire for a modern Rust-native architecture. Use **Swift + STTextView** only if first-class macOS behavior and multicursor support outweigh having the more comprehensive, higher-starred Rust component ecosystem.

[1]: https://longbridge.github.io/gpui-component/docs/components/editor "https://longbridge.github.io/gpui-component/docs/components/editor"
[2]: https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md?utm_source=chatgpt.com "zed/crates/gpui/README.md at main"
[3]: https://github.com/zed-industries/zed "https://github.com/zed-industries/zed"
[4]: https://github.com/lapce/lapce "https://github.com/lapce/lapce"
[5]: https://scintilla.org/ScintillaHistory.html?utm_source=chatgpt.com "History of Scintilla"
[6]: https://github.com/krzyzanowskim/STTextView "https://github.com/krzyzanowskim/STTextView"
[7]: https://github.com/CodeEditApp/CodeEditSourceEditor "https://github.com/CodeEditApp/CodeEditSourceEditor"
[8]: https://github.com/longbridge/gpui-component "https://github.com/longbridge/gpui-component"
[9]: https://github.com/longbridge/gpui-component/issues/2566 "https://github.com/longbridge/gpui-component/issues/2566"
[10]: https://github.com/longbridge/gpui-component/blob/main/CLAUDE.md?utm_source=chatgpt.com "CLAUDE.md - longbridge/gpui-component"
[11]: https://github.com/longbridge/gpui-component/blob/main/crates/ui/src/input/state.rs "gpui-component/crates/ui/src/input/state.rs at main · longbridge/gpui-component · GitHub"

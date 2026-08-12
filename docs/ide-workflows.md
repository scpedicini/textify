# IDE workflows and configuration

IDE services are lazy. The first window creates only an untitled editor and the file watcher. A
project index, Git process, workspace search, or language server cannot start until a folder is
opened or the corresponding command is invoked.

## Project explorer and search

Use **Open Folder** (Command-Shift-O on macOS or Control-Shift-O elsewhere) to build a bounded
background index. The sidebar renders its
flattened tree through a virtualized list; directory rows are always expanded in this initial IDE
workflow. It lists recognized text, configuration, and source files while omitting media, archives,
binary files, `.DS_Store`, and build/dependency directories. Refresh from the sidebar to rescan
files and Git state. File → Close Folder, the command palette, or the sidebar close button stops the
folder's index, Git, search, and language-server services without closing open document tabs. The
folder is persisted with the tab session until it is closed.

Search Open Tabs (Command-P on macOS or Control-P elsewhere, or the magnifying-glass button) searches
the live ropes of every editable tab, including unsaved documents. Exact phrases rank ahead of lines containing all query words;
results show document, line, column, and preview. Arrow keys move through the virtualized list and
Enter activates the tab, reveals it in the ribbon, selects the match, and focuses the editor.

Workspace Search (Command-Shift-F on macOS or Control-Shift-F elsewhere) remains the folder-wide
disk search. It scans indexed files on a background executor and streams matches into a virtualized result list. Starting another query
cancels the previous scan. Binary files and files over `search_max_file_bytes` are skipped; result
count is bounded by `search_max_matches`.

Command-Option-P on macOS or Control-Alt-P elsewhere opens the tab navigator with up to ten visible
rows. It lists every tab immediately, supports arrows across longer lists, filters with fuzzy text
or ordered `*` fragments, and opens the highlighted tab with Enter.

## File-dialog context and text encodings

Open File, Open Folder, and Save As use the active saved document's parent directory. When the
active tab is untitled, they fall back to the open workspace and then the process directory. A
normal Save writes directly to the document's existing path and therefore opens no dialog.

Valid UTF-8 always opens as UTF-8. When UTF-8 validation fails, Textify recognizes CP437 only when
the bytes look like text; data with NUL bytes or excessive control characters remains rejected as
binary. The encoding label in the status bar is a button. It opens a searchable, keyboard-driven
UTF-8 / CP437 list and reloads a clean saved document from disk using the chosen decoder. Textify
blocks this destructive reload for dirty tabs. The choice is stored in the session, used for
external reload and comparison, and retained by Save and Save As. CP437 saves stream through the
atomic writer; a character outside the CP437 repertoire produces an error without replacing the
file. The read-only huge-file viewer remains UTF-8-only.

Shell detection covers `.sh`, `.bash`, `.zsh`, `.bashrc`, `.zshrc`, and `.profile`; all use the
bundled Tree-sitter Bash grammar.

## Settings

Textify creates `settings.json` and `keymap.json` in its application-data directory after the first
paint. Set `TEXTIFY_DATA_DIR` to use an isolated directory. Saving either file reloads it; malformed
JSON leaves the previous configuration active and reports the error in the status bar.

Default `settings.json`:

```json
{
  "appearance": {
    "font_family": "SFMono-Regular",
    "font_size": 14,
    "minimap_on_by_default": false,
    "show_line_numbers": true,
    "show_title_bar": true,
    "show_tagline": true
  },
  "indentation": {
    "tab_width": 4,
    "hard_tabs": false
  },
  "recovery": {
    "save_temporary_files": true,
    "keep_unsaved_changes": true,
    "temporary_files_location": null
  },
  "recent_files": {
    "enabled": true,
    "max_files": 10
  },
  "editor": {
    "normal_undo_bytes": 67108864,
    "large_undo_bytes": 8388608,
    "normal_search_matches": 20000,
    "large_search_matches": 2000
  },
  "workspace": {
    "max_entries": 100000,
    "open_tab_search_results": 100,
    "search_max_file_bytes": 8388608,
    "search_max_matches": 2000,
    "git_enabled": true
  },
  "lsp": {
    "enabled": false,
    "command": [],
    "file_extensions": ["rs"],
    "max_document_bytes": 4194304
  }
}
```

Changing editor budgets or indentation updates existing tabs as well as future ones. Reducing the
undo budget prunes complete oldest history groups immediately. By default Tab inserts four spaces;
enable Use Tab Characters to insert literal tab characters, and choose a width from 1 through 8.
Shift-Tab removes one available leading indentation level even when the prefix is partial spaces or
does not match the current tabs-versus-spaces setting; repeated presses remove the full prefix.

Command-, on macOS or Control-, elsewhere opens the native-styled Settings panel. Editor Font is a searchable dropdown populated
from installed system families; a configured family remains selectable if it is temporarily
unavailable. Appearance changes apply to tabs that have not been individually zoomed. Show Tagline
controls only “A fast place for text,” while View → Toggle Title Bar controls the complete heading;
both choices persist. Recovery copies are revisioned, atomic, and stored under `Backups/` when no
custom location is selected. Untitled recovery and unsaved changes to named files can be enabled
independently. Saving or discarding a tab removes its active recovery copy; the platform's primary
shortcut plus Q writes a final snapshot before exiting. Recent Files controls a newest-first local Open Recent list (1–100 entries,
default 10); disabling it clears the list, and Clear History is also available independently. See
[privacy.md](privacy.md) for the complete local-data boundary.

File → Revert… presents a destructive-action confirmation before reloading the active saved file
from disk. Cancel leaves the buffer and its recovery copy untouched; confirming discards unsaved
edits, clears the recovery copy, refreshes document metadata, and keeps tab-local display choices.

View → Toggle Line Numbers and the matching Settings switch control the gutter for every open and
future editor tab. The choice is persisted in `appearance.show_line_numbers` and defaults to true.
Settings also exposes Minimap On By Default. Tabs inherit changes to that default until View →
Toggle Minimap or the matching command-palette entry overrides that choice for only the active tab;
the per-tab choice is restored with the session. The right-hand minimap follows a bounded window of
up to 1,000 document rows, samples at most 120 marks, and advances with editor or wheel scrolling. It
marks the visible viewport, accepts clicks to jump within the displayed region, and lets you drag
the viewport like a vertical scrollbar without scanning every line on repaint.

## Native menus and editor gestures

Textify installs Textify, File, Edit, View, and Window menus in the macOS menu bar and an equivalent
in-window application menu on Windows and Linux. The View menu and Command-Shift-P palette on macOS
(Control-Shift-P elsewhere) expose per-tab word wrap; Option-Z on macOS or Alt-Z elsewhere is its
direct shortcut. Large and huge files always remain unwrapped. The palette and View menu also expose
the per-tab minimap. Command-scroll on macOS or Control-scroll elsewhere adjusts only the hovered
active tab's size, while preserving its visible caret position; Command-T or Control-T creates a new
tab. Overflowed tabs accept horizontal trackpad or ordinary wheel
scrolling, activating a tab reveals it, and the ribbon chevron lists every open document.
Command-Option-P on macOS or Control-Alt-P elsewhere opens the searchable Open Tabs switcher, whose
query supports fuzzy text and ordered `*` wildcard fragments. Every overlay shows a highlighted
selection and supports Up, Down, Enter, and Escape. Dirty tabs close through a modal Save / Don't
Save / Cancel decision, and Save closes only after a successful write. Dropped files follow the same
validation and large-file policy as Open. The editor omits generic textbox chrome. When the Textify
title row is hidden, macOS traffic-light space remains reserved ahead of the tab toolbar. The status
bar's WRAP / NO WRAP label is clickable and always reflects the active tab; its other metadata
progressively collapses at narrow window widths so paths and counters cannot overlap.

Default `keymap.json`:

```json
{
  "command_palette": "secondary-shift-p",
  "search_open_tabs": "secondary-p",
  "workspace_search": "secondary-shift-f",
  "open_folder": "secondary-shift-o",
  "toggle_sidebar": "secondary-b",
  "go_to_definition": "f12"
}
```

Newly loaded shortcuts become active immediately. GPUI does not expose selective removal from its
keymap, so a shortcut replaced during the current process remains as an alias until Textify is
restarted; the newly configured shortcut is active without a restart.

`secondary` means Command on macOS and Control on Windows and Linux. The generated default editor
font is SF Mono on macOS, Consolas on Windows, and DejaVu Sans Mono on Linux; the `settings.json`
example above shows the macOS value.

## Git

When `git_enabled` is true and the folder contains `.git`, Textify runs
`git status --porcelain=v1 -z` after indexing, refresh, reload, or save. It disables optional Git
locks and performs no network operation. Two-character porcelain states appear beside files. No Git
library or process is initialized at application launch.

## Optional language server

Set `lsp.enabled` to true and provide `lsp.command` as an executable followed by arguments. For
example, a Rust workspace can use:

```json
"lsp": {
  "enabled": true,
  "command": ["rust-analyzer"],
  "file_extensions": ["rs"],
  "max_document_bytes": 4194304
}
```

The server starts only after a folder is open. Textify speaks JSON-RPC over standard LSP
Content-Length framing, initializes the workspace, responds to configuration/registration requests,
sends full document changes after a 300 ms debounce, displays published diagnostics, and opens the
first result returned by F12 go-to-definition. Huge files, files outside the configured extensions,
and documents over `max_document_bytes` are never synchronized. The optional server process is
terminated with Textify or when LSP settings change.

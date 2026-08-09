# IDE workflows and configuration

IDE services are lazy. The first window creates only an untitled editor and the file watcher. A
project index, Git process, workspace search, or language server cannot start until a folder is
opened or the corresponding command is invoked.

## Project explorer, quick-open, and search

Use **Open Folder** (Command-Shift-O) to build a bounded background index. The sidebar renders its
flattened tree through a virtualized list; directory rows are always expanded in this initial IDE
workflow. Refresh from the sidebar to rescan files and Git state. The folder is persisted with the
tab session.

Quick Open (Command-P) performs subsequence scoring over indexed relative paths. Workspace Search
(Command-Shift-F) scans indexed files on a background executor and streams matches into a
virtualized result list. Starting another query cancels the previous scan. Binary files and files
over `search_max_file_bytes` are skipped; result count is bounded by `search_max_matches`.

## Settings

Textify creates `settings.json` and `keymap.json` in its application-data directory after the first
paint. Set `TEXTIFY_DATA_DIR` to use an isolated directory. Saving either file reloads it; malformed
JSON leaves the previous configuration active and reports the error in the status bar.

Default `settings.json`:

```json
{
  "appearance": {
    "font_family": "SFMono-Regular",
    "font_size": 14
  },
  "recovery": {
    "save_temporary_files": true,
    "keep_unsaved_changes": true,
    "temporary_files_location": null
  },
  "editor": {
    "normal_undo_bytes": 67108864,
    "large_undo_bytes": 8388608,
    "normal_search_matches": 20000,
    "large_search_matches": 2000
  },
  "workspace": {
    "max_entries": 100000,
    "quick_open_results": 100,
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

Changing editor budgets updates existing tabs as well as future ones. Reducing the undo budget
prunes complete oldest history groups immediately.

Command-, opens the native-styled Settings panel. Appearance changes apply to tabs that have not
been individually zoomed. Recovery copies are revisioned, atomic, and stored under `Backups/` when
no custom location is selected. Untitled recovery and unsaved changes to named files can be enabled
independently. Saving or discarding a tab removes its active recovery copy; Command-Q writes a final
snapshot before exiting. See [privacy.md](privacy.md) for the complete local-data boundary.

## Native menus and editor gestures

Textify installs Textify, File, Edit, View, and Window menus in the macOS menu bar. The View menu and
Command-Shift-P palette expose per-tab word wrap; Option-Z is its direct shortcut. Large and huge
files always remain unwrapped. Command-scroll adjusts only the hovered active tab's size, while
Command-T creates a new tab. Dropped files follow the same validation and large-file policy as Open.

Default `keymap.json`:

```json
{
  "command_palette": "cmd-shift-p",
  "quick_open": "cmd-p",
  "workspace_search": "cmd-shift-f",
  "open_folder": "cmd-shift-o",
  "toggle_sidebar": "cmd-b",
  "go_to_definition": "f12"
}
```

Newly loaded shortcuts become active immediately. GPUI does not expose selective removal from its
keymap, so a shortcut replaced during the current process remains as an alias until Textify is
restarted; the newly configured shortcut is active without a restart.

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

# Settings Window

Status: complete.

Shortcut: Cmd + ,

Initial settings for a proper full-featured text editor:

- [x] Font and size.
- [x] Save Temporary Files continuously snapshots Untitled tabs and restores them after restart.
- [x] Keep Unsaved Changes independently snapshots edits to named files without overwriting them.
- [x] Temporary Files Location defaults to Textify's private `Backups` folder and is configurable.
- [x] Graceful Command-Q flushes the newest snapshots before exit; snapshots are atomic for crash and
  force-quit recovery.

# Additional Shortcuts

- [x] Cmd + T: Open new tab.

- [x] Cmd + Scrollwheel: Change text size for the active tab only.

# File Name

- [x] Untitled tabs use a bounded, whitespace-normalized first-line title with an ellipsis when
  needed.

# Drag and Drop

- [x] Drag and drop one or more files into the app to open them in tabs.

# Command Palette

- [x] Command-Shift-P opens the centered palette; ranked natural-language matching covers document
  and IDE commands.

# Word Wrap

- [x] Word wrap is a per-tab toggle in the native macOS View menu and command palette. Large-file
  policy always keeps it off.
- [x] Native Textify, File, Edit, View, and Window menus live in the macOS menu bar.

# More stuff

- [x] Unsaved documents show a dot in their tab and window title plus an `UNSAVED` status badge.

- [x] Textify installs no network client and contains no telemetry, analytics, crash uploader, or
  update checker. The boundary is documented and regression tested in `docs/privacy.md`.

# UX hardening

- [x] Overflowed tabs scroll, every activation reveals the selected tab, and Command-Option-P
  searches open files with fuzzy or `*` wildcard matching.
- [x] Command overlays close directly from Escape or an outside click and return focus to the editor.
- [x] Enabling word wrap immediately reflows against the usable text width without edge clipping.
- [x] Command-scroll zoom preserves the document row under the pointer through every reflow, with
  the visible caret as a fallback when the pointer is outside the editor.
- [x] Editor Font is a searchable dropdown populated from installed font families.
- [x] View → Toggle Title Bar hides or restores the complete Textify heading and persists the choice.
- [x] Settings → Show Tagline independently hides or restores “A fast place for text.”

# Interaction hardening — close, navigation, and history

- [x] Command-W and every tab close button close clean temporary tabs immediately; dirty temporary
  or named tabs use a rendered, keyboard-operable Save / Don't Save / Cancel dialog, and Save closes
  only after a successful write or Save As.
- [x] Open Tabs opens as a ten-row keyboard-driven list of every tab, keeps the highlighted row in
  view, filters live, and activates/reveals the selected tab with Enter.
- [x] File → Open Recent uses a private newest-first history with a configurable 1–100 item limit,
  an enable switch that clears history when disabled, and explicit Clear History commands.
- [x] The code-editor surface no longer draws the generic rounded textbox border.
- [x] Command-P and the magnifying glass search live text across all editable tabs, including
  unsaved tabs, rank phrase/all-word matches, and jump to the selected document location.
- [x] Hiding the Textify title row reserves the macOS traffic-light area ahead of the tab toolbar.
- [x] The project explorer and cancellable folder-wide Workspace Search remain separate from
  open-tab navigation and content search.
- [x] Session and recent-history writes no longer masquerade as settings/keymap reloads; genuine
  configuration reload notices name what changed and every bottom-bar status message is dismissible.

# Workspace, editor, and compact-window polish

- [x] A folder can be closed from File → Close Folder, the command palette, or the explorer header;
  folder services stop while already-open tabs remain available.
- [x] The status bar truncates its path and progressively hides secondary metadata before narrow
  windows can make counters overlap the filename.
- [x] Indentation is explicit and persisted: the default is four spaces, with a 1–8 tab-width
  setting and an independent Use Tab Characters switch applied to every tab.
- [x] The explorer lists known text, configuration, and source files only; binary/media/archive
  files, `.DS_Store`, and ignored dependency/build directories stay hidden.
- [x] `.html` and `.htm` documents use the bundled Tree-sitter HTML highlighter.
- [x] WRAP / NO WRAP in the status bar is a clickable per-tab toggle and respects large-file policy.
- [x] The decorative circle beside the Textify title was removed; it was not an application-icon
  slot.
- [x] The redundant right-side Save button was removed from the tab ribbon; File → Save and
  Command-S remain available.

# File context, shell highlighting, and DOS text

- [x] Open File, Open Folder, and Save As start beside the active saved document, falling back to
  the open workspace for an untitled tab.
- [x] `.sh`, `.bash`, `.zsh`, `.bashrc`, `.zshrc`, and `.profile` use the bundled Tree-sitter Bash
  grammar and display as Shell.
- [x] Text-like invalid UTF-8 can be detected and decoded as CP437; binary-looking files remain
  rejected.
- [x] The clickable encoding status opens a keyboard-driven UTF-8 / CP437 “Reopen With Encoding”
  picker for clean saved files, and the explicit choice survives session restore.
- [x] CP437 documents save atomically back to CP437 without a document-sized conversion buffer;
  unrepresentable edits fail without replacing the original file.

# Long-running and deployment readiness

- [x] View → Toggle Line Numbers, the command palette, and Settings can show or hide the editor
  gutter globally; the persisted choice applies immediately to existing and future tabs.
- [x] The optimized GUI has a recorded native `ps`, `vmmap`, and `leaks` baseline; its bounded idle
  soak stayed flat, and UTF-8 loading no longer duplicates the complete file buffer.
- [x] A thin-LTO release build creates a valid local macOS app bundle. Safe installer symlinks expose
  it at `~/Applications/Textify.app` and `~/bin/textify`, and the Raycast Script Command launches or
  focuses the one existing app instance.

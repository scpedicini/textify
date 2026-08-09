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
- [x] Command-scroll zoom preserves the visible caret position through the new layout.
- [x] Editor Font is a searchable dropdown populated from installed font families.
- [x] View → Toggle Title Bar hides or restores the complete Textify heading and persists the choice.
- [x] Settings → Show Tagline independently hides or restores “A fast place for text.”

# Interaction hardening — close, navigation, and history

- [x] Command-W and every tab close button close clean temporary tabs immediately; dirty temporary
  or named tabs use a Save / Don't Save / Cancel dialog, and Save closes only after a successful
  write or Save As.
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

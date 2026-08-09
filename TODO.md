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

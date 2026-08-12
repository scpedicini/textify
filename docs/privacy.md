# Privacy and offline operation

Textify does not include telemetry, analytics, advertising, crash uploads, update checks, or an
application network client. Files, searches, settings, session data, and recovery copies remain on
the computer. GPUI contains an HTTP abstraction for applications that opt into it, but Textify leaves
GPUI's default `NullHttpClient` installed, so attempted remote asset requests fail locally.

Local data is stored in the operating system's normal application-data location by default:

- macOS: `~/Library/Application Support/Textify`
- Windows: `%APPDATA%\Textify`
- Linux: `$XDG_CONFIG_HOME/textify`, or `~/.config/textify` when `XDG_CONFIG_HOME` is unset

Set `TEXTIFY_DATA_DIR` to override this location. Within it:

- `settings.json` and `keymap.json` contain user preferences.
- `session.json` records open tabs and workspace state.
- `recent-files.json` contains the bounded Open Recent path list when that setting is enabled. It
  can be cleared from the File menu or Settings; disabling recent files also clears it.
- `Backups/` contains atomic crash-recovery copies when recovery is enabled. The location is
  configurable in Settings, and each copy is removed after its tab is saved or discarded.

Textify's diagnostic logging writes only to the local process output. It can include local file
paths and timings, but never document contents.

## Explicit integrations

- Git decorations invoke the locally installed `git` executable and read its standard output.
- Language-server support is disabled by default. When explicitly enabled, Textify starts the
  configured local executable and sends eligible open-document contents over that process's
  standard input. A language server may have its own networking or telemetry behavior, so its
  privacy policy is separate from Textify's.

Downloading Rust crates while building Textify is a build-time operation, not application
telemetry. Once built, normal editor operation requires no network connection.

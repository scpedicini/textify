# Isolated development and manual testing

Development runs must not use the installed Textify's settings, session, unsaved buffers, recovery
files, or instance lock. Updating the installed/Raycast app is a separate, explicit operation.

## Launch a development instance

On macOS or Linux, from the repository root:

```sh
./scripts/run-isolated.sh
./scripts/run-isolated.sh --minimap-demo
./scripts/run-isolated.sh -- path/to/example.json
```

The launcher builds the debug binary, then copies it into a private instance directory under
`/tmp/textify-dev.*`. On macOS it creates **Textify Dev.app**, with the distinct bundle identifier
`com.shaun.textify.dev`, and does not register file associations or change application links.
Every run gets new settings, session storage, recent-file history, recovery storage, and an instance
lock through `TEXTIFY_DATA_DIR`. Minimap is enabled and external language servers are disabled.
Any file paths passed to the launcher are copied into that instance before opening, so editing and
saving those test tabs changes only the copies.

`--minimap-demo` opens four generated tabs:

- 82-line JSON for checking short documents and syntax colors;
- Markdown with headings, indentation, blank lines, and fenced code;
- 50,000-line plain text for long-document dragging;
- 10,002-line JSON for long-document dragging with syntax colors.

Leave the development window open when asking someone to test it. Identify it by **Textify Dev**
in the Dock/application switcher and the `TEST-` tab names. New builds also show **DEV** in the
title bar and **Textify Dev** in the window title. Do not ask the user to quit their real Textify
to test a change.

The launcher prints its private instance directory. It retains that directory after quitting so
test edits and recovery files are not silently deleted. File → Open still opens the selected
original file; use the generated/copied tabs when experimenting. These temporary instances are
for testing, not long-term storage of important work.

## Other build and launch paths

Debug builds (`cargo run --locked`, including on Windows) default to a separate **Textify Dev**
data directory rather than the installed application's directory. On Linux the directory is
`textify-dev` under the usual config root. `TEXTIFY_DEV_INSTANCE=1` gives an optimized binary the
same development identity. An explicit `TEXTIFY_DATA_DIR` always wins; never point a test run at
the installed app's real data directory or copy its settings with a custom recovery path.

To test an already built binary without rebuilding:

```sh
./scripts/run-isolated.sh --binary /path/to/textify --minimap-demo
```

The binary is copied, not linked. To build an optimized experiment, use a separate target directory:

```sh
cargo build --locked --release --bin textify --target-dir target/optimized-dev
./scripts/run-isolated.sh --binary target/optimized-dev/release/textify --minimap-demo
```

Do not run `cargo build --release --bin textify`, `build-release.sh`, or `install-local.sh` as part of
ordinary iteration. Legacy local installations link directly to `target/release/textify`; rebuilding
that file changes what Raycast launches even without running the installer. The isolated launcher
uses the debug profile and leaves that release file alone.

## Explicitly update the installed app

Only after an explicit request to update the installed/Raycast version:

```sh
./scripts/install-local.sh
```

The installer makes a self-contained copy under `~/Applications/.textify-local-builds/` and points
`~/Applications/Textify.app` and `~/bin/textify` at that snapshot. Later compilation and `cargo clean`
cannot replace its executable. Previous snapshots are retained. Existing legacy links migrate on
the next explicit install; introducing this workflow does not change those links or restart the app.

After an explicit install, quit and relaunch the installed Textify to use the new version. Its
settings, session, and unsaved-buffer recovery remain in its normal data directory.

#!/bin/zsh

# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title Textify
# @raycast.mode silent

# Optional parameters:
# @raycast.icon 📝
# @raycast.packageName Textify IDE

# Documentation:
# @raycast.description Launch or focus the local optimized Textify build

# Keep this Script Command: macOS Launch Services gives the app correct focus/reopen behavior.
# The installed bundle and its executable are symlinks into this checkout's Cargo release output,
# so rebuilding target/release/textify updates what the next Raycast launch runs. A process that is
# already open must still be quit and relaunched to load the rebuilt executable.
application="${HOME}/Applications/Textify.app"
executable="${application}/Contents/MacOS/Textify"

if [[ ! -d "${application}" || ! -x "${executable}" ]]; then
    print -u2 "Textify is not installed. Run scripts/install-local.sh from the repository."
    exit 1
fi

/usr/bin/open "${application}"

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

application="${HOME}/Applications/Textify.app"

if [[ ! -d "${application}" ]]; then
    print -u2 "Textify is not installed. Run scripts/install-local.sh from the repository."
    exit 1
fi

open "${application}"

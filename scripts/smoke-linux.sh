#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 /path/to/textify" >&2
    exit 2
fi

binary="$(realpath "$1")"
test -x "${binary}"
runtime_directory="$(mktemp -d)"
trap 'rm -rf "${runtime_directory}"' EXIT
chmod 0700 "${runtime_directory}"

set +e
timeout 10s dbus-run-session -- xvfb-run -a -s "-screen 0 1280x800x24" env \
    HOME="${runtime_directory}" \
    XDG_CONFIG_HOME="${runtime_directory}/config" \
    XDG_RUNTIME_DIR="${runtime_directory}" \
    TEXTIFY_DATA_DIR="${runtime_directory}/textify" \
    LIBGL_ALWAYS_SOFTWARE=1 \
    WGPU_BACKEND=vulkan \
    "${binary}" > "${runtime_directory}/textify.log" 2>&1
exit_code=$?
set -e

if [[ ${exit_code} -ne 124 ]]; then
    cat "${runtime_directory}/textify.log" >&2
    echo "Textify exited during the Linux startup smoke test (exit ${exit_code})" >&2
    if [[ ${exit_code} -eq 0 ]]; then
        exit 1
    fi
    exit "${exit_code}"
fi

echo "Textify remained responsive for the 10-second Linux startup smoke window"

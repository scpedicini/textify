#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 /path/to/Textify" >&2
    exit 2
fi

binary="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
test -x "${binary}"
runtime_directory="$(mktemp -d)"
textify_pid=""

cleanup() {
    if [[ -n "${textify_pid}" ]] && kill -0 "${textify_pid}" 2>/dev/null; then
        kill "${textify_pid}" 2>/dev/null || true
        wait "${textify_pid}" 2>/dev/null || true
    fi
    rm -rf "${runtime_directory}"
}
trap cleanup EXIT

TEXTIFY_DATA_DIR="${runtime_directory}/textify" \
    "${binary}" > "${runtime_directory}/textify.log" 2>&1 &
textify_pid=$!
sleep 10

if ! kill -0 "${textify_pid}" 2>/dev/null; then
    set +e
    wait "${textify_pid}"
    exit_code=$?
    set -e
    cat "${runtime_directory}/textify.log" >&2
    echo "Textify exited during the macOS startup smoke test (exit ${exit_code})" >&2
    if [[ ${exit_code} -eq 0 ]]; then
        exit 1
    fi
    exit "${exit_code}"
fi

echo "Textify remained responsive for the 10-second macOS startup smoke window"

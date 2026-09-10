#!/usr/bin/env bash
# Build and launch a private development instance. Never installs or registers Textify.
set -euo pipefail

script_directory="$(cd "$(dirname "$0")" && pwd)"
repository_root="$(cd "$script_directory/.." && pwd)"
binary=""
minimap_demo=false
documents=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary)
            [[ $# -ge 2 ]] || { echo "--binary requires a path" >&2; exit 2; }
            binary="$2"
            shift 2
            ;;
        --minimap-demo) minimap_demo=true; shift ;;
        --help|-h)
            echo "Usage: $0 [--minimap-demo] [--binary PATH] [--] [FILE ...]"
            echo "Each run uses fresh private data and copies any supplied files before opening."
            exit 0
            ;;
        --) shift; documents+=("$@"); break ;;
        -*) echo "Unknown option: $1" >&2; exit 2 ;;
        *) documents+=("$1"); shift ;;
    esac
done

if [[ -z "$binary" ]]; then
    # Pin the dev profile and target path: installed legacy apps can point into target/release.
    cargo build --locked --manifest-path "$repository_root/Cargo.toml" \
        --target-dir "$repository_root/target" --profile dev --bin textify
    binary="$repository_root/target/debug/textify"
fi
[[ -x "$binary" ]] || { echo "Not an executable: $binary" >&2; exit 1; }
for document in ${documents[@]+"${documents[@]}"}; do
    [[ -f "$document" ]] || { echo "Not a file: $document" >&2; exit 1; }
done

# A short path also keeps macOS Unix socket names under their length limit.
instance_directory="$(mktemp -d /tmp/textify-dev.XXXXXX)"
mkdir -p "$instance_directory/data" "$instance_directory/documents"
printf '%s\n' '{"appearance":{"minimap_on_by_default":true},"lsp":{"enabled":false}}' \
    > "$instance_directory/data/settings.json"

open_paths=()
if "$minimap_demo"; then
    bash "$script_directory/create-minimap-demo.sh" "$instance_directory/documents"
    open_paths+=(
        "$instance_directory/documents/TEST-short.json"
        "$instance_directory/documents/TEST-sections.md"
        "$instance_directory/documents/TEST-50000-lines.txt"
        "$instance_directory/documents/TEST-10000-lines.json"
    )
fi
index=0
for document in ${documents[@]+"${documents[@]}"}; do
    index=$((index + 1))
    copy="$instance_directory/documents/TEST-$index-$(basename "$document")"
    cp "$document" "$copy"
    open_paths+=("$copy")
done

if [[ "$(uname -s)" == Darwin ]]; then
    bundle="$instance_directory/Textify Dev.app/Contents"
    mkdir -p "$bundle/MacOS" "$bundle/Resources"
    cp "$repository_root/packaging/Info-dev.plist" "$bundle/Info.plist"
    cp "$repository_root/packaging/Textify.icns" "$bundle/Resources/Textify.icns"
    executable="$bundle/MacOS/Textify Dev"
else
    executable="$instance_directory/textify-dev"
fi
# Copy, never symlink: later builds must not replace this running instance's executable.
cp "$binary" "$executable"

export TEXTIFY_DATA_DIR="$instance_directory/data"
export TEXTIFY_DEV_INSTANCE=1
echo "Starting Textify Dev. Installed Textify is untouched."
echo "Private instance: $instance_directory"
echo "Session and recovery files are retained here after you quit."
exec "$executable" ${open_paths[@]+"${open_paths[@]}"}

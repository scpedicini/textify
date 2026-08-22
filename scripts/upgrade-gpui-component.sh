#!/usr/bin/env bash
# Manage the vendored gpui-component fork (vendor/gpui-component).
#
# The fork's local changes are documented in vendor/gpui-component/TEXTIFY_FORK.md,
# guarded by tests/vendor_fork.rs, and captured as a reapplyable diff in
# vendor/patches/gpui-component-textify.patch. This script keeps those three in
# sync with upstream:
#
#   scripts/upgrade-gpui-component.sh upgrade <upstream-git-sha>
#       Replace the vendored sources with the given upstream commit (normalized
#       with rustfmt so formatting noise never pollutes diffs) and reapply the
#       Textify patch. Conflicting hunks are written next to their files as
#       .rej files for manual resolution.
#
#   scripts/upgrade-gpui-component.sh regenerate-patch <baseline-git-sha>
#       Rebuild vendor/patches/gpui-component-textify.patch as the diff between
#       the given pristine upstream commit and the current vendored sources.
#       Run this after an upgrade is resolved (with the new sha) or after
#       adding a fork change (with the sha recorded in
#       vendor/gpui-component/.cargo_vcs_info.json).
#
# Upstream lives at https://github.com/longbridge/gpui-component (crates/ui).
#
# Full upgrade recipe:
#   1. scripts/upgrade-gpui-component.sh upgrade <new-sha>
#   2. resolve any .rej files, then delete them
#   3. cargo test --workspace   (tests/vendor_fork.rs verifies the fork survived)
#   4. update the commit hash in vendor/gpui-component/TEXTIFY_FORK.md
#   5. scripts/upgrade-gpui-component.sh regenerate-patch <new-sha>
#   6. commit vendor/, the patch, and Cargo.lock together
set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT=$(pwd)
VENDOR_DIR="$REPO_ROOT/vendor/gpui-component"
PATCH_FILE="$REPO_ROOT/vendor/patches/gpui-component-textify.patch"
UPSTREAM_URL="https://github.com/longbridge/gpui-component"
# Files that belong to the vendoring/packaging layer or to Textify's own
# documentation, not to the upstream-vs-fork source diff.
DIFF_EXCLUDES=(.cargo-ok .cargo_vcs_info.json Cargo.lock Cargo.toml.orig README.md TEXTIFY_FORK.md)

usage() {
    sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'
    exit 64
}

# Download upstream crates/ui at the given sha and normalize its formatting the
# same way the vendored copy is formatted, so diffs contain only real changes.
fetch_normalized_upstream() {
    local sha=$1 destination=$2
    local archive="$WORKDIR/upstream-$sha.tar.gz"
    echo "Fetching $UPSTREAM_URL at $sha..."
    curl -sfL --max-time 300 -o "$archive" "$UPSTREAM_URL/archive/$sha.tar.gz"
    tar -xzf "$archive" -C "$WORKDIR"
    mv "$WORKDIR/gpui-component-$sha/crates/ui" "$destination"
    echo "Normalizing formatting (rustfmt, edition 2024)..."
    find "$destination" -name '*.rs' -exec rustfmt --edition 2024 {} +
}

# Copy the vendored crate without packaging artifacts and Textify documents.
copy_vendor_sources() {
    local destination=$1
    mkdir -p "$destination"
    (cd "$VENDOR_DIR" && tar -cf - --exclude=./target $(printf -- '--exclude=./%s ' "${DIFF_EXCLUDES[@]}") .) |
        tar -xf - -C "$destination"
}

WORKDIR=$(mktemp -d /tmp/gpui-component-upgrade.XXXXXX)
trap 'rm -rf "$WORKDIR"' EXIT

command=${1:-}
sha=${2:-}
[ -n "$command" ] && [ -n "$sha" ] || usage

case "$command" in
upgrade)
    fetch_normalized_upstream "$sha" "$WORKDIR/upstream"
    [ -f "$PATCH_FILE" ] || {
        echo "error: $PATCH_FILE is missing; regenerate it first" >&2
        exit 1
    }

    echo "Replacing vendored sources (TEXTIFY_FORK.md is preserved)..."
    cp "$VENDOR_DIR/TEXTIFY_FORK.md" "$WORKDIR/TEXTIFY_FORK.md"
    find "$VENDOR_DIR" -mindepth 1 -maxdepth 1 ! -name target -exec rm -rf {} +
    cp -R "$WORKDIR/upstream/." "$VENDOR_DIR/"
    cp "$WORKDIR/TEXTIFY_FORK.md" "$VENDOR_DIR/TEXTIFY_FORK.md"
    printf '{\n  "git": {\n    "sha1": "%s"\n  },\n  "path_in_vcs": "crates/ui"\n}' "$sha" \
        >"$VENDOR_DIR/.cargo_vcs_info.json"

    echo "Reapplying the Textify fork patch..."
    if git apply -p1 --directory=vendor/gpui-component --reject "$PATCH_FILE"; then
        echo "Patch applied cleanly."
    else
        echo
        echo "Some hunks did not apply. Resolve these rejects by hand:"
        find "$VENDOR_DIR" -name '*.rej' | sed 's/^/  /'
    fi
    echo
    echo "Next: resolve rejects (if any), run 'cargo test --workspace', update the"
    echo "commit hash in vendor/gpui-component/TEXTIFY_FORK.md, then run:"
    echo "  scripts/upgrade-gpui-component.sh regenerate-patch $sha"
    ;;
regenerate-patch)
    fetch_normalized_upstream "$sha" "$WORKDIR/a"
    copy_vendor_sources "$WORKDIR/b"
    mkdir -p "$(dirname "$PATCH_FILE")"
    echo "Writing $PATCH_FILE..."
    # diff exits 1 when the trees differ, which is the expected outcome.
    (cd "$WORKDIR" && diff -ruN a b >"$PATCH_FILE") && {
        echo "error: no difference between upstream $sha and the vendored crate" >&2
        exit 1
    } || [ $? -eq 1 ]
    grep -c '^diff -ruN' "$PATCH_FILE" |
        xargs -I{} echo "Patch covers {} files. Commit it together with vendor/."
    ;;
*)
    usage
    ;;
esac

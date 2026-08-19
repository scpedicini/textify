#!/usr/bin/env bash
set -euo pipefail

# Raises the Textify package version in Cargo.toml and Cargo.lock.
#
# Usage: scripts/bump-version.sh <major|minor|patch>
#
# The new version is written to stdout so callers can capture it.

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "${script_directory}/.." && pwd)"
manifest="${repository_root}/Cargo.toml"
lockfile="${repository_root}/Cargo.lock"

level="${1:-}"
case "${level}" in
    major | minor | patch) ;;
    *)
        echo "Usage: $(basename "$0") <major|minor|patch>" >&2
        exit 2
        ;;
esac

current="$(awk -F\" '/^version = / { print $2; exit }' "${manifest}")"
if [[ ! "${current}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Cargo.toml holds the non-numeric version '${current}'; bump it by hand." >&2
    exit 1
fi

IFS=. read -r major minor patch <<<"${current}"
case "${level}" in
    major)
        major=$((major + 1))
        minor=0
        patch=0
        ;;
    minor)
        minor=$((minor + 1))
        patch=0
        ;;
    patch)
        patch=$((patch + 1))
        ;;
esac
new="${major}.${minor}.${patch}"

# Only the first `version = ` line belongs to the [package] table.
awk -v replacement="${new}" '
    !replaced && /^version = "/ {
        sub(/"[^"]*"/, "\"" replacement "\"")
        replaced = 1
    }
    { print }
' "${manifest}" > "${manifest}.bump"
mv "${manifest}.bump" "${manifest}"

# In Cargo.lock the version line always directly follows its package name.
awk -v replacement="${new}" '
    matched && /^version = "/ {
        sub(/"[^"]*"/, "\"" replacement "\"")
        matched = 0
    }
    /^name = "textify"$/ { matched = 1 }
    { print }
' "${lockfile}" > "${lockfile}.bump"
mv "${lockfile}.bump" "${lockfile}"

written="$(awk -F\" '/^version = / { print $2; exit }' "${manifest}")"
locked="$(awk '/^name = "textify"$/ { found = 1; next } found && /^version = "/ { gsub(/[^0-9.]/, ""); print; exit }' "${lockfile}")"
if [[ "${written}" != "${new}" || "${locked}" != "${new}" ]]; then
    echo "Version bump failed: Cargo.toml has '${written}' and Cargo.lock has '${locked}', expected '${new}'." >&2
    exit 1
fi

echo "${new}"

#!/bin/zsh

set -euo pipefail

script_directory="${0:A:h}"
repository_root="${script_directory:h}"
release_directory="${repository_root}/target/release"
bundle="${release_directory}/Textify.app"
bundle_contents="${bundle}/Contents"
bundle_executable="${bundle_contents}/MacOS/Textify"
source_executable="${release_directory}/textify"

cargo build --manifest-path "${repository_root}/Cargo.toml" --release --bin textify

mkdir -p "${bundle_contents}/MacOS" "${bundle_contents}/Resources"
cp "${repository_root}/packaging/Info.plist" "${bundle_contents}/Info.plist"

version="$(awk -F\" '/^version = / { print $2; exit }' "${repository_root}/Cargo.toml")"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${bundle_contents}/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${version}" "${bundle_contents}/Info.plist"

# This development-only bundle follows future `cargo build --release` outputs without copying
# the Mach-O. A signed distribution bundle should copy the executable before signing instead.
ln -sfn ../../../textify "${bundle_executable}"

plutil -lint "${bundle_contents}/Info.plist"
test -x "${bundle_executable}"
touch "${bundle}"

print "Built ${bundle}"

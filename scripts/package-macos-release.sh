#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "${script_directory}/.." && pwd)"
dist_directory="${TEXTIFY_DIST_DIR:-${repository_root}/dist}"
target_directory="${CARGO_TARGET_DIR:-${repository_root}/target}"
version="$(awk -F\" '/^version = / { print $2; exit }' "${repository_root}/Cargo.toml")"
requested_architectures="${TEXTIFY_MACOS_ARCHES:-aarch64-apple-darwin,x86_64-apple-darwin}"
IFS=',' read -r -a architectures <<< "${requested_architectures}"
bundle="${dist_directory}/Textify.app"
bundle_contents="${bundle}/Contents"
bundle_executable="${bundle_contents}/MacOS/Textify"
if [[ ${#architectures[@]} -eq 1 ]]; then
    case "${architectures[0]}" in
        aarch64-apple-darwin) package_architecture="arm64" ;;
        x86_64-apple-darwin) package_architecture="x64" ;;
        *) package_architecture="${architectures[0]}" ;;
    esac
else
    package_architecture="universal"
fi
archive="${dist_directory}/textify-${version}-macos-${package_architecture}.zip"

cd "${repository_root}"
for architecture in "${architectures[@]}"; do
    rustup target add "${architecture}"
    cargo build --locked --release --bin textify --target "${architecture}"
done

mkdir -p "${dist_directory}"
rm -rf "${bundle}" "${archive}"
mkdir -p "${bundle_contents}/MacOS" "${bundle_contents}/Resources"
cp "${repository_root}/packaging/Info.plist" "${bundle_contents}/Info.plist"
cp "${repository_root}/packaging/Textify.icns" "${bundle_contents}/Resources/Textify.icns"

if [[ ${#architectures[@]} -eq 1 ]]; then
    cp "${target_directory}/${architectures[0]}/release/textify" "${bundle_executable}"
else
    inputs=()
    for architecture in "${architectures[@]}"; do
        inputs+=("${target_directory}/${architecture}/release/textify")
    done
    lipo -create "${inputs[@]}" -output "${bundle_executable}"
fi
chmod 0755 "${bundle_executable}"

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${version}" "${bundle_contents}/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion ${version}" "${bundle_contents}/Info.plist"
plutil -lint "${bundle_contents}/Info.plist"

if [[ -n "${TEXTIFY_MACOS_SIGN_IDENTITY:-}" ]]; then
    codesign --force --deep --options runtime --timestamp --sign "${TEXTIFY_MACOS_SIGN_IDENTITY}" "${bundle}"
    codesign --verify --deep --strict "${bundle}"
fi

ditto -c -k --sequesterRsrc --keepParent "${bundle}" "${archive}"
echo "Created ${archive}"

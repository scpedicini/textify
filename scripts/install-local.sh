#!/bin/zsh

set -euo pipefail

script_directory="${0:A:h}"
repository_root="${script_directory:h}"
binary_target="${repository_root}/target/release/textify"
bundle_target="${repository_root}/target/release/Textify.app"
stale_distribution_bundle="${repository_root}/dist/Textify.app"
binary_link="${HOME}/bin/textify"
bundle_link="${HOME}/Applications/Textify.app"
installation_root="${HOME}/Applications/.textify-local-builds"
launch_services="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

mkdir -p "${HOME}/bin" "${HOME}/Applications"

for link in "${binary_link}" "${bundle_link}"; do
    if [[ -e "${link}" && ! -L "${link}" ]]; then
        print -u2 "Refusing to replace non-symlink: ${link}"
        exit 1
    fi
done

"${script_directory}/build-release.sh"

# Install a snapshot outside Cargo's target tree. Future builds and cargo clean must
# not change the installed app, and existing processes can keep using older snapshots.
mkdir -p "${installation_root}"
installation_directory="$(mktemp -d "${installation_root}/build.XXXXXX")"
installed_bundle="${installation_directory}/Textify.app"
installed_contents="${installed_bundle}/Contents"
installed_binary="${installed_contents}/MacOS/Textify"
mkdir -p "${installed_contents}/MacOS"
cp "${bundle_target}/Contents/Info.plist" "${installed_contents}/Info.plist"
cp -R "${bundle_target}/Contents/Resources" "${installed_contents}/Resources"
cp "${binary_target}" "${installed_binary}"

ln -sfn "${installed_binary}" "${binary_link}"
ln -sfn "${installed_bundle}" "${bundle_link}"
if [[ -x "${launch_services}" ]]; then
    # A locally opened distribution build has the same bundle identifier and can otherwise remain
    # eligible in Launch Services/Raycast after a development rebuild.
    if [[ -d "${stale_distribution_bundle}" ]]; then
        "${launch_services}" -u "${stale_distribution_bundle}" 2>/dev/null || true
    fi
    "${launch_services}" -f "${bundle_link}"
fi

print "Installed local Textify links:"
print "  ${binary_link} -> ${installed_binary}"
print "  ${bundle_link} -> ${installed_bundle}"
print "Raycast can launch the indexed ${bundle_link} application directly."
print "No Raycast Script Command directory is required."
print "Quit a running Textify instance before launching the newly installed version."

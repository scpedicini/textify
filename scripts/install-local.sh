#!/bin/zsh

set -euo pipefail

script_directory="${0:A:h}"
repository_root="${script_directory:h}"
binary_target="${repository_root}/target/release/textify"
bundle_target="${repository_root}/target/release/Textify.app"
stale_distribution_bundle="${repository_root}/dist/Textify.app"
binary_link="${HOME}/bin/textify"
bundle_link="${HOME}/Applications/Textify.app"
launch_services="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

"${script_directory}/build-release.sh"

mkdir -p "${HOME}/bin" "${HOME}/Applications"

for link in "${binary_link}" "${bundle_link}"; do
    if [[ -e "${link}" && ! -L "${link}" ]]; then
        print -u2 "Refusing to replace non-symlink: ${link}"
        exit 1
    fi
done

ln -sfn "${binary_target}" "${binary_link}"
ln -sfn "${bundle_target}" "${bundle_link}"
if [[ -x "${launch_services}" ]]; then
    # A locally opened distribution build has the same bundle identifier and can otherwise remain
    # eligible in Launch Services/Raycast after a development rebuild.
    if [[ -d "${stale_distribution_bundle}" ]]; then
        "${launch_services}" -u "${stale_distribution_bundle}" 2>/dev/null || true
    fi
    "${launch_services}" -f "${bundle_link}"
fi

print "Installed local Textify links:"
print "  ${binary_link} -> ${binary_target}"
print "  ${bundle_link} -> ${bundle_target}"
print "Raycast can launch the indexed ${bundle_link} application directly."
print "No Raycast Script Command directory is required."
print "After future release rebuilds, quit a running Textify instance before launching it again."

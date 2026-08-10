#!/bin/zsh

set -euo pipefail

script_directory="${0:A:h}"
repository_root="${script_directory:h}"
binary_target="${repository_root}/target/release/textify"
bundle_target="${repository_root}/target/release/Textify.app"
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
    "${launch_services}" -f "${bundle_link}"
fi

print "Installed local Textify links:"
print "  ${binary_link} -> ${binary_target}"
print "  ${bundle_link} -> ${bundle_target}"
print "Add ${repository_root}/raycast as a Raycast Script Directory once."

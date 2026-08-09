#!/bin/zsh

set -euo pipefail

script_directory="${0:A:h}"
repository_root="${script_directory:h}"
source_icon="${repository_root}/packaging/Textify.svg"
output_icon="${repository_root}/packaging/Textify.icns"
work_directory="$(mktemp -d "${TMPDIR:-/tmp}/textify-icon.XXXXXX")"
iconset="${work_directory}/Textify.iconset"
master_png="${work_directory}/Textify-1024.png"
generated_icon="${work_directory}/Textify.icns"

trap 'rm -rf "${work_directory}"' EXIT

if command -v magick >/dev/null 2>&1; then
    magick -background none "${source_icon}" -resize 1024x1024 -depth 8 "${master_png}"
elif command -v convert >/dev/null 2>&1; then
    convert -background none "${source_icon}" -resize 1024x1024 -depth 8 "${master_png}"
else
    print -u2 "ImageMagick is required to regenerate Textify.icns from Textify.svg."
    exit 1
fi

mkdir -p "${iconset}"
for specification in \
    icon_16x16.png:16 \
    icon_16x16@2x.png:32 \
    icon_32x32.png:32 \
    icon_32x32@2x.png:64 \
    icon_128x128.png:128 \
    icon_128x128@2x.png:256 \
    icon_256x256.png:256 \
    icon_256x256@2x.png:512 \
    icon_512x512.png:512 \
    icon_512x512@2x.png:1024
do
    filename="${specification%%:*}"
    dimension="${specification##*:}"
    sips -z "${dimension}" "${dimension}" "${master_png}" \
        --out "${iconset}/${filename}" >/dev/null
done

iconutil -c icns "${iconset}" -o "${generated_icon}"
cp "${generated_icon}" "${output_icon}"

print "Built ${output_icon} from ${source_icon}"

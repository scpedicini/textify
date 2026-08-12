#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "${script_directory}/.." && pwd)"
dist_directory="${TEXTIFY_DIST_DIR:-${repository_root}/dist}"
target_directory="${CARGO_TARGET_DIR:-${repository_root}/target}"
version="$(awk -F\" '/^version = / { print $2; exit }' "${repository_root}/Cargo.toml")"

case "$(uname -m)" in
    x86_64|amd64)
        archive_arch="x64"
        deb_arch="amd64"
        ;;
    aarch64|arm64)
        archive_arch="arm64"
        deb_arch="arm64"
        ;;
    *)
        echo "Unsupported Linux architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

package_name="textify-${version}-linux-${archive_arch}"
stage_directory="${dist_directory}/${package_name}"
deb_root="${dist_directory}/deb-root"

cd "${repository_root}"
cargo build --locked --release --bin textify

mkdir -p "${dist_directory}"
rm -rf "${stage_directory}" "${deb_root}"
mkdir -p \
    "${stage_directory}/bin" \
    "${stage_directory}/share/applications" \
    "${stage_directory}/share/icons/hicolor/scalable/apps" \
    "${stage_directory}/share/metainfo"
install -m 0755 "${target_directory}/release/textify" "${stage_directory}/bin/textify"
install -m 0644 "${repository_root}/packaging/linux/com.shaun.textify.desktop" "${stage_directory}/share/applications/"
install -m 0644 "${repository_root}/packaging/Textify.svg" "${stage_directory}/share/icons/hicolor/scalable/apps/com.shaun.textify.svg"
install -m 0644 "${repository_root}/packaging/linux/com.shaun.textify.metainfo.xml" "${stage_directory}/share/metainfo/"
cp "${repository_root}/README.md" "${stage_directory}/"

tar -C "${dist_directory}" -czf "${dist_directory}/${package_name}.tar.gz" "${package_name}"

mkdir -p \
    "${deb_root}/DEBIAN" \
    "${deb_root}/usr/bin" \
    "${deb_root}/usr/share/applications" \
    "${deb_root}/usr/share/icons/hicolor/scalable/apps" \
    "${deb_root}/usr/share/metainfo"
sed \
    -e "s/@VERSION@/${version}/g" \
    -e "s/@ARCH@/${deb_arch}/g" \
    "${repository_root}/packaging/linux/control" > "${deb_root}/DEBIAN/control"
install -m 0755 "${target_directory}/release/textify" "${deb_root}/usr/bin/textify"
install -m 0644 "${repository_root}/packaging/linux/com.shaun.textify.desktop" "${deb_root}/usr/share/applications/"
install -m 0644 "${repository_root}/packaging/Textify.svg" "${deb_root}/usr/share/icons/hicolor/scalable/apps/com.shaun.textify.svg"
install -m 0644 "${repository_root}/packaging/linux/com.shaun.textify.metainfo.xml" "${deb_root}/usr/share/metainfo/"
dpkg-deb --root-owner-group --build "${deb_root}" "${dist_directory}/textify-${version}-linux-${deb_arch}.deb"

echo "Created ${dist_directory}/${package_name}.tar.gz"
echo "Created ${dist_directory}/textify-${version}-linux-${deb_arch}.deb"

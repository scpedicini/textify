#!/usr/bin/env bash
set -euo pipefail

script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "${script_directory}/.." && pwd)"
image_name="textify-linux-ci:local"
output_directory="${repository_root}/dist/linux-docker"

cd "${repository_root}"
docker build --platform linux/amd64 --build-arg RUST_TOOLCHAIN=1.93.1 -f packaging/linux/Dockerfile -t "${image_name}" .
container_id="$(docker create --platform linux/amd64 "${image_name}")"
trap 'docker rm -f "${container_id}" >/dev/null 2>&1 || true' EXIT
mkdir -p "${output_directory}"
docker cp "${container_id}:/workspace/dist/." "${output_directory}/"

echo "Copied Docker-verified Linux packages to ${output_directory}"

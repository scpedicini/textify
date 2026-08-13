#!/usr/bin/env bash
set -euo pipefail

if [[ "$(id -u)" -eq 0 ]]; then
    sudo_command=()
else
    sudo_command=(sudo)
fi

"${sudo_command[@]}" apt-get update
"${sudo_command[@]}" apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    clang \
    cmake \
    curl \
    dbus-x11 \
    dpkg-dev \
    file \
    fonts-dejavu-core \
    git \
    libasound2-dev \
    libclang-dev \
    libfontconfig1-dev \
    libssl-dev \
    libvulkan-dev \
    libwayland-dev \
    libx11-xcb-dev \
    libxcb1-dev \
    libxkbcommon-x11-dev \
    libzstd-dev \
    mesa-vulkan-drivers \
    pkg-config \
    vulkan-tools \
    xauth \
    xvfb

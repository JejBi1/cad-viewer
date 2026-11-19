#!/bin/bash
# One-time setup of the Linux build environment (Ubuntu 22.04, e.g. the WSL distro "cadbuild").
# Builds against glibc 2.35 -> the result runs on Ubuntu 22.04+, Debian 12+, Fedora 36+ ...
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

apt-get update -q
apt-get install -y -q --no-install-recommends \
    build-essential pkg-config cmake ninja-build ca-certificates curl wget file xz-utils \
    binutils desktop-file-utils \
    libwayland-client0 libwayland-dev libvulkan1 mesa-vulkan-drivers fonts-dejavu-core

# Rust toolchain (matching the Windows build environment)
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null; then
    curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi

# LibreDWG (dwg2dxf) from source, static-ish, installed into /opt/libredwg
if [ ! -x /opt/libredwg/bin/dwg2dxf ]; then
    cd /tmp
    curl -sSL -o libredwg.tar.xz https://github.com/LibreDWG/libredwg/releases/download/0.14.8601/libredwg-0.14.8601.tar.xz
    # sha256 of the official release asset; matches both GitHub's asset digest and the release's own dist.sha256
    # (verified 2026-11-11). A different hash = a tampered download: stop before building anything from it.
    echo "f1140a219d6edb291e55fad740be6c5e49c046fee558860575aeef110caa4ef9  libredwg.tar.xz" | sha256sum -c -
    rm -rf libredwg-0.14.8601 && tar xf libredwg.tar.xz
    cd libredwg-0.14.8601
    ./configure --prefix=/opt/libredwg --disable-bindings --disable-python --disable-docs \
                --disable-shared --enable-static --disable-werror -q
    make -j"$(nproc)" -s
    make install -s
fi

# appimagetool
if [ ! -x /opt/appimagetool ]; then
    curl -sSL -o /opt/appimagetool \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x /opt/appimagetool
fi

rustc --version
cargo --version
/opt/libredwg/bin/dwg2dxf --version | head -1
echo SETUP-DONE

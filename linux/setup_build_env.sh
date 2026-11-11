#!/bin/bash
# One-time setup of the Linux build environment (Ubuntu 22.04, e.g. the WSL distro "cadbuild").
# Builds against glibc 2.35 -> the result runs on Ubuntu 22.04+, Debian 12+, Fedora 36+ ...
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
HERE="$(cd "$(dirname "$0")" && pwd)"        # STEP_viewer/linux

apt-get update -q
apt-get install -y -q --no-install-recommends \
    python3 python3-venv python3-dev build-essential pkg-config ca-certificates curl wget file xz-utils \
    binutils patchelf desktop-file-utils \
    libgl1 libegl1 libglu1-mesa libopengl0 libfontconfig1 libfreetype6 libdbus-1-3 libglib2.0-0 \
    libxkbcommon0 libxkbcommon-x11-0 libxcb-cursor0 libxcb-icccm4 libxcb-image0 libxcb-keysyms1 \
    libxcb-randr0 libxcb-render-util0 libxcb-shape0 libxcb-xinerama0 libxcb-xkb1 libxcb-xfixes0 \
    libx11-xcb1 libxrender1 libxi6 libsm6 libice6 libxext6 libwayland-client0 libwayland-cursor0 libwayland-egl1 \
    libpcre2-8-0 libiconv-hook1 fonts-dejavu-core xvfb xauth libgl1-mesa-dri libglx-mesa0 x11-utils imagemagick

# Python 3.11 (OCP 8 wheels are cp311+; Ubuntu 22.04 ships 3.10) from deadsnakes
if ! command -v python3.11 >/dev/null; then
    apt-get install -y -q --no-install-recommends software-properties-common gpg-agent
    add-apt-repository -y ppa:deadsnakes/ppa
    apt-get update -q
    apt-get install -y -q --no-install-recommends python3.11 python3.11-venv python3.11-dev
fi

# Python build venv
if [ ! -x /opt/venv/bin/python3.11 ]; then
    rm -rf /opt/venv
    python3.11 -m venv /opt/venv
fi
/opt/venv/bin/pip install -q --upgrade pip
# versions = the Windows build venv (pip freeze); transitive dependencies (numpy, fonttools, pyparsing, ...) are
# not pinned - for a fully reproducible build use a requirements file with --require-hashes
/opt/venv/bin/pip install -q PySide6==6.11.2 ezdxf==1.4.4 pillow==12.3.0 pyinstaller==6.22.3

# 1.9: slim OCP instead of cadquery-ocp (+ VTK): Open CASCADE 8.0.1 (conda-forge, no VTK) with the 73 binding modules the
# program uses, import names OCP + OCP.libs, prebuilt in native/slimocp (README.md there: how to rebuild it).
# The stock wheel and the slim one both provide the package OCP -> the stock one (and vtk) must not be installed.
/opt/venv/bin/pip uninstall -y -q cadquery-ocp cadquery-ocp-proxy vtk 2>/dev/null || true
SLIM=$(ls "$HERE"/../native/slimocp/wheels/cadquery_ocp_slim-*-linux_x86_64.whl | head -1)
test -f "$SLIM" || { echo "slim OCP wheel missing in native/slimocp/wheels" >&2; exit 1; }
/opt/venv/bin/pip install -q --no-deps --force-reinstall "$SLIM"

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

# ---- rdxf begin
# Native module native/rdxf (Rust + PyO3; 1.9: z-order batching of the 2D renderer). The prebuilt wheel in
# native/rdxf/wheels (abi3, manylinux_2_34 = built on glibc 2.35) is installed as it is - no compiler needed. Only when it
# is missing is it built: that needs the Rust toolchain and maturin; maturin lives in its own venv, so it never ends up
# in the release venv or the bundle; build_linux.sh then stores the new wheel in native/rdxf/wheels. Without the module
# the program still works (a Python fallback is used, slower for drawings above ~15 000 entities), but a release build
# must have it.
RDXF=$(ls "$HERE"/../native/rdxf/wheels/rdxf-*-manylinux*_x86_64.whl 2>/dev/null | head -1 || true)
if [ -n "$RDXF" ]; then
    /opt/venv/bin/pip install -q --no-deps --force-reinstall "$RDXF"
else
    RUST_VERSION=1.99.1
    MATURIN_VERSION=1.16.0
    export PATH="$HOME/.cargo/bin:$PATH"
    if ! command -v cargo >/dev/null; then
        curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION"
    fi
    if [ ! -x /opt/venv-rust/bin/maturin ]; then
        rm -rf /opt/venv-rust
        python3.11 -m venv /opt/venv-rust
        /opt/venv-rust/bin/pip install -q "maturin==$MATURIN_VERSION"
    fi
    bash "$HERE/../native/rdxf/build_linux.sh"
fi
# ---- rdxf end

# appimagetool
if [ ! -x /opt/appimagetool ]; then
    curl -sSL -o /opt/appimagetool \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x /opt/appimagetool
fi

# the slim OCP needs the newer libstdc++ of OCP.libs process-wide (conda OCCT: CXXABI_1.3.15); the frozen bundle gets it
# from build_linux.sh, in the venv it has to be preloaded
SPK=$(/opt/venv/bin/python -c "import sysconfig; print(sysconfig.get_paths()['purelib'])")
LD_PRELOAD="$SPK/OCP.libs/libstdc++.so.6:$SPK/OCP.libs/libgcc_s.so.1" \
    /opt/venv/bin/python -c "import OCP, PySide6, ezdxf, rdxf; print('python ok', OCP.__version__, PySide6.__version__, ezdxf.__version__, 'rdxf API', rdxf.API)"
/opt/libredwg/bin/dwg2dxf --version | head -1
echo SETUP-DONE

#!/bin/bash
# Builds the rdxf wheel (Rust + PyO3, abi3) in the Linux build environment, copies it to native/rdxf/wheels/ and installs it
# into /opt/venv (linux/setup_build_env.sh runs this only when no prebuilt wheel is in native/rdxf/wheels/):
#   wsl -d cadbuild -u root -- bash "/mnt/c/.../STEP_viewer/native/rdxf/build_linux.sh"
# Needs rustup/cargo and /opt/venv-rust (maturin, pinned) - both are set up by linux/setup_build_env.sh.
# The build runs in /tmp (the source folder is not written to; a build on /mnt/c would also be slow).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
PY=${PY:-/opt/venv/bin/python}                 # the interpreter of the release venv (python3.11, glibc 2.35 host)
MATURIN=${MATURIN:-/opt/venv-rust/bin/maturin}
export CARGO_TARGET_DIR=/tmp/rdxf-target
SRC=/tmp/rdxf-src
WHEELS=/tmp/rdxf-wheels
command -v cargo >/dev/null || { echo "cargo not found - run linux/setup_build_env.sh first" >&2; exit 1; }
test -x "$MATURIN" || { echo "$MATURIN missing - run linux/setup_build_env.sh first" >&2; exit 1; }
rm -rf "$SRC" "$WHEELS"; mkdir -p "$SRC" "$WHEELS"
cp -r "$HERE/src" "$HERE/Cargo.toml" "$HERE/Cargo.lock" "$HERE/pyproject.toml" "$SRC/"
cd "$SRC"
"$MATURIN" build --release --locked -o "$WHEELS" -i "$PY"
mkdir -p "$HERE/wheels" && cp -f "$WHEELS"/rdxf-*.whl "$HERE/wheels/"      # prebuilt wheel kept in the project (linux/setup_build_env.sh installs it)
"$PY" -m pip install -q --force-reinstall --no-deps "$WHEELS"/rdxf-*.whl
"$PY" -c "import rdxf; print('rdxf OK, API', rdxf.API)"

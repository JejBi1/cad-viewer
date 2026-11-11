#!/bin/bash
# Integration test of the slim OCP in the project's real PyInstaller recipe (linux/build_linux.sh), in a private work dir.
# usage: integ_linux.sh stock|slim     (copy of the project's linux/build_linux.sh pyinstaller + prune steps; output: /root/slimocp/integ/<variant>/)
set -euo pipefail
V=$1
SRC="$(cd "$(dirname "$0")/../../.." && pwd)"
W=/root/slimocp/integ/$V
rm -rf "$W"; mkdir -p "$W"
cp -r "$SRC/stepview" "$SRC/linux" "$W/"
cd "$W"
if [ "$V" = slim ]; then export PYTHONPATH=/root/slimocp/out/linux; else unset PYTHONPATH; fi
/opt/venv/bin/python -c "import OCP; print('OCP =', OCP.__file__)"
/opt/venv/bin/pyinstaller stepview/main.py \
    --name cad-viewer --noconfirm --clean --windowed --log-level WARN \
    --distpath "$W/dist" --workpath "$W/work" --specpath "$W" \
    --paths stepview \
    --add-data "stepview/stepview.png:." \
    --hidden-import core --hidden-import viewer3d --hidden-import view2d --hidden-import thumbs --hidden-import analysis --hidden-import materials --hidden-import collisions --hidden-import meshmeasure \
    --hidden-import fastscene --hidden-import qtbuild --hidden-import rdxf \
    --collect-data ezdxf \
    --add-binary "/opt/libredwg/bin/dwg2dxf:libredwg" \
    --exclude-module tkinter --exclude-module matplotlib --exclude-module PyQt5 --exclude-module PyQt6 \
    --exclude-module PySide6.QtWebEngineCore --exclude-module PySide6.QtQml --exclude-module PySide6.QtQuick \
    --exclude-module PySide6.QtPdf --exclude-module PySide6.QtNetwork \
    --exclude-module PySide6.QtQuickWidgets --exclude-module PySide6.QtOpenGL --exclude-module PySide6.QtOpenGLWidgets \
    --exclude-module PySide6.QtSvg --exclude-module PySide6.QtSvgWidgets \
    --exclude-module ssl --exclude-module _ssl --exclude-module _hashlib \
    --exclude-module PIL._avif --exclude-module PIL._webp --exclude-module PIL._imagingft \
    --exclude-module PIL._imagingcms --exclude-module PIL._imagingtk
APP="$W/dist/cad-viewer"
test -x "$APP/cad-viewer"
du -s -B1M --apparent-size "$APP" | sed 's/^/bundle before prune (MB): /'
if [ "$V" = slim ]; then
  # NEW STEP: the whole process must use the gcc-15 C++ runtime that the conda OCCT libraries were built with
  cp -f /root/slimocp/out/linux/OCP.libs/libstdc++.so.6 "$APP/_internal/libstdc++.so.6"
  cp -f /root/slimocp/out/linux/OCP.libs/libgcc_s.so.1  "$APP/_internal/libgcc_s.so.1"
fi
/opt/venv/bin/python "$W/linux/prune_bundle.py" "$APP"
du -s -B1M --apparent-size "$APP" | sed 's/^/bundle after prune (MB): /'
du -s -B1M "$APP" | sed 's/^/bundle after prune, disk usage incl. holes (MB): /'
echo "INTEG-DONE $V"

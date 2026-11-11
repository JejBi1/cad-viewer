#!/bin/bash
# Build the Linux version (run inside the Ubuntu 22.04 build environment, see setup_build_env.sh):
#   wsl -d cadbuild -u root -- bash "/mnt/c/.../STEP_viewer/linux/build_linux.sh"
# Outputs into STEP_viewer/dist/linux/:
#   cad-viewer-<ver>-x86_64.AppImage        single file, just run it
#   cad-viewer-<ver>-linux-x86_64.tar.gz    folder + install.sh (menu, MIME, file associations, thumbnails)
# 1.8: the PyInstaller folder is slimmed by linux/prune_bundle.py (unused Qt plug-ins / libraries, GTK stack, symbols,
# zero padding of the wheels' libraries); ssl and the Pillow codecs the program doesn't use are left out.
# 1.9: OCP is the slim build from native/slimocp (no VTK, 73 binding modules; installed by setup_build_env.sh).
set -euo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"
VER=$(grep -oP '^VERSION = "\K[^"]+' "$SRC/stepview/main.py")
W=/tmp/cadviewer-build          # build on the Linux FS (fast, no SynologyDrive locking)
OUT="$SRC/dist/linux"
# 1.9: the 2D scene builder needs the native rdxf module (native/rdxf, built by linux/setup_build_env.sh)
/opt/venv/bin/python -c "import rdxf" || { echo "rdxf missing in /opt/venv - run native/rdxf/build_linux.sh" >&2; exit 1; }
rm -rf "$W"; mkdir -p "$W" "$OUT"
cp -r "$SRC/stepview" "$SRC/linux" "$W/"
cd "$W"

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
# 1.9 slim OCP: the conda-forge Open CASCADE needs the gcc 15 C++ runtime (CXXABI_1.3.15), and two libstdc++ in one process
# crash - so the one from OCP.libs must be THE runtime: it replaces the copy PyInstaller took from the build host
SP=$(/opt/venv/bin/python -c "import sysconfig; print(sysconfig.get_paths()['purelib'])")
cp -f "$SP/OCP.libs/libstdc++.so.6" "$SP/OCP.libs/libgcc_s.so.1" "$APP/_internal/"
du -s -B1M --apparent-size "$APP" | sed 's/^/bundle before prune (MB): /'
/opt/venv/bin/python "$W/linux/prune_bundle.py" "$APP"
test -x "$APP/cad-viewer"

# ---- tar.gz with installer
PKG="$W/cad-viewer-$VER"
mkdir -p "$PKG"
cp -a "$APP" "$PKG/cad-viewer"
cp linux/install.sh linux/cad-viewer.desktop linux/cad-viewer-mime.xml "$PKG/"
cp stepview/stepview.png "$PKG/cad-viewer.png"
mkdir -p "$PKG/licenses" && cp "$SRC/installer/licenses/"* "$PKG/licenses/"
cat > "$PKG/README.txt" <<EOF
CAD prohlížeč $VER pro Linux (x86_64) – STEP, IGES, STL (3D), DXF, DWG (2D)

Spuštění bez instalace:      ./cad-viewer/cad-viewer [soubor]
Instalace pro uživatele:     ./install.sh            (nabídka aplikací, typy souborů, náhledy KDE/XFCE)
Instalace pro všechny:       sudo ./install.sh --system   (náhledy i v GNOME/Nautilus)
Výchozí program pro soubory: přidej --default
Odinstalace:                 ./install.sh --uninstall  (resp. sudo ./install.sh --system --uninstall)

Potřebuje: glibc 2.35+ (Ubuntu 22.04+, Debian 12+, Fedora 36+, …) a OpenGL (Mesa / ovladač GPU).
Open CASCADE (štíhlé OCP) i s C++ během (libstdc++ z gcc 15) je přibalený, nic dalšího se neinstaluje.
Na Waylandu běží přes XWayland (QT_QPA_PLATFORM=xcb) – 3D jádro OCCT kreslí do X11 okna.
EOF
chmod +x "$PKG/install.sh"
# --sparse: the OCCT libraries of the wheels contain long runs of zeros (kept as holes, see prune_bundle.py)
tar -C "$W" --sparse -czf "$W/cad-viewer-$VER-linux-x86_64.tar.gz" "cad-viewer-$VER"

# ---- AppImage
AD="$W/AppDir"
mkdir -p "$AD/usr/lib"
cp -a "$APP" "$AD/usr/lib/cad-viewer"
cp linux/cad-viewer.desktop "$AD/cad-viewer.desktop"
cp stepview/stepview.png "$AD/cad-viewer.png"
cat > "$AD/AppRun" <<'EOF'
#!/bin/bash
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/lib/cad-viewer/cad-viewer" "$@"
EOF
chmod +x "$AD/AppRun"
ARCH=x86_64 /opt/appimagetool --appimage-extract-and-run --no-appstream "$AD" "$W/cad-viewer-$VER-x86_64.AppImage" >/dev/null
chmod +x "$W/cad-viewer-$VER-x86_64.AppImage"
cp "$W/cad-viewer-$VER-x86_64.AppImage" "$W/cad-viewer-$VER-linux-x86_64.tar.gz" "$OUT/"

ls -la "$OUT"
echo "bundle: $(du -sh --apparent-size "$APP" | cut -f1) apparent, $(du -sh "$APP" | cut -f1) on disk"
echo BUILD-DONE

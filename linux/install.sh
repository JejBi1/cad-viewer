#!/bin/bash
# CAD prohlížeč – instalace na Linux
#
#   ./install.sh                 instalace pro aktuálního uživatele (~/.local), bez sudo
#   sudo ./install.sh --system   instalace pro všechny (/usr/local) – náhledy fungují i v GNOME (Nautilus),
#                                který generátory náhledů spouští v sandboxu bez přístupu do domovské složky
#   přidej --default             nastaví CAD prohlížeč jako výchozí program pro STEP/IGES/STL/DXF/DWG
#   přidej --dwg-thumbs          zapne i náhledy DWG (převodník LibreDWG; ve výchozím stavu vypnuto – náhledy se
#                                dělají automaticky pro každý soubor ve složce, viz README, kapitola Bezpečnost)
#   ./install.sh --uninstall     (resp. sudo ./install.sh --system --uninstall) odinstaluje
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SYSTEM=0; DEFAULT=0; UNINSTALL=0; DWG_THUMBS=0
for a in "$@"; do
    case "$a" in
        --system) SYSTEM=1 ;;
        --default) DEFAULT=1 ;;
        --dwg-thumbs) DWG_THUMBS=1 ;;
        --uninstall) UNINSTALL=1 ;;
        -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
        *) echo "Neznámý parametr: $a"; exit 1 ;;
    esac
done

if [ $SYSTEM = 1 ]; then
    [ "$(id -u)" = 0 ] || { echo "Systémová instalace potřebuje sudo: sudo $0 $*"; exit 1; }
    APPDIR=/usr/local/lib/cad-viewer; BIN=/usr/local/bin; SHARE=/usr/local/share
    THUMBDIR=/usr/share/thumbnailers      # gnome-desktop looks in XDG_DATA_DIRS, /usr/share is always there
else
    APPDIR="$HOME/.local/lib/cad-viewer"; BIN="$HOME/.local/bin"; SHARE="$HOME/.local/share"
    THUMBDIR="$SHARE/thumbnailers"
fi
# types the program opens (.desktop, xdg-mime) - DWG included
TYPES="model/step model/iges model/stl application/sla image/vnd.dxf image/vnd.dwg"
# types that get an automatic thumbnail: DWG only on request (LibreDWG is a C parser fed with whatever file a
# file manager happens to display)
THUMB_TYPES="model/step model/iges model/stl application/sla image/vnd.dxf"
[ $DWG_THUMBS = 1 ] && THUMB_TYPES="$THUMB_TYPES image/vnd.dwg"

refresh() {
    command -v update-mime-database >/dev/null && update-mime-database "$SHARE/mime" >/dev/null 2>&1 || true
    command -v update-desktop-database >/dev/null && update-desktop-database "$SHARE/applications" >/dev/null 2>&1 || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$SHARE/icons/hicolor" >/dev/null 2>&1 || true
}

if [ $UNINSTALL = 1 ]; then
    rm -rf "$APPDIR"
    rm -f "$BIN/cad-viewer" "$SHARE/applications/cad-viewer.desktop" "$SHARE/mime/packages/cad-viewer.xml" \
          "$SHARE/icons/hicolor/256x256/apps/cad-viewer.png" "$THUMBDIR/cad-viewer.thumbnailer"
    rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/cad-viewer"
    refresh
    echo "CAD prohlížeč odinstalován."
    exit 0
fi

echo "Instaluji do $APPDIR …"
rm -rf "$APPDIR"
mkdir -p "$APPDIR" "$BIN" "$SHARE/applications" "$SHARE/mime/packages" "$SHARE/icons/hicolor/256x256/apps" "$THUMBDIR"
# the tarball carries the builder's uid/gid: never keep them (as root they would make the program directory,
# which root later runs and which the thumbnailer starts automatically, owned and writable by a stranger)
cp -a --no-preserve=ownership "$HERE/cad-viewer/." "$APPDIR/"
if [ $SYSTEM = 1 ]; then
    chown -R root:root "$APPDIR"
    chmod -R go-w "$APPDIR"
fi
ln -sf "$APPDIR/cad-viewer" "$BIN/cad-viewer"
sed "s|^Exec=.*|Exec=$APPDIR/cad-viewer %f|" "$HERE/cad-viewer.desktop" > "$SHARE/applications/cad-viewer.desktop"
cp "$HERE/cad-viewer-mime.xml" "$SHARE/mime/packages/cad-viewer.xml"
cp "$HERE/cad-viewer.png" "$SHARE/icons/hicolor/256x256/apps/cad-viewer.png"
# the thumbnailer runs on every file shown in a folder: wall-clock limit (timeout) and address-space / CPU limits
# (prlimit) from the system tools, when they exist (absolute paths - the file manager's PATH may differ);
# the program also sets its own rlimits in --thumbnail mode.
# Address space 4 GB, not 2 GB: measured on Linux, an ordinary STEP part needs 2.25 GB of VIRTUAL memory (OCCT / TBB
# threads reserve a malloc arena each; RSS is only 0.5 GB), so a 2 GB limit made every STEP thumbnail fail.
WRAP=""
TIMEOUT_BIN="$(command -v timeout || true)"
PRLIMIT_BIN="$(command -v prlimit || true)"
[ -n "$TIMEOUT_BIN" ] && WRAP="$TIMEOUT_BIN -k 5 60 "
[ -n "$PRLIMIT_BIN" ] && WRAP="$WRAP$PRLIMIT_BIN --as=4000000000 --cpu=120 "
cat > "$THUMBDIR/cad-viewer.thumbnailer" <<EOF
[Thumbnailer Entry]
TryExec=$APPDIR/cad-viewer
Exec=${WRAP}$APPDIR/cad-viewer --thumbnail %i %o %s
MimeType=${THUMB_TYPES// /;};
EOF
refresh

if [ $DEFAULT = 1 ] && command -v xdg-mime >/dev/null; then
    for t in $TYPES; do xdg-mime default cad-viewer.desktop "$t"; done
    echo "Nastaveno jako výchozí program pro STEP/IGES/STL/DXF/DWG."
fi

echo "Hotovo. Spuštění: cad-viewer (nebo z nabídky aplikací „CAD prohlížeč“)."
if [ $SYSTEM = 0 ]; then
    case ":$PATH:" in *":$BIN:"*) ;; *) echo "Pozn.: $BIN není v PATH – spouštěj z nabídky aplikací nebo přes $APPDIR/cad-viewer";; esac
    echo "Náhledy ve správci souborů: KDE/XFCE/Caja/Nemo fungují hned; GNOME (Nautilus) je potřebuje systémovou instalaci (sudo $0 --system)."
fi
if [ $DWG_THUMBS = 1 ]; then
    echo "Náhledy DWG zapnuty (--dwg-thumbs): převodník LibreDWG zpracuje každý zobrazený soubor DWG."
else
    echo "Náhledy DWG jsou vypnuté (soubory DWG se otevírají normálně); zapnout: $0 --dwg-thumbs"
fi
[ -z "$TIMEOUT_BIN" ] && echo "Pozn.: příkaz timeout nenalezen – náhledy nemají časový limit od systému (program si hlídá CPU čas sám)."
[ -z "$PRLIMIT_BIN" ] && echo "Pozn.: příkaz prlimit (util-linux) nenalezen – paměťový limit náhledů hlídá jen program sám."
echo "Staré náhledy se obnoví po smazání ~/.cache/thumbnails (rm -rf ~/.cache/thumbnails/*)."

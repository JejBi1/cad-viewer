"""Build dist/StepViewer/StepViewer.exe with PyInstaller.

Run with the build venv (kept outside SynologyDrive, ~1.3 GB):
    %USERPROFILE%\\.venvs\\stepviewer\\Scripts\\python.exe build.py [--installer]
Create it with:
    python -m venv %USERPROFILE%\\.venvs\\stepviewer
    %USERPROFILE%\\.venvs\\stepviewer\\Scripts\\pip install cadquery-ocp PySide6 pyinstaller pefile

The folder is built and pruned in the (not synchronised) build directory and copied to dist/StepViewer at the end -
the Synology Drive client would lock files that are being written or deleted.
"""
import glob
import os
import shutil
import subprocess
import sys
import time

import pefile
import PyInstaller.__main__

HERE = os.path.dirname(os.path.abspath(__file__))
SP = os.path.join(sys.prefix, "Lib", "site-packages")
BUILD = r"C:/slimocp_w/integ/buildA"
STAGE = os.path.join(BUILD, "dist")                                           # built + pruned here, then copied
LIB_DIRS = ["cadquery_ocp.libs", "vtk.libs"]   # OCP/__init__ adds both as DLL directories

# ---- 1.8 install-size diet ------------------------------------------------------------------------------------
# Python modules that nothing in the program uses but PyInstaller pulls in: ssl (+ libcrypto / libssl, 6 MB; hashlib
# falls back to its built-in SHA-1), Pillow codecs for AVIF / WebP / FreeType / littlecms / Tk (ezdxf only needs
# the core and PNG), Qt modules the program never imports.
EXCLUDE_MODULES = (
    "matplotlib", "vtk", "vtkmodules", "tkinter", "PyQt5", "PyQt6",
    "PySide6.QtNetwork", "PySide6.QtQml", "PySide6.QtQuick", "PySide6.QtQuickWidgets", "PySide6.QtWebEngineCore",
    "PySide6.QtPdf", "PySide6.QtOpenGL", "PySide6.QtOpenGLWidgets", "PySide6.QtSvg", "PySide6.QtSvgWidgets",
    "ssl", "_ssl", "_hashlib",
    "PIL._avif", "PIL._webp", "PIL._imagingft", "PIL._imagingcms", "PIL._imagingtk",
)
# Qt: the program uses QtCore / QtGui / QtWidgets only, with raster painting (OCCT makes its own OpenGL context, so
# opengl32sw.dll - Mesa software GL for Qt Quick / QOpenGLWidget - is never used) and no translations (all texts are
# in the program). Everything else PyInstaller's PySide6 hook copies is removed after the build; unresolved_imports()
# then makes sure no remaining DLL lost a dependency.
QT_KEEP_DLLS = {"qt6core.dll", "qt6gui.dll", "qt6widgets.dll"}
QT_KEEP_PLUGINS = {                       # folder -> file names (lower case); None = whole folder
    "platforms": {"qwindows.dll", "qoffscreen.dll"},       # window system; off-screen for --thumbnail
    "styles": None,                                        # Windows 11 style
    "imageformats": {"qico.dll", "qjpeg.dll", "qgif.dll", "qtiff.dll"},   # app icon (.ico), raster images in DXF
}
PRUNE_FILES = ("PySide6/opengl32sw.dll",)
PRUNE_DIRS = ("PySide6/translations",)
SOURCE_SUFFIXES = (".c", ".h", ".pyx", ".pxd")            # ezdxf/acc ships the Cython sources next to the .pyd


def ocp_dll_closure():
    """DLLs from the wheel lib folders that OCP.pyd really needs (direct + transitive)."""
    index = {}
    for d in LIB_DIRS:
        for f in os.listdir(os.path.join(SP, d)):
            index[f.lower()] = (d, os.path.join(SP, d, f))
    need, todo, seen = {}, [os.path.join(SP, "OCP", "OCP.cp311-win_amd64.pyd")], set()
    dirs = [pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"],
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT"]]
    while todo:
        p = todo.pop()
        if p in seen:
            continue
        seen.add(p)
        pe = pefile.PE(p, fast_load=True)
        pe.parse_data_directories(directories=dirs)
        for attr in ("DIRECTORY_ENTRY_IMPORT", "DIRECTORY_ENTRY_DELAY_IMPORT"):
            for e in getattr(pe, attr, []):
                n = e.dll.decode().lower()
                if n in index and n not in need:
                    need[n] = index[n]
                    todo.append(index[n][1])
        pe.close()
    return list(need.values())


def tree_size(path):
    return sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(path) for f in fs)


def unresolved_imports(root):
    """{(file, dll)} - DLLs imported by a binary in the folder that are neither in the folder nor in System32."""
    sysdir = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "System32")
    have = {f.lower() for _, _, fs in os.walk(root) for f in fs}
    dirs = [pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]]       # delay-load imports are optional by design
    out = set()
    for d, _, fs in os.walk(root):
        for f in fs:
            if not f.lower().endswith((".dll", ".pyd", ".exe")):
                continue
            pe = pefile.PE(os.path.join(d, f), fast_load=True)
            pe.parse_data_directories(directories=dirs)
            for e in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []):
                n = e.dll.decode().lower()
                if n not in have and not os.path.exists(os.path.join(sysdir, n)):
                    out.add((os.path.relpath(os.path.join(d, f), root), n))
            pe.close()
    return out


def prune(dist):
    """Remove what the program never loads (see the lists at the top). Returns the number of bytes freed."""
    internal = os.path.join(dist, "_internal")
    freed = 0

    def rm(path):
        nonlocal freed
        if os.path.isdir(path):
            freed += tree_size(path)
            shutil.rmtree(path)
        elif os.path.exists(path):
            freed += os.path.getsize(path)
            os.remove(path)

    for rel in PRUNE_FILES + PRUNE_DIRS:
        rm(os.path.join(internal, *rel.split("/")))
    qt = os.path.join(internal, "PySide6")
    for f in os.listdir(qt):
        if f.lower().startswith("qt6") and f.lower().endswith(".dll") and f.lower() not in QT_KEEP_DLLS:
            rm(os.path.join(qt, f))
    plug = os.path.join(qt, "plugins")
    for sub in os.listdir(plug):
        keep = QT_KEEP_PLUGINS.get(sub, set())
        if keep is None:
            continue
        for f in os.listdir(os.path.join(plug, sub)):
            if f.lower() not in keep:
                rm(os.path.join(plug, sub, f))
        if not os.listdir(os.path.join(plug, sub)):
            os.rmdir(os.path.join(plug, sub))
    for f in glob.glob(os.path.join(internal, "ezdxf", "acc", "*")):
        if f.lower().endswith(SOURCE_SUFFIXES):
            rm(f)
    # PyInstaller 6 also analyses DLLs / exes added as data: libredwg-0.dll (31 MB) and libiconv-2.dll were copied to
    # the top folder as well. dwg2dxf.exe loads them from its own folder, nothing else needs the copies.
    lib = os.path.join(internal, "libredwg")
    for f in os.listdir(lib):
        top = os.path.join(internal, f)
        if f.lower().endswith(".dll") and os.path.isfile(top) and os.path.getsize(top) == os.path.getsize(os.path.join(lib, f)):
            rm(top)
    return freed


def publish(stage_dir, final_dir):
    """Replace dist/StepViewer by the finished folder (retries: the sync client may hold a file for a moment)."""
    for attempt in range(6):
        try:
            if os.path.isdir(final_dir):
                shutil.rmtree(final_dir)
            shutil.copytree(stage_dir, final_dir)
            return
        except OSError as ex:
            print("  copy to dist failed (%s), retrying..." % ex)
            time.sleep(5)
    raise SystemExit("could not replace " + final_dir)


def main():
    try:     # native module of the 2D renderer (native/rdxf): a release without it would silently use the slower Python fallback
        import rdxf  # noqa: F401
    except ImportError:
        raise SystemExit("rdxf is not installed in this venv - run native/rdxf/build_win.bat first")
    deps = ocp_dll_closure()
    print(f"OCP needs {len(deps)} DLLs from {LIB_DIRS}")
    args = [
        os.path.join(HERE, "stepview", "main.py"),
        "--name", "StepViewer",
        "--noconfirm", "--clean", "--windowed",
        "--icon", os.path.join(HERE, "stepview", "stepview.ico"),
        "--add-data", os.path.join(HERE, "stepview", "stepview.ico") + os.pathsep + ".",
        "--paths", os.path.join(HERE, "stepview"),
        "--distpath", STAGE,
        "--workpath", BUILD,
        "--specpath", BUILD,
        "--hidden-import", "core", "--hidden-import", "viewer3d",
        "--hidden-import", "view2d", "--hidden-import", "thumbs", "--hidden-import", "thumbsettings", "--hidden-import", "analysis", "--hidden-import", "materials", "--hidden-import", "collisions", "--hidden-import", "meshmeasure", "--hidden-import", "spacemouse",
        "--hidden-import", "fastscene", "--hidden-import", "qtbuild", "--hidden-import", "rdxf",     # 1.9 2D scene builder (+ native/rdxf)
        "--collect-data", "ezdxf",
        # DWG -> DXF converter (LibreDWG, GPLv3, separate program); only dwg2dxf.exe - the other tools in the folder
        # (dwgbmp.exe, ...) are never started by the program and would only add attack surface.
        # PyInstaller also copies libredwg-0.dll (31 MB) and libiconv-2.dll to the top folder; prune() removes those
        # copies, because dwg2dxf.exe loads its DLLs from its own folder (_internal\libredwg).
        "--add-data", os.path.join(HERE, "third_party", "libredwg", "dwg2dxf.exe") + os.pathsep + "libredwg",
        "--add-data", os.path.join(HERE, "third_party", "libredwg", "*.dll") + os.pathsep + "libredwg",
    ]
    for mod in EXCLUDE_MODULES:
        args += ["--exclude-module", mod]
    for d, p in deps:
        args += ["--add-binary", p + os.pathsep + d]
    PyInstaller.__main__.run(args)
    dist = os.path.join(STAGE, "StepViewer")
    before = tree_size(dist)
    unresolved_before = unresolved_imports(dist)
    freed = prune(dist)
    print(f"prune: {freed / 1e6:.1f} MB removed ({before / 1e6:.1f} -> {tree_size(dist) / 1e6:.1f} MB)")
    lost = sorted(unresolved_imports(dist) - unresolved_before)
    if lost:
        for f, n in lost:
            print(f"  MISSING after prune: {f} needs {n}")
        raise SystemExit("prune() removed a DLL that is still needed")
    # make sure both lib folders exist even if empty (os.add_dll_directory needs them)
    for d in LIB_DIRS:
        os.makedirs(os.path.join(dist, "_internal", d), exist_ok=True)
    # Explorer thumbnail handler (.NET Framework 4 COM server) next to the exe
    csc = os.path.join(os.environ["WINDIR"], "Microsoft.NET", "Framework64", "v4.0.30319", "csc.exe")
    subprocess.run([csc, "/nologo", "/target:library", "/platform:x64", "/optimize",
                    "/out:" + os.path.join(dist, "StepThumb.dll"),
                    os.path.join(HERE, "shellext", "StepThumb.cs"), os.path.join(HERE, "shellext", "Sandbox.cs"),
                    "/r:System.Drawing.dll"], check=True)
    final = os.path.join(HERE, "distA", "StepViewer")
    os.makedirs(os.path.dirname(final), exist_ok=True)
    publish(dist, final)
    print("done:", os.path.join(final, "StepViewer.exe"), f"({tree_size(final) / 1e6:.1f} MB)")
    if "--installer" in sys.argv:
        # compile outside SynologyDrive (the sync client locks the half-written exe), then move it
        iscc = os.path.join(os.environ["LOCALAPPDATA"], "Programs", "Inno Setup 6", "ISCC.exe")
        out = os.path.join(BUILD, "installer")
        os.makedirs(out, exist_ok=True)
        subprocess.run([iscc, "/Q", "/O" + out, os.path.join(HERE, "installer", "StepViewer.iss")], check=True)
        for f in glob.glob(os.path.join(out, "*.exe")):
            shutil.move(f, os.path.join(HERE, "dist", os.path.basename(f)))
            print("installer:", os.path.join(HERE, "dist", os.path.basename(f)))


if __name__ == "__main__":
    sys.exit(main())

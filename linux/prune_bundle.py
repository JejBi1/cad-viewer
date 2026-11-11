#!/usr/bin/env python3
"""Install-size diet of the PyInstaller bundle (1.8): python3 prune_bundle.py <dist/cad-viewer folder>

What goes (nothing the program loads is touched):
  * Qt plug-ins / translations the program never uses - it needs QtCore / QtGui / QtWidgets with the xcb platform
    (OCCT renders into a native X11 window; no Qt Quick, no QOpenGLWidget), the off-screen platform for --thumbnail,
    and a few image formats; no Wayland / EGLFS / linuxfb / evdev plug-ins, no GTK platform theme;
  * every bundled shared library that nothing which stays needs any more (Qt Quick / Qml / Pdf / Svg / Network /
    Wayland ..., and the GTK 3 stack with its ~60 dependencies that only the GTK theme plug-in pulled in);
  * the Cython sources (.c / .pyx) that ezdxf ships next to its compiled modules;
  * symbol tables of all shared libraries (strip --strip-unneeded; the wheels' VTK / OpenBLAS / dwg2dxf carry them);
  * then all-zero blocks are turned into file holes (fallocate --dig-holes): the wheels' patched OCCT libraries are
    padded with megabytes of zeros; the tar.gz is made with --sparse so that they stay holes after installation.
A check at the end verifies that no remaining file lost a DLL / shared-library dependency.
"""
import os
import re
import shutil
import subprocess
import sys

KEEP_PLUGINS = {                        # folder -> file names; None = whole folder
    "platforms": {"libqxcb.so", "libqoffscreen.so"},
    "xcbglintegrations": None,
    "platforminputcontexts": {"libcomposeplatforminputcontextplugin.so", "libibusplatforminputcontextplugin.so"},
    "platformthemes": {"libqxdgdesktopportal.so"},
    "imageformats": {"libqico.so", "libqjpeg.so", "libqgif.so", "libqtiff.so"},
}
SOURCE_SUFFIXES = (".c", ".h", ".pyx", ".pxd")
PROTECTED = re.compile(r"^libpython")      # dlopen-ed by the PyInstaller bootloader, not a DT_NEEDED of anything


def is_elf(path):
    try:
        with open(path, "rb") as f:
            return f.read(4) == b"\x7fELF"
    except OSError:
        return False


def needed(path):
    out = subprocess.run(["readelf", "-d", path], capture_output=True, text=True).stdout
    return re.findall(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]", out)


def tree_size(path):
    return sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(path) for f in fs
               if not os.path.islink(os.path.join(d, f)))


def system_libs():
    out = subprocess.run(["ldconfig", "-p"], capture_output=True, text=True).stdout
    return {m.group(1) for m in re.finditer(r"^\s*(\S+) \(", out, re.M)}


def scan(root):
    """(by_name {soname: {realpath}}, elfs [realpath])"""
    by_name, elfs = {}, []
    for d, _, fs in os.walk(root):
        for f in fs:
            p = os.path.join(d, f)
            if os.path.islink(p) or not is_elf(p):
                continue
            elfs.append(p)
    for p in elfs:
        by_name.setdefault(os.path.basename(p), set()).add(p)
    for d, _, fs in os.walk(root):              # root-level symlinks carry the soname the loader asks for
        for f in fs:
            p = os.path.join(d, f)
            if os.path.islink(p) and os.path.exists(p):
                rp = os.path.realpath(p)
                if rp in by_name.get(os.path.basename(rp), ()):
                    by_name.setdefault(f, set()).add(rp)
    return by_name, elfs


def unresolved(root):
    """{(file, lib)} - DT_NEEDED entries of bundled files found neither in the bundle nor on this system."""
    by_name, elfs = scan(root)
    sysl = system_libs()
    return {(os.path.relpath(p, root), n) for p in elfs for n in needed(p) if n not in by_name and n not in sysl}


def loadable(path):
    """Every PT_LOAD segment keeps file offset congruent to its address modulo its alignment (glibc checks this)."""
    out = subprocess.run(["readelf", "-lW", path], capture_output=True, text=True).stdout
    for line in out.splitlines():
        t = line.split()
        if t and t[0] == "LOAD" and len(t) >= 8:
            off, va, align = int(t[1], 16), int(t[2], 16), int(t[-1], 16)
            if align > 1 and (va - off) % align:
                return False
    return True


def main(root):
    internal = os.path.join(root, "_internal")
    qt = os.path.join(internal, "PySide6", "Qt")
    before = tree_size(root)
    bad_before = unresolved(root)

    # ---- Qt plug-ins, translations, everything but lib / plugins
    for sub in os.listdir(qt):
        if sub not in ("lib", "plugins"):
            shutil.rmtree(os.path.join(qt, sub), ignore_errors=True)
    plug = os.path.join(qt, "plugins")
    for sub in os.listdir(plug):
        keep = KEEP_PLUGINS.get(sub, set())
        if keep is None:
            continue
        for f in os.listdir(os.path.join(plug, sub)):
            if f not in keep:
                os.remove(os.path.join(plug, sub, f))
        if not os.listdir(os.path.join(plug, sub)):
            os.rmdir(os.path.join(plug, sub))
    # ---- Cython sources next to the compiled ezdxf modules
    acc = os.path.join(internal, "ezdxf", "acc")
    for f in os.listdir(acc) if os.path.isdir(acc) else ():
        if f.endswith(SOURCE_SUFFIXES):
            os.remove(os.path.join(acc, f))

    # ---- shared libraries nothing needs any more: real files directly in _internal and in PySide6/Qt/lib
    qtlib = os.path.join(qt, "lib")
    candidates = {os.path.join(d, f) for d in (internal, qtlib) for f in os.listdir(d)
                  if os.path.isfile(os.path.join(d, f)) and not os.path.islink(os.path.join(d, f))
                  and ".so" in f and not PROTECTED.match(f)}
    by_name, elfs = scan(root)
    reached, todo = set(), [p for p in elfs if p not in candidates]
    reached.update(todo)
    while todo:
        p = todo.pop()
        for n in needed(p):
            for q in by_name.get(n, ()):
                if q not in reached:
                    reached.add(q)
                    todo.append(q)
    removed = sorted(c for c in candidates if c not in reached)
    for c in removed:
        os.remove(c)
    for d, _, fs in os.walk(root):               # symlinks that pointed to them
        for f in fs:
            p = os.path.join(d, f)
            if os.path.islink(p) and not os.path.exists(p):
                os.remove(p)
    print(f"removed {len(removed)} unneeded shared libraries:")
    print("  " + " ".join(os.path.basename(c) for c in removed))

    lost = sorted(unresolved(root) - bad_before)
    if lost:
        for f, n in lost:
            print(f"MISSING after prune: {f} needs {n}")
        sys.exit("prune removed a library that is still needed")

    # ---- symbols (not the PyInstaller bootloader: the archive is appended to it). The wheels' patched libraries (OCCT,
    # OpenBLAS) have their segments 2 MB apart in the file; strip packs them, the loader then refuses them ("ELF load
    # command address/offset not page-aligned") - every stripped file is checked and the original put back if so.
    n_strip = n_skip = 0
    for d, _, fs in os.walk(root):
        for f in fs:
            p = os.path.join(d, f)
            if os.path.islink(p) or p == os.path.join(root, "cad-viewer") or not is_elf(p):
                continue
            shutil.copy2(p, p + ".orig")
            ok = subprocess.run(["strip", "--strip-unneeded", p], capture_output=True).returncode == 0 and loadable(p)
            if ok:
                os.remove(p + ".orig")
                n_strip += 1
            else:
                os.replace(p + ".orig", p)
                n_skip += 1
    # ---- all-zero blocks -> holes
    for d, _, fs in os.walk(root):
        for f in fs:
            p = os.path.join(d, f)
            if not os.path.islink(p) and os.path.getsize(p) > 65536:
                subprocess.run(["fallocate", "--dig-holes", p], capture_output=True)
    print(f"stripped {n_strip} files ({n_skip} left as they are); size {before / 1e6:.0f} -> {tree_size(root) / 1e6:.0f} MB apparent")


if __name__ == "__main__":
    main(os.path.abspath(sys.argv[1]))

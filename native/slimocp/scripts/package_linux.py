#!/usr/bin/env python3
"""Assemble the slim OCP package (Linux).

usage: package_linux.py --so build/OCP.cpython-311-x86_64-linux-gnu.so --modules modules.txt \
                        --prefix /root/mamba/envs/b311 --out OUTDIR [--patchelf patchelf]
Result:  OUTDIR/OCP/{__init__.py, OCP.cpython-311-*.so, <Module>/__init__.py ...}
         OUTDIR/OCP.libs/{libTK*.so.8.0, ocpfreeimage.so.3, ..., libstdc++.so.6 (gcc 15 runtime)}   (RUNPATH $ORIGIN, stripped)
Not bundled (taken from the system, like the official wheel does): glibc, libGL*/libGLX*, X11/xcb, fontconfig+expat.

C++ runtime: conda-forge's OCCT is built with gcc 15 and needs CXXABI_1.3.15 (__cxa_call_terminate); Ubuntu 22.04's
libstdc++ stops at CXXABI_1.3.13.  libstdc++.so.6 / libgcc_s.so.1 of the conda env are therefore bundled in OCP.libs and
the WHOLE process must use them: in a PyInstaller onedir bundle copy them over _internal/libstdc++.so.6 and
_internal/libgcc_s.so.1 (the bootloader puts _internal first in LD_LIBRARY_PATH, so Qt gets them too); outside a bundle use
LD_PRELOAD.  (Tried and rejected: private SONAMEs for the C++ runtime like auditwheel does - works while OCP is imported
before Qt, but segfaults in STEPCAFControl_Reader.ReadFile once the system libstdc++ is already loaded: two C++ runtimes
in one process.)

Third-party libraries (libjpeg, libpng, libz, libtiff, libfreetype, ...) get a private SONAME (lib<name> -> ocp<name>,
same length, patched in place in .dynstr).  PyInstaller flattens every dependency into one folder by file name: without
this, conda's libjpeg.so.8 (libjpeg-turbo 3) is replaced by another libjpeg.so.8 from Qt / Pillow / the system and
libtiff fails with "undefined symbol: jpeg12_write_raw_data" (the official wheels avoid that with hash-suffixed names)."""
import argparse
import os
import re
import shutil
import subprocess

ap = argparse.ArgumentParser()
ap.add_argument('--so', required=True)
ap.add_argument('--modules', required=True)
ap.add_argument('--prefix', required=True)
ap.add_argument('--out', required=True)
ap.add_argument('--patchelf', default='patchelf')
a = ap.parse_args()

SYSTEM = re.compile(r'^(libc|libm|libdl|libpthread|librt|libutil|libresolv|libnsl|ld-linux.*|libGL|libGLX|libGLdispatch|libEGL|libOpenGL|'
                    r'libX11|libXext|libXi|libXt|libXrender|libXau|libXdmcp|libxcb.*|libSM|libICE|libfontconfig|libexpat|libuuid)\.so')
KEEP_NAME = re.compile(r'^(libTK|libstdc\+\+|libgcc_s)')      # unique anyway / the C++ runtime must keep its real name
NUL = bytes([0])


def needed(path):
    out = subprocess.run(['readelf', '-d', path], capture_output=True, text=True).stdout
    return re.findall(r'\(NEEDED\)\s+Shared library: \[(.+?)\]', out)


def dynstr_patch(path, rename):
    """Replace library names inside .dynstr (DT_NEEDED, DT_SONAME and the version-need file names share these strings)."""
    out = subprocess.run(['readelf', '-S', '-W', path], capture_output=True, text=True).stdout
    m = re.search(r'\.dynstr\s+STRTAB\s+[0-9a-f]+\s+([0-9a-f]+)\s+([0-9a-f]+)', out)
    off, size = int(m.group(1), 16), int(m.group(2), 16)
    data = bytearray(open(path, 'rb').read())
    blob = bytes(data[off:off + size])
    for o, n in rename.items():
        assert len(o) == len(n)
        blob = blob.replace(o.encode() + NUL, n.encode() + NUL)
    data[off:off + size] = blob
    open(path, 'wb').write(data)


out_pkg = os.path.join(a.out, 'OCP')
out_libs = os.path.join(a.out, 'OCP.libs')
shutil.rmtree(a.out, ignore_errors=True)
os.makedirs(out_pkg)
os.makedirs(out_libs)
so_name = os.path.basename(a.so)
shutil.copy2(a.so, os.path.join(out_pkg, so_name))

# --- lib closure (readelf NEEDED, searched in the conda env)
libdir = os.path.join(a.prefix, 'lib')
todo = needed(a.so)
got, system, missing = {}, set(), set()
while todo:
    n = todo.pop()
    if n in got or n in system:
        continue
    if SYSTEM.match(n):
        system.add(n)
        continue
    p = os.path.join(libdir, n)
    if not os.path.exists(p):
        missing.add(n)
        continue
    got[n] = os.path.realpath(p)
    todo += needed(got[n])

RENAME = {n: 'ocp' + n[3:] for n in got if n.startswith('lib') and not KEEP_NAME.match(n)}
for n, p in sorted(got.items()):
    dst = os.path.join(out_libs, RENAME.get(n, n))
    shutil.copy2(p, dst)
    os.chmod(dst, 0o755)
    subprocess.run([a.patchelf, '--set-rpath', '$ORIGIN', dst], check=True)            # shorter than conda's: no growth
subprocess.run([a.patchelf, '--set-rpath', '$ORIGIN/../OCP.libs', os.path.join(out_pkg, so_name)], check=True)
for f in [os.path.join(out_pkg, so_name)] + [os.path.join(out_libs, x) for x in os.listdir(out_libs)]:
    dynstr_patch(f, RENAME)
    subprocess.run(['strip', '--strip-unneeded', f], capture_output=True)

# --- python package: same layout as the official wheel (OCP/<Module>/__init__.py -> from ..OCP.<Module> import *)
mods = open(a.modules).read().split() + ['collections']
for m in mods:
    os.makedirs(os.path.join(out_pkg, m), exist_ok=True)
    open(os.path.join(out_pkg, m, '__init__.py'), 'w').write('from ..OCP.%s import *\n' % m)
INIT = '''"""Slim OCP (Open CASCADE 8.0.1 bindings, viewer subset) - same module names as cadquery-ocp."""
try:
    from OCP.OCP import *
    from OCP.OCP import __version__
except ImportError as _e:
    if 'CXXABI' in str(_e) or 'GLIBCXX' in str(_e):
        import os as _os
        _d = _os.path.abspath(_os.path.join(_os.path.dirname(__file__), _os.pardir, 'OCP.libs'))
        raise ImportError(
            str(_e) + '\\n\\nThe slim OCP is linked against the gcc-15 C++ runtime shipped in ' + _d + ' - the whole process '
            'must use it. Run python with LD_PRELOAD="' + _d + '/libstdc++.so.6 ' + _d + '/libgcc_s.so.1" '
            '(a PyInstaller bundle must contain these two files in its top folder, replacing the system ones).') from None
    raise
'''
open(os.path.join(out_pkg, '__init__.py'), 'w').write(INIT)


def size(p):
    return sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(p) for f in fs)


print('bundled libs: %d (%.1f MB); OCP pkg %.1f MB' % (len(got), size(out_libs) / 1e6, size(out_pkg) / 1e6))
print('left to the system:', sorted(system))
print('NOT FOUND:', sorted(missing))
for n in sorted(got):
    print('  ', RENAME.get(n, n), os.path.getsize(os.path.join(out_libs, RENAME.get(n, n))))

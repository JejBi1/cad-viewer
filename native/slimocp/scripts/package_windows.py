#!/usr/bin/env python
"""Assemble the slim OCP package (Windows).
usage: python package_windows.py --pyd OCP.cp311-win_amd64.pyd --modules modules.txt --bindir C:\slimocp_w\mm\envs\bwin\Library\bin --out OUTDIR
Result:  OUTDIR\OCP\{__init__.py, OCP.cp311-win_amd64.pyd, <Module>\__init__.py ...}
         OUTDIR\OCP.libs\{TK*.dll, freeimage.dll, ... (the real import closure of the .pyd; nothing else)}
Not bundled: Windows system DLLs (System32 / api-ms-*)."""
import argparse, os, shutil, sys
import pefile

ap = argparse.ArgumentParser()
ap.add_argument('--pyd', required=True)
ap.add_argument('--modules', required=True)
ap.add_argument('--bindir', required=True)
ap.add_argument('--out', required=True)
a = ap.parse_args()

sysdir = os.path.join(os.environ.get('WINDIR', r'C:\Windows'), 'System32')
index = {f.lower(): os.path.join(a.bindir, f) for f in os.listdir(a.bindir) if f.lower().endswith('.dll')}
dirs = [pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT']]       # delay-load imports are optional by design

def imports(p):
    pe = pefile.PE(p, fast_load=True)
    pe.parse_data_directories(directories=dirs)
    r = [e.dll.decode().lower() for e in getattr(pe, 'DIRECTORY_ENTRY_IMPORT', [])]
    pe.close()
    return r

need, todo, system = {}, imports(a.pyd), set()
while todo:
    n = todo.pop()
    if n in need or n in system: continue
    if n.startswith('api-ms-') or n.startswith('ext-ms-'):
        system.add(n); continue
    if n in index and n not in ('python311.dll', 'python3.dll'):
        # vcruntime/msvcp from the conda env are newer than python's: keep them (they only get loaded if not loaded before)
        need[n] = index[n]; todo += imports(index[n])
    else:
        system.add(n)

shutil.rmtree(a.out, ignore_errors=True)
pkg, libs = os.path.join(a.out, 'OCP'), os.path.join(a.out, 'OCP.libs')
os.makedirs(pkg); os.makedirs(libs)
shutil.copy2(a.pyd, os.path.join(pkg, os.path.basename(a.pyd)))
for n, p in sorted(need.items()):
    shutil.copy2(p, os.path.join(libs, os.path.basename(p)))
mods = open(a.modules).read().split() + ['collections']
for m in mods:
    os.makedirs(os.path.join(pkg, m), exist_ok=True)
    open(os.path.join(pkg, m, '__init__.py'), 'w').write('from ..OCP.%s import *\n' % m)
open(os.path.join(pkg, '__init__.py'), 'w').write('''"""Slim OCP (Open CASCADE 8.0.1 bindings, viewer subset) - same module names as cadquery-ocp."""
def _dll_dir():
    import os
    d = os.path.abspath(os.path.join(os.path.dirname(__file__), os.pardir, 'OCP.libs'))
    if os.path.isdir(d):
        os.add_dll_directory(d)


_dll_dir()
del _dll_dir

from OCP.OCP import *

from OCP.OCP import __version__
''')
def size(p): return sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(p) for f in fs)
print('bundled DLLs: %d (%.1f MB); OCP pkg %.1f MB' % (len(need), size(libs) / 1e6, size(pkg) / 1e6))
print('system:', sorted(s for s in system if not s.startswith(('api-ms-', 'ext-ms-'))))
for n, p in sorted(need.items()): print('  %-24s %9d' % (n, os.path.getsize(p)))

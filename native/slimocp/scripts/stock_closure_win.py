"""Size of what the current build.py bundles for OCP: OCP.pyd + the DLL closure from cadquery_ocp.libs + vtk.libs"""
import os, sys, pefile
SP = r'%USERPROFILE%\.venvs\stepviewer\Lib\site-packages')
LIB_DIRS = ["cadquery_ocp.libs", "vtk.libs"]
index = {}
for d in LIB_DIRS:
    for f in os.listdir(os.path.join(SP, d)):
        index[f.lower()] = (d, os.path.join(SP, d, f))
pyd = os.path.join(SP, "OCP", "OCP.cp311-win_amd64.pyd")
need, todo, seen = {}, [pyd], set()
dirs = [pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"], pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT"]]
while todo:
    p = todo.pop()
    if p in seen: continue
    seen.add(p)
    pe = pefile.PE(p, fast_load=True); pe.parse_data_directories(directories=dirs)
    for attr in ("DIRECTORY_ENTRY_IMPORT", "DIRECTORY_ENTRY_DELAY_IMPORT"):
        for e in getattr(pe, attr, []):
            n = e.dll.decode().lower()
            if n in index and n not in need:
                need[n] = index[n]; todo.append(index[n][1])
    pe.close()
tot = sum(os.path.getsize(p) for d, p in need.values())
by = {}
for d, p in need.values(): by[d] = by.get(d, 0) + os.path.getsize(p)
print('stock OCP.pyd: %.1f MB' % (os.path.getsize(pyd) / 1e6))
print('stock DLL closure: %d DLLs, %.1f MB  %s' % (len(need), tot / 1e6, {k: '%.1f MB' % (v / 1e6) for k, v in by.items()}))
print('stock total (pyd + DLLs): %.1f MB' % ((tot + os.path.getsize(pyd)) / 1e6))
pk = os.path.join(SP, 'OCP')
print('stock OCP package dir (pyd + 324 stub packages): %.1f MB' % (sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(pk) for f in fs) / 1e6))
print('whole libs dirs on disk: cadquery_ocp.libs %.1f MB, vtk.libs %.1f MB' % tuple(sum(os.path.getsize(os.path.join(SP, x, f)) for f in os.listdir(os.path.join(SP, x))) / 1e6 for x in LIB_DIRS))

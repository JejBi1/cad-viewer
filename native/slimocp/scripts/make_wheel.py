#!/usr/bin/env python3
"""Pack the output of package_linux.py / package_windows.py (OUTDIR with OCP/ and OCP.libs/) into a pip-installable wheel.
usage: make_wheel.py OUTDIR DESTDIR {linux_x86_64|win_amd64} [version]
The wheel provides the top-level packages OCP and OCP.libs (same import names as cadquery-ocp) and has NO dependencies
(no vtk, no cadquery-ocp-proxy).  pip install --no-deps is not even needed.  Do not install it next to cadquery-ocp."""
import base64
import csv
import hashlib
import io
import os
import sys
import zipfile

out_dir, dest, plat = sys.argv[1:4]
version = sys.argv[4] if len(sys.argv) > 4 else '8.0.1.1+slim1'
name = 'cadquery_ocp_slim'
tag = 'cp311-cp311-' + plat
dist_info = '%s-%s.dist-info' % (name, version)
wheel_name = '%s-%s-%s.whl' % (name, version, tag)

files = []
for top in ('OCP', 'OCP.libs'):
    for d, _, fs in os.walk(os.path.join(out_dir, top)):
        for f in fs:
            p = os.path.join(d, f)
            files.append((os.path.relpath(p, out_dir).replace(os.sep, '/'), p))

metadata = '''Metadata-Version: 2.1
Name: cadquery-ocp-slim
Version: %s
Summary: Slim OCP (Open CASCADE 8.0.1 Python bindings, STEP viewer subset: 73 modules, no VTK)
License: Apache-2.0
Requires-Python: >=3.11,<3.12
Description-Content-Type: text/plain

Rebuilt from the official CadQuery/OCP 8.0.1.1 generator (pywrap) restricted to the modules a CAD viewer uses.
Same module names as cadquery-ocp (from OCP.AIS import ..., OCP.collections, ...).  Conflicts with cadquery-ocp.
''' % version
wheel = '''Wheel-Version: 1.0
Generator: make_wheel.py
Root-Is-Purelib: false
Tag: %s
''' % tag


def digest(data):
    return 'sha256=' + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b'=').decode()


os.makedirs(dest, exist_ok=True)
rows = []
with zipfile.ZipFile(os.path.join(dest, wheel_name), 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for arc, p in files:
        data = open(p, 'rb').read()
        zi = zipfile.ZipInfo(arc, date_time=(2026, 11, 11, 0, 0, 0))
        zi.compress_type = zipfile.ZIP_DEFLATED
        zi.external_attr = (0o755 if (arc.endswith(('.so', '.dll', '.pyd')) or '.so.' in arc) else 0o644) << 16
        z.writestr(zi, data)
        rows.append((arc, digest(data), len(data)))
    for arc, text in (('%s/METADATA' % dist_info, metadata), ('%s/WHEEL' % dist_info, wheel),
                      ('%s/top_level.txt' % dist_info, 'OCP\n')):
        data = text.encode()
        z.writestr(arc, data)
        rows.append((arc, digest(data), len(data)))
    buf = io.StringIO()
    w = csv.writer(buf, lineterminator='\n')
    for r in rows:
        w.writerow(r)
    w.writerow(('%s/RECORD' % dist_info, '', ''))
    z.writestr('%s/RECORD' % dist_info, buf.getvalue())
print(os.path.join(dest, wheel_name), '%.1f MB' % (os.path.getsize(os.path.join(dest, wheel_name)) / 1e6))

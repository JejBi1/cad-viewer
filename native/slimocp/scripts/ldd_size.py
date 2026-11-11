#!/usr/bin/env python3
"""usage: ldd_size.py file.so [exclude_regex]  -> transitive shared-lib closure (via ldd), sizes; libs under /lib,/usr/lib are 'system'"""
import subprocess, sys, os, re
f = sys.argv[1]
out = subprocess.run(['ldd', f], capture_output=True, text=True).stdout
sysre = re.compile(r'^/(usr/)?lib(64)?/(x86_64-linux-gnu/)?')
tot = 0; n = 0; sysn = []
rows = []
for l in out.splitlines():
    m = re.match(r'\s*(\S+) => (\S+)', l)
    if not m: continue
    name, path = m.groups()
    if path == 'not' : print('NOT FOUND', l); continue
    if sysre.match(path) and 'site-packages' not in path:
        sysn.append(name); continue
    s = os.path.getsize(os.path.realpath(path)); tot += s; n += 1
    rows.append((s, name, path))
for s, nme, p in sorted(rows, reverse=True)[:int(os.environ.get('TOP', '12'))]:
    print('%10d %s' % (s, p))
print('bundled-lib closure: %d libs, %.1f MB; system libs: %d' % (n, tot / 1e6, len(sysn)))
print('system:', ' '.join(sorted(sysn)))

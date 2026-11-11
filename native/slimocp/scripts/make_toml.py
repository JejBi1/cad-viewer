#!/usr/bin/env python3
"""Rewrite the `modules = [...]` list of the official OCP ocp.toml.
usage: make_toml.py ocp.toml out.toml modules.txt   (modules.txt: whitespace separated names)
       make_toml.py ocp.toml out.toml -x IVtk IVtkOCC ...   (all modules except these)
Platform specific modules (Xw / WNT / Cocoa) stay in their [Linux]/[Windows] tables."""
import re, sys
src, out = sys.argv[1], sys.argv[2]
txt = open(src).read()
m = re.search(r'^modules = \[(.*?)^\]', txt, re.S | re.M)
allmods = []
for l in m.group(1).splitlines():
    l = l.strip()
    if l.startswith('#') or not l:
        continue
    allmods += re.findall(r'"([^"]+)"', l)
if sys.argv[3] == '-x':
    mods = [x for x in allmods if x not in set(sys.argv[4:])]
else:
    want = open(sys.argv[3]).read().split()
    unknown = [w for w in want if w not in allmods and w not in ('Xw', 'WNT', 'Cocoa')]
    if unknown:
        sys.stderr.write('WARNING: not in ocp.toml: %s\n' % unknown)
    mods = [x for x in allmods if x in set(want)]
new = 'modules = [\n' + ''.join('"%s",\n' % x for x in mods) + ']'
txt = txt[:m.start()] + new + txt[m.end():]
open(out, 'w').write(txt)
print(len(mods), 'modules written to', out)

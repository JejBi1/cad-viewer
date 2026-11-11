#!/usr/bin/env python3
"""Dump the public API surface of an OCP build: {module: {name: [members] | None}}.
usage: python api_dump.py out.json    (uses whatever 'OCP' is first on sys.path; uses OCP.OCP.<mod> registry)"""
import sys, json, inspect, importlib
import OCP.OCP as o
out = {}
mods = [n for n in dir(o) if inspect.ismodule(getattr(o, n))]
for n in mods:
    m = getattr(o, n)
    d = {}
    for k in dir(m):
        if k.startswith('__'): continue
        v = getattr(m, k)
        if inspect.isclass(v):
            d[k] = sorted(x for x in dir(v) if not x.startswith('__') or x in ('__init__','__call__','__add__','__sub__','__mul__','__truediv__'))
        else:
            d[k] = None
    out[n] = d
json.dump(out, open(sys.argv[1], 'w'))
print(len(out), 'modules', sum(len(v) for v in out.values()), 'names')

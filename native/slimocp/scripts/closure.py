#!/usr/bin/env python3
"""Closure analysis over pywrap parse pickles.
usage: closure.py base_modules.txt used_names.txt  pickle1 [pickle2 ...]
 - base closure: superclass chains (always required)
 - signature closure: modules referenced in signatures of methods whose NAME is in used_names (names the app calls)
Prints modules needed but not yet parsed."""
import pickle, re, sys
sys.path.insert(0, '/root/slimocp/OCP_src/pywrap')
import toml
allmods = set()
txt = open('/root/slimocp/OCP_src/ocp.toml').read()
m = re.search(r'^modules = \[(.*?)^\]', txt, re.S | re.M)
for l in m.group(1).splitlines():
    l = l.strip()
    if l.startswith('#') or not l: continue
    allmods |= set(re.findall(r'"([^"]+)"', l))
allmods |= {'Xw', 'WNT'}
allmods -= {'IVtk', 'IVtkOCC', 'IVtkTools', 'IVtkVTK'}
ident = re.compile(r'[A-Za-z_][A-Za-z0-9_]*')

def mods_of(tstr):
    out = set()
    for i in ident.findall(tstr):
        p = i.split('_')[0]
        if p in allmods:
            out.add(p)
    return out

base = open(sys.argv[1]).read().split()
used = set(open(sys.argv[2]).read().split())
mods = {}
for pk in sys.argv[3:]:
    for mm in pickle.load(open(pk, 'rb')):
        mods[mm.name] = mm
cls2mod = {c.name: mm.name for mm in mods.values() for c in mm.classes}
print('parsed modules:', len(mods))
# base closure
need_base = set(); missing_parse = set()
def sup_mods(mname):
    r = set()
    mm = mods.get(mname)
    if not mm: return r
    for c in mm.classes:
        for s in c.superclass:
            r |= mods_of(s)
    return r
cur = set(base); changed = True
while changed:
    changed = False
    for mn in list(cur):
        for s in sup_mods(mn):
            if s not in cur:
                cur.add(s); changed = True
print('base-closure (superclasses):', len(cur))
print(sorted(cur - set(base)))
print('not parsed yet:', sorted(x for x in cur if x not in mods))
# signature closure by called names
sig = {}
for mn in cur:
    mm = mods.get(mn)
    if not mm: continue
    for c in mm.classes:
        for me in c.methods + c.static_methods + c.methods_byref + c.static_methods_byref + c.constructors:
            if me.name in used or me.name == c.name:
                t = set()
                for _, a, _ in me.args: t |= mods_of(a)
                t |= mods_of(me.return_type)
                for x in t:
                    if x not in cur:
                        sig.setdefault(x, set()).add('%s::%s' % (c.name, me.name))
print('signature-referenced modules not in set (from called-name methods):')
for k, v in sorted(sig.items(), key=lambda kv: -len(kv[1])):
    print(' ', k, len(v), sorted(v)[:4])

#!/usr/bin/env python3
"""Class-level reachability over pywrap parse pickles (what the Python app can actually reach).
usage: closure2.py app_imports.json used_names.txt out_modules.txt  parse_*.pkl
Reach starts at the classes/enums/functions the app imports; a class pulls in
  - its base classes (pybind11 needs them registered, otherwise inherited methods vanish)
  - every class in the signature (args, return type, field type) of each constructor / method whose NAME occurs in the app source
Modules of all reached classes = needed modules.  Modules not parsed yet are reported (parse them, rerun)."""
import json, pickle, re, sys
sys.path.insert(0, '/root/slimocp/OCP_src/pywrap')
imports = json.load(open(sys.argv[1]))
used = {n for n in open(sys.argv[2]).read().split() if n[0].isupper()}
out = sys.argv[3]
txt = open('/root/slimocp/OCP_src/ocp.toml').read()
m = re.search(r'^modules = \[(.*?)^\]', txt, re.S | re.M)
allmods = set()
for l in m.group(1).splitlines():
    l = l.strip()
    if l.startswith('#') or not l: continue
    allmods |= set(re.findall(r'"([^"]+)"', l))
allmods |= {'Xw', 'WNT'}
allmods -= {'IVtk', 'IVtkOCC', 'IVtkTools', 'IVtkVTK'}
mods = {}
for pk in sys.argv[4:]:
    for mm in pickle.load(open(pk, 'rb')):
        mods[mm.name] = mm
classes = {c.name: c for mm in mods.values() for c in mm.classes}
cls_mod = {c.name: mm.name for mm in mods.values() for c in mm.classes}
tdefs = {}
for mm in mods.values():
    for t in mm.typedefs:
        tdefs[t.name] = (mm.name, t.type)
    for h in mm.headers:
        for t in h.typedefs:
            tdefs.setdefault(t.name, (mm.name, t.type))
enums = {e.name: mm.name for mm in mods.values() for e in mm.enums}
for c in classes.values():
    pass
ident = re.compile(r'[A-Za-z_][A-Za-z0-9_:]*')
needed_unparsed = {}; why = {}
reach = set(); work = []

def add_class(n, src):
    if n in reach: return
    reach.add(n); work.append(n); why.setdefault(cls_mod[n], (n, src))

def visit_token(t, src, depth=0):
    t = t.replace('::', '_') if t.startswith('opencascade') else t
    t = t.split('::')[0] if '::' in t and t.split('::')[0] in classes else t
    if t in classes:
        add_class(t, src); return
    if t in tdefs and depth < 6:
        for u in ident.findall(tdefs[t][1]):
            visit_token(u, src, depth + 1)
        return
    if t in enums:
        why.setdefault(enums[t], (t, src)); reach_mods.add(enums[t]); return
    p = t.split('_')[0]
    if p in allmods and p not in mods and '_' in t:
        needed_unparsed.setdefault(p, set()).add(t)

reach_mods = set()
for mname, names in imports.items():
    mod = mname.split('.')[1]
    reach_mods.add(mod)
    if mod == 'collections':
        continue
    for n in names:
        if n in classes: add_class(n, 'imported')
        elif n in enums: reach_mods.add(enums[n])
        else:
            # module-level function / constant / enum value / class in unparsed module
            if mod not in mods: needed_unparsed.setdefault(mod, set()).add(n)

def sigs(c):
    for me in c.methods + c.static_methods + c.methods_byref + c.static_methods_byref + c.constructors + c.methods_return_byref:
        yield me
while work:
    n = work.pop()
    c = classes[n]
    for s in c.superclass:
        for t in ident.findall(s): visit_token(t, 'base of ' + n)
    for me in sigs(c):
        if me.name not in used and me.name != c.name and not me.name.endswith('_s') and me.name != '__init__':
            continue
        if me.name.endswith('_s') and me.name[:-2] not in used and me.name not in used:
            continue
        for _, a, _ in me.args:
            for t in ident.findall(a): visit_token(t, '%s::%s' % (n, me.name))
        for t in ident.findall(me.return_type): visit_token(t, '%s::%s' % (n, me.name))
    for f in c.fields:
        for t in ident.findall(getattr(f, 'type', '') or ''): visit_token(t, 'field of ' + n)

need = set(cls_mod[n] for n in reach) | reach_mods
need |= set(imports_m.split('.')[1] for imports_m in imports if imports_m != 'OCP.collections')
print('reached classes:', len(reach), ' modules:', len(need))
print(' '.join(sorted(need)))
print('--- modules needed but not parsed:', sorted(needed_unparsed))
for k in sorted(needed_unparsed): print('   ', k, sorted(needed_unparsed[k])[:5])
open(out, 'w').write(' '.join(sorted(need)))
print('--- first reason per module (non-app):')
appm = {x.split('.')[1] for x in imports}
for mname in sorted(need - appm):
    print('  %-14s %s <- %s' % (mname, why.get(mname, ('?', '?'))[0], why.get(mname, ('?', '?'))[1]))

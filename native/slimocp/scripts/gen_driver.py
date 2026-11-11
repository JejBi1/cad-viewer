#!/usr/bin/env python3
"""transform + render for a *subset* of OCP modules, from one or more pywrap parse pickles.

usage: gen_driver.py --platform Linux|Windows --toml ocp.toml --modules modules.txt --src OCP_src \
                     --out OUTDIR  parse_a.pkl [parse_b.pkl ...]
Equivalent to `python -m bindgen transform` + `generate` of the official OCP build, but restricted to the modules in
modules.txt (everything else is simply not generated; IVtk* never is)."""
import argparse, os, pickle, shutil, sys

ap = argparse.ArgumentParser()
ap.add_argument('--platform', required=True)
ap.add_argument('--toml', required=True)
ap.add_argument('--modules', required=True)
ap.add_argument('--src', required=True)
ap.add_argument('--out', required=True)
ap.add_argument('--njobs', type=int, default=8)
ap.add_argument('pickles', nargs='+')
a = ap.parse_args()

sys.path.insert(0, os.path.join(a.src, 'pywrap'))
import logzero
from path import Path
from bindgen import read_settings, transform_modules, render

want = a.modules and open(a.modules).read().split()
settings, module_mapping, module_settings = read_settings(os.path.abspath(a.toml))
byname = {}
for pk in a.pickles:
    for m in pickle.load(open(pk, 'rb')):
        byname[m.name] = m
missing = [w for w in want if w not in byname]
if missing:
    sys.exit('modules not parsed: %s' % missing)
modules = [byname[w] for w in want]
print(len(modules), 'modules selected')

# ---- prune everything that mentions a type of a module that is not part of this build ------------------------------------
# (the official build has all modules, so all headers are included and every signature compiles; with a subset, a method
#  like IFSelect_X::Entities() -> Interface_EntityIterator would use an incomplete type. Such a method could never be
#  called from Python anyway - the argument/return class is not bound - so it is dropped.)
import re
txt = open(os.path.join(a.src, 'ocp.toml')).read()      # the FULL module list of the official build
mm_ = re.search(r'^modules = \[(.*?)^\]', txt, re.S | re.M)
fullmods = set()
for l in mm_.group(1).splitlines():
    l = l.strip()
    if l and not l.startswith('#'):
        fullmods |= set(re.findall(r'"([^"]+)"', l))
fullmods |= {'Xw', 'WNT', 'Cocoa'}
fullmods |= {'Interface', 'Transfer', 'Resource', 'Units', 'BinMDF', 'BinMDataStd', 'XmlObjMgt', 'IGESData', 'StepData', 'Storage'}
included = set(want) | {'collections'}
tok = re.compile(r'[A-Za-z_][A-Za-z0-9_]*')
bad_names = set()

last_bad = ['']
def bad_type(s):
    if not s:
        return False
    for t in tok.findall(s):
        p = t.split('_')[0]
        if (p in fullmods and p not in included) or t in bad_names:
            last_bad[0] = t
            return True
    return False

def bad_fn(f):
    if bad_type(f.return_type):
        return True
    for _, t, d in f.args:
        if bad_type(t) or bad_type(d if isinstance(d, str) else ''):
            return True
    return any(bad_type(t) for t in (getattr(f, 'default_value_types', None) or []))

for _ in range(3):      # typedefs of included modules that mention a missing type are missing as well
    for m in modules:
        for h in m.headers:
            for t in h.typedefs:
                if bad_type(t.type):
                    bad_names.add(t.name)
dropped = {}
lost_pure = []
prune_log = []
def prune_class(c, m):
    for attr in ('methods', 'static_methods', 'constructors', 'operators', 'static_operators',
                 'protected_virtual_methods', 'private_virtual_methods'):
        lst = getattr(c, attr, None)
        if lst:
            keep = []
            for f in lst:
                if bad_fn(f):
                    prune_log.append('%s::%s  <- %s' % (c.name, f.name, last_bad[0]))
                else:
                    keep.append(f)
            if len(keep) != len(lst):
                dropped[m.name] = dropped.get(m.name, 0) + len(lst) - len(keep)
                setattr(c, attr, keep)
                if any(getattr(f, 'pure_virtual', False) for f in lst if f not in keep):
                    lost_pure.append(c)
    if getattr(c, 'fields', None):
        c.fields = [f for f in c.fields if not bad_type(f.type)]
for m in modules:
    for c in m.classes + m.class_templates:
        prune_class(c, m)
    m.functions = [f for f in m.functions if not bad_fn(f)]
    m.typedefs = [t for t in m.typedefs if t.name not in bad_names]
    for h in m.headers:
        h.functions = [f for f in h.functions if not bad_fn(f)]
        h.typedefs = [t for t in h.typedefs if t.name not in bad_names]
lost_names = set(c.name for c in lost_pure)
for m in modules:       # abstract descendants of such a class inherit the problem (their trampoline cannot override it)
    for c in m.classes:
        if c.abstract and (set(c.superclass) | set(c.superclasses)) & lost_names and c not in lost_pure:
            lost_pure.append(c); lost_names.add(c.name)
for c in lost_pure:     # a trampoline can no longer override every pure virtual -> the class cannot be instantiated from Python
    c.constructors = []
print('abstract classes without Python constructor (pure virtual needs a missing module):', sorted(set(c.name for c in lost_pure)))
print('pruned %d methods/constructors that need modules outside the build; typedefs dropped: %d'
      % (sum(dropped.values()), len(bad_names)))
print('  most affected:', sorted(dropped.items(), key=lambda kv: -kv[1])[:8])

open(os.path.join(os.path.dirname(os.path.abspath(a.out)), 'pruned_%s.txt' % a.platform), 'w').write(chr(10).join(sorted(prune_log)))
os.makedirs(a.out, exist_ok=True)
os.chdir(a.out)            # symbols_mangled_*.dat is looked up in the cwd; output folder is relative to it
logzero.logger.setLevel(logzero.logging.INFO)
modules, classinfo_dict, class_dict, enum_dict, symbol_dict, collections = transform_modules(
    False, a.njobs, settings, module_mapping, module_settings, modules, platform=a.platform)
render(settings, module_settings, modules, classinfo_dict, class_dict, symbol_dict, Path(a.src),
       collections=collections, platform=a.platform)
out = Path(settings['output_folder'])
(Path(a.src) / Path(settings['Extras']['include_pre'])).copy(out)
print('generated', len(modules), 'modules,', len(collections), 'collection types ->', os.path.abspath(out))

#!/usr/bin/env python3
"""usage: api_diff.py ref.json slim.json used_names.txt  -> what the slim build lacks compared with the stock OCP"""
import json, sys
ref = json.load(open(sys.argv[1])); slim = json.load(open(sys.argv[2]))
used = set(open(sys.argv[3]).read().split())
mods = [m for m in slim if m in ref]
print('modules: slim %d / stock %d' % (len(slim), len(ref)))
tot_cls = tot_missing_cls = tot_mem = tot_missing_mem = 0
missing_cls, missing_mem = [], []
for m in mods:
    for name, mem in ref[m].items():
        if name not in slim[m]:
            missing_cls.append((m, name)); continue
        if mem is None: continue
        tot_cls += 1
        s = set(slim[m][name] or [])
        miss = [x for x in mem if x not in s]
        tot_mem += len(mem); tot_missing_mem += len(miss)
        if miss: missing_mem.append((m, name, miss))
print('names (classes/enums/functions) in stock modules that exist in slim: missing %d' % len(missing_cls))
print('members of shared classes: stock %d, missing in slim %d (%.2f%%)' % (tot_mem, tot_missing_mem, 100.0 * tot_missing_mem / max(1, tot_mem)))
hit = [(m, c, [x for x in miss if x in used]) for m, c, miss in missing_mem]
hit = [h for h in hit if h[2]]
print('\nmissing members whose NAME occurs in the app sources (%d classes):' % len(hit))
for m, c, x in hit: print('  %s.%s: %s' % (m, c, ' '.join(x)))
print('\nmissing classes/functions whose name occurs in the app sources:')
for m, n in missing_cls:
    if n in used: print('  %s.%s' % (m, n))
print('\nmost affected modules (missing members):')
agg = {}
for m, c, miss in missing_mem: agg[m] = agg.get(m, 0) + len(miss)
print(sorted(agg.items(), key=lambda kv: -kv[1])[:10])
print('\nmissing classes per module:', sorted({m: sum(1 for x in missing_cls if x[0] == m) for m in mods}.items(), key=lambda kv: -kv[1])[:8])

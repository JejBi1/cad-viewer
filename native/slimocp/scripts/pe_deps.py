"""usage: pe_deps.py <bindir> <root1.dll> [root2.dll ...]  -> prints DLL closure (found in bindir), non-found as SYSTEM"""
import pefile, sys, os
bindir = sys.argv[1]
roots = sys.argv[2:]
seen = {}; sysdeps = set()
stack = list(roots)
def lookup(n):
    p = os.path.join(bindir, n)
    return p if os.path.exists(p) else None
while stack:
    n = stack.pop()
    if n.lower() in seen: continue
    p = n if os.path.isabs(n) else lookup(n)
    if not p:
        sysdeps.add(n.lower()); continue
    seen[os.path.basename(n).lower()] = p
    pe = pefile.PE(p, fast_load=True)
    pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT']])
    for e in getattr(pe, 'DIRECTORY_ENTRY_IMPORT', []):
        stack.append(e.dll.decode())
tot = 0
for k, p in sorted(seen.items()):
    s = os.path.getsize(p); tot += s
    print('%10d  %s' % (s, k))
print('TOTAL', tot, 'bytes in', len(seen), 'files')
print('system/not-found:', sorted(d for d in sysdeps if not d.startswith('api-ms-')))

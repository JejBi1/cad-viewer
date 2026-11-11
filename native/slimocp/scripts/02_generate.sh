#!/bin/bash
# usage: 02_generate.sh <Linux|Windows> <modules.txt> [parse pickles...]  -> work/<plat>/gen/OCP (C++ sources of the slim OCP)
# Generates into gen_new/ and copies only files whose content changed into gen/ (keeps mtimes -> ninja rebuilds only what changed)
set -e
PLAT=$1; MODS=$2; shift 2
source /root/slimocp/scripts/env_gen.sh
D=/root/slimocp/work/$(echo $PLAT | tr A-Z a-z)
cd $D
$PY /root/slimocp/scripts/make_toml.py $SRC/ocp.toml slim.toml $MODS
# mark the build and keep the official version string
$PY - <<'PY'
t = open('slim.toml').read()
if '__slim__' not in t:
    t = t.replace('__version__ = "8.0.1.1"', '__version__ = "8.0.1.1"\n    __slim__ = "1"')
open('slim.toml', 'w').write(t)
PY
rm -rf gen_new; mkdir gen_new; cp symbols_mangled_*.dat gen_new/
time $PY /root/slimocp/scripts/gen_driver.py --platform $PLAT --toml $D/slim.toml --modules $MODS --src $SRC --out $D/gen_new --njobs 8 "$@"
cp /root/slimocp/scripts/CMakeLists.slim.txt $D/gen_new/OCP/CMakeLists.txt
mkdir -p gen/OCP
$PY - <<'PY'
import filecmp, os, shutil
src, dst = 'gen_new/OCP', 'gen/OCP'
changed = 0
for f in os.listdir(src):
    s, d = os.path.join(src, f), os.path.join(dst, f)
    if not os.path.exists(d) or not filecmp.cmp(s, d, shallow=False):
        shutil.copy2(s, d); changed += 1
for f in os.listdir(dst):
    if not os.path.exists(os.path.join(src, f)):
        os.remove(os.path.join(dst, f)); changed += 1
print('files changed in gen/OCP:', changed, 'of', len(os.listdir(src)))
PY

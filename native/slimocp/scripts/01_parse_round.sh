#!/bin/bash
# usage: 01_parse_round.sh <round-name> <modules.txt> [platform=Linux] [njobs=8]
# parses the listed modules with the pywrap generator; result: work/<platform>/parse_<round-name>.pkl
set -e
NAME=$1; MODS=$2; PLAT=${3:-Linux}; NJ=${4:-8}
source /root/slimocp/scripts/env_gen.sh
[ "$PLAT" = Windows ] && source /root/slimocp/scripts/env_gen_win.sh
D=/root/slimocp/work/$(echo $PLAT | tr A-Z a-z); mkdir -p $D; cd $D
if [ "$PLAT" = Linux ]; then [ -f symbols_mangled_linux.dat ] || $PY $SRC/dump_symbols.py "$(ls $CONDA_PREFIX/lib/libTK*.so.8.0.1 | paste -sd';')"; fi
$PY /root/slimocp/scripts/make_toml.py $SRC/ocp.toml parse_$NAME.toml $MODS
time $PY -m bindgen -n $NJ $PYWRAP_ARGS parse $D/parse_$NAME.toml parse_$NAME.pkl $PLAT

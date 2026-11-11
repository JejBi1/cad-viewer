#!/bin/bash
# One-time preparation (run in WSL) for generating the *Windows* flavour of OCP from Linux:
#  - symlinks (without spaces) to the local MSVC 14.44 + UCRT headers on the Windows side
#  - symbols_mangled_win.dat = exported symbols of the Windows conda-forge TK*.dll (pywrap drops methods that the DLLs do not export)
set -e
MSVC="/mnt/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207/include"
UCRT="/mnt/c/Program Files (x86)/Windows Kits/10/Include/10.0.26100.0/ucrt"
ln -sfn "$MSVC" /root/slimocp/work/msvc_inc
ln -sfn "$UCRT" /root/slimocp/work/ucrt_inc
source /root/slimocp/scripts/env_gen.sh
mkdir -p /root/slimocp/work/windows/dlls && cd /root/slimocp/work/windows
cp /mnt/c/slimocp_w/mm/envs/bwin/Library/bin/TK*.dll dlls/
$PY $SRC/dump_symbols.py "$(ls $PWD/dlls/TK*.dll | paste -sd';')"      # -> symbols_mangled_win.dat
wc -l symbols_mangled_win.dat

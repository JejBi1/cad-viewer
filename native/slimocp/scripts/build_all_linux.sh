#!/bin/bash
# Whole Linux pipeline from scratch (about 1.5 h on 16 threads); run inside WSL:  wsl -d cadbuild -u root -- bash build_all_linux.sh
# Needs the OCP sources:  git clone --depth 1 --branch 8.0.1.1 https://github.com/CadQuery/OCP.git /root/slimocp/OCP_src
#                         git -C /root/slimocp/OCP_src submodule update --init --depth 1 pywrap      (pywrap commit 76a16d0)
set -e
S=/root/slimocp/scripts; W=/root/slimocp/work; mkdir -p $W/linux
bash $S/00_setup_envs_linux.sh                                             # conda envs gen (py3.13 + libclang 21) and b311 (py3.11 + gcc 15)
# parse: first the modules the app imports, then (closure2.py) what they reach; round files = module lists
bash $S/01_parse_round.sh r1 $W/app_modules.txt Linux 8
bash $S/01_parse_round.sh r2 $W/r2.txt Linux 8
bash $S/01_parse_round.sh r3 $W/r3.txt Linux 4
# (closure2.py app_imports.json used_names.txt need.txt parse_r*.pkl  -> modules_linux.txt, see README)
bash $S/02_generate.sh Linux $W/modules_linux.txt $W/linux/parse_r1.pkl $W/linux/parse_r2.pkl $W/linux/parse_r3.pkl
bash $S/03_build_linux.sh 9                                                # cmake + ninja, conda gcc 15  -> work/linux/build/OCP.cpython-311-x86_64-linux-gnu.so
export PATH=/root/mamba/envs/gen/bin:$PATH
python3 $S/package_linux.py --so $W/linux/build/OCP.cpython-311-x86_64-linux-gnu.so --modules $W/modules_linux.txt \
        --prefix /root/mamba/envs/b311 --out /root/slimocp/out/linux --patchelf /root/mamba/envs/gen/bin/patchelf
python3 $S/make_wheel.py /root/slimocp/out/linux /root/slimocp/out/wheels linux_x86_64

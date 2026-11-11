#!/bin/bash
# usage: 03_build_linux.sh [jobs]   -> work/linux/build/OCP.cpython-311-x86_64-linux-gnu.so
set -e
J=${1:-8}
export MAMBA_ROOT_PREFIX=/root/mamba
P=/root/mamba/envs/b311
export PATH=$P/bin:$PATH
D=/root/slimocp/work/linux
cd $D
cmake -S gen/OCP -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
   -DCMAKE_PREFIX_PATH=$P -DPython_EXECUTABLE=$P/bin/python -DPython_FIND_STRATEGY=LOCATION \
   -DCMAKE_C_COMPILER=$P/bin/x86_64-conda-linux-gnu-gcc -DCMAKE_CXX_COMPILER=$P/bin/x86_64-conda-linux-gnu-g++ \
   -DOpenCASCADE_DIR=$P/lib/cmake/opencascade -Dpybind11_DIR=$P/share/cmake/pybind11
time cmake --build build -j $J -- -k 50

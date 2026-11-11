#!/bin/bash
# Creates two conda-forge envs under /root/mamba:
#   gen   : python 3.13 + libclang 21 + pywrap deps + occt 8.0.1 (novtk) -> run the OCP generator
#   b311  : python 3.11 + pybind11 2.13 + occt 8.0.1 (novtk) + conda gcc 15 -> compile OCP.so for the app's python
set -e
export MAMBA_ROOT_PREFIX=/root/mamba
MM=/opt/micromamba/bin/micromamba
$MM create -y -n gen -c conda-forge python=3.13 "occt=8.0.1=novtk*" "pybind11=2.13.*" \
   "clangdev=21.*" "python-clang=21.*" joblib toml click jinja2 logzero pandas path pyparsing schema tqdm toposort py-lief \
   cmake ninja fmt rapidjson patchelf
$MM create -y -n b311 -c conda-forge python=3.11 "occt=8.0.1=novtk*" "pybind11=2.13.*" \
   cxx-compiler cmake ninja fmt patchelf pyelftools pefile

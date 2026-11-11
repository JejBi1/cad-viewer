# sourced: environment for the pywrap generator on Linux
export MAMBA_ROOT_PREFIX=/root/mamba
export CONDA_PREFIX=/root/mamba/envs/gen
export PATH=$CONDA_PREFIX/bin:$PATH
export SRC=/root/slimocp/OCP_src
export PY=$CONDA_PREFIX/bin/python
export PYTHONPATH=$SRC/pywrap
export PYTHONHASHSEED=0
CLANGVER=$(ls $CONDA_PREFIX/lib/clang | head -1)
export PYWRAP_ARGS="-l $CONDA_PREFIX/lib/libclang.so \
 -i $CONDA_PREFIX/include -i $CONDA_PREFIX/include/freetype2 \
 -i $CONDA_PREFIX/lib/clang/$CLANGVER/include/ \
 -i /usr/include/c++/11/ -i /usr/include/x86_64-linux-gnu/c++/11/ -i /usr/include/c++/11/backward/ \
 -i /usr/lib/gcc/x86_64-linux-gnu/11/include/ -i /usr/local/include/ -i /usr/include/x86_64-linux-gnu/ -i /usr/include/ \
 -p $SRC"

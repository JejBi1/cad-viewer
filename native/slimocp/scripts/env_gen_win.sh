# sourced: pywrap arguments for generating the *Windows* flavour of OCP from Linux (clang --target=x86_64-pc-windows-msvc),
# exactly like the official OCP CMake does for its Windows bindings, but with the local MSVC 14.44 + UCRT headers
# (symlinks without spaces) and the stub <windows.h> shipped in OCP/extern/include.
source /root/slimocp/scripts/env_gen.sh
export PYWRAP_ARGS="-l $CONDA_PREFIX/lib/libclang.so \
 -i $CONDA_PREFIX/lib/clang/$CLANGVER/include/ \
 -i /root/slimocp/work/msvc_inc -i /root/slimocp/work/ucrt_inc -i $SRC/extern/include/ \
 -i $CONDA_PREFIX/include -i $CONDA_PREFIX/include/freetype2 \
 -p $SRC"

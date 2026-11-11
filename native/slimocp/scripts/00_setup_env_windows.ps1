# Windows: conda-forge env with occt 8.0.1 (novtk), python 3.11, pybind11 2.13 (MSVC compiles OCP.pyd)
$env:MAMBA_ROOT_PREFIX = "C:\slimocp_w\mm"
$mm = "$env:LOCALAPPDATA\micromamba\micromamba.exe"
& $mm create -y -n bwin -c conda-forge python=3.11 "occt=8.0.1=novtk*" "pybind11=2.13.*" cmake ninja fmt pefile

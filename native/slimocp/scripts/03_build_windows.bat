@echo off
rem Build the slim OCP.pyd with MSVC 2022 + conda-forge occt 8.0.1 (novtk) + python 3.11.
rem usage: 03_build_windows.bat [jobs]      sources: C:\slimocp_w\gen\OCP (copied from WSL /root/slimocp/work/windows/gen/OCP)
setlocal
set JOBS=%1
if "%JOBS%"=="" set JOBS=8
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set P=C:\slimocp_w\mm\envs\bwin
set PATH=%P%;%P%\Library\bin;%P%\Library\mingw-w64\bin;%P%\Scripts;%PATH%
cd /d C:\slimocp_w
if not exist build\build.ninja (
  cmake -S C:\slimocp_w\gen\OCP -B C:\slimocp_w\build -G Ninja -DCMAKE_BUILD_TYPE=Release ^
    -DCMAKE_C_COMPILER=cl -DCMAKE_CXX_COMPILER=cl ^
    -DCMAKE_PREFIX_PATH=%P%\Library;%P% -DPython_EXECUTABLE=%P%\python.exe -DPython_FIND_STRATEGY=LOCATION ^
    -DOpenCASCADE_DIR=%P%\Library\lib\cmake\opencascade -Dpybind11_DIR=%P%\Library\share\cmake\pybind11
  if errorlevel 1 exit /b 1
)
cmake --build C:\slimocp_w\build -j %JOBS% -- -k 50
exit /b %errorlevel%

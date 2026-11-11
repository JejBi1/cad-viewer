@echo off
rem Builds the rdxf wheel (Rust + PyO3, abi3 for Python 3.11+), copies it to native\rdxf\wheels\ (prebuilt wheel, so that a
rem release build needs no Rust) and installs it into the release build venv.
rem Without Rust: just  %USERPROFILE%\.venvs\stepviewer\Scripts\pip install --no-deps native\rdxf\wheels\rdxf-*-win_amd64.whl
rem   native\rdxf\build_win.bat            (needs rustup/cargo and the Visual Studio C++ Build Tools, both already
rem                                         there for the 1.8 toolchain)
rem maturin lives in a separate tools venv (not in the release venv, so it never gets near the bundle) and the Cargo
rem target folder is outside SynologyDrive (several hundred MB of build files the sync client would upload).
setlocal
set "HERE=%~dp0"
set "VENV=%USERPROFILE%\.venvs\stepviewer"
set "TOOLS=%USERPROFILE%\.venvs\rdxf-tools"
set "MATURIN_VERSION=1.16.0"
set "CARGO_TARGET_DIR=%USERPROFILE%\.venvs\rdxf-target"
set "WHEELS=%USERPROFILE%\.venvs\rdxf-wheels"
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

if not exist "%VENV%\Scripts\python.exe" (echo release venv %VENV% not found & exit /b 1)
set "VCVARS="
for /f "usebackq tokens=*" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2^>nul`) do set "VCVARS=%%i\VC\Auxiliary\Build\vcvars64.bat"
if not defined VCVARS set "VCVARS=%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
call "%VCVARS%" >nul || (echo vcvars64.bat not found & exit /b 1)

if not exist "%TOOLS%\Scripts\maturin.exe" (
    "%VENV%\Scripts\python.exe" -m venv "%TOOLS%" || exit /b 1
    "%TOOLS%\Scripts\python.exe" -m pip install -q maturin==%MATURIN_VERSION% || exit /b 1
)
rem maturin (not "pip install ."): the wheel is built from a copy, nothing is written into the synced source folder
set "SRC=%TEMP%\rdxf-src"
if exist "%SRC%" rmdir /s /q "%SRC%"
mkdir "%SRC%" && xcopy /e /i /q "%HERE%src" "%SRC%\src" >nul && copy /y "%HERE%Cargo.toml" "%SRC%\" >nul && copy /y "%HERE%Cargo.lock" "%SRC%\" >nul && copy /y "%HERE%pyproject.toml" "%SRC%\" >nul || exit /b 1
pushd "%SRC%"
"%TOOLS%\Scripts\maturin.exe" build --release --locked -o "%WHEELS%" -i "%VENV%\Scripts\python.exe" || (popd & exit /b 1)
popd
for /f "delims=" %%w in ('dir /b /o-d "%WHEELS%\rdxf-*win_amd64.whl"') do (set "WHEEL=%WHEELS%\%%w" & goto :install)
echo no wheel built & exit /b 1
:install
if not exist "%HERE%wheels" mkdir "%HERE%wheels"
copy /y "%WHEEL%" "%HERE%wheels\" >nul
"%VENV%\Scripts\python.exe" -m pip install --force-reinstall --no-deps "%WHEEL%" || exit /b 1
"%VENV%\Scripts\python.exe" -c "import rdxf; print('rdxf OK, API', rdxf.API)"

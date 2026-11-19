# Dot-source before cargo:  . rust\tools\devenv.ps1 [target-suffix]
# User-space toolchain (no admin): MSVC + Windows SDK in ~/.local/msvc-portable, OCCT 8.0.1 in ~/.local/occt-env.
# The Cargo target folder stays outside SynologyDrive.
param([string]$Suffix = "main")
$msvc = Join-Path $env:USERPROFILE ".local\msvc-portable"
$e = Get-Content (Join-Path $msvc "env.json") -Raw -Encoding UTF8 | ConvertFrom-Json
$env:INCLUDE = $e.INCLUDE
$env:LIB = $e.LIB
$env:OCCT_PREFIX = Join-Path $env:USERPROFILE ".local\occt-env\Library"
$env:CARGO_TARGET_DIR = Join-Path $env:USERPROFILE ".venvs\cad-target-$Suffix"
$env:PATH = "$env:USERPROFILE\.cargo\bin;$($e.PATH);$env:OCCT_PREFIX\bin;$env:PATH"

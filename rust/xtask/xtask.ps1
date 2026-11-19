# Runs `cargo xtask` from any folder with the user-space toolchain (rust/tools/devenv.ps1, no admin rights).
#   .\rust\xtask\xtask.ps1 occt-static
#   .\rust\xtask\xtask.ps1 dist [--no-dwg]
# The Cargo target folder of xtask itself is ~/.venvs/cad-target-xtask (outside SynologyDrive).
# (No $ErrorActionPreference = "Stop": Windows PowerShell 5.1 turns cargo's stderr warnings into errors then.)
$rust = Split-Path -Parent $PSScriptRoot
. (Join-Path $rust "tools\devenv.ps1") "xtask"
cargo run --release --quiet --manifest-path (Join-Path $rust "Cargo.toml") --package xtask -- @args
exit $LASTEXITCODE

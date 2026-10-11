# Builds the Windows recorder (recorder-win/) and places it where Tauri expects its sidecar.
#
#   scripts/build-recorder-windows.ps1
#
# Produces app/src-tauri/binaries/gatit-recorder-x86_64-pc-windows-msvc.exe.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$triple = "x86_64-pc-windows-msvc"

cargo build --release --manifest-path "$root/recorder-win/Cargo.toml"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$out = "$root/app/src-tauri/binaries"
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "$root/recorder-win/target/release/gatit-recorder.exe" "$out/gatit-recorder-$triple.exe" -Force
Write-Host "built app/src-tauri/binaries/gatit-recorder-$triple.exe"

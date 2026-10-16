$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
if ($env:GATIT_MODE -eq 'release') {
    $names = if ($env:WINDOWS_SIGNING_PROVIDER -eq 'signpath-foundation') {
        @('gatit-recorder')
    } else { @('gatit-recorder', 'whisper-cli', 'whisper-quantize') }
    foreach ($name in $names) {
        & "$PSScriptRoot/sign-windows.ps1" -Path "$root/app/src-tauri/binaries/$name-x86_64-pc-windows-msvc.exe"
    }
    if ($env:WINDOWS_SIGNING_PROVIDER -eq 'azure-artifact-signing') {
        python "$root/scripts/upstream-integrity.py" create "$root/app/src-tauri/binaries" "$root/app/src-tauri/binaries/whisper-provenance.json"
        if ($LASTEXITCODE -ne 0) { throw 'Failed to record signed upstream hashes' }
    }
}
Push-Location "$root/app/src-tauri"
try {
    cargo tauri build --bundles nsis --config ../../build-config.json
    if ($LASTEXITCODE -ne 0) { throw 'Tauri Windows build failed' }
} finally { Pop-Location }

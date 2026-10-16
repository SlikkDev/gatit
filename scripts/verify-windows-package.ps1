$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
$installer = @(Get-ChildItem "$root/app/src-tauri/target/release/bundle/nsis/*-setup.exe")
if ($installer.Count -ne 1) { throw 'Expected one NSIS installer' }
& "$PSScriptRoot/verify-windows.ps1" -Path $installer[0].FullName
& "$PSScriptRoot/verify-windows.ps1" -Path "$root/app/src-tauri/target/release/gatit.exe"
$binaryDir = "$root/app/src-tauri/binaries"
& "$PSScriptRoot/verify-windows.ps1" -Path "$binaryDir/gatit-recorder-x86_64-pc-windows-msvc.exe"
if ($env:WINDOWS_SIGNING_PROVIDER -eq 'signpath-foundation') {
    # The uninstaller is signed by NSIS !uninstfinalize before embedding. Its
    # actual verified bytes are retained by the signing hook; never execute it.
    foreach ($role in 'app', 'recorder', 'uninstaller', 'installer') {
        & "$PSScriptRoot/verify-windows.ps1" -Path "$root/.build/signing-receipts/$role.exe"
    }
    python "$PSScriptRoot/upstream-integrity.py" verify $binaryDir "$binaryDir/whisper-provenance.json" --suffixed
    if ($LASTEXITCODE -ne 0) { throw 'Whisper bytes changed before packaging' }
    & "$PSScriptRoot/inspect-windows-payload.ps1"
    $extract = "$root/.build/nsis-verified"
    foreach ($name in 'gatit', 'gatit-recorder') {
        $matches = @(Get-ChildItem $extract -Recurse -File -Filter "$name.exe")
        if ($matches.Count -ne 1) { throw "Expected one packaged $name executable" }
        & "$PSScriptRoot/verify-windows.ps1" -Path $matches[0].FullName
        $role = if ($name -eq 'gatit') { 'app' } else { 'recorder' }
        if ((Get-FileHash $matches[0].FullName).Hash -cne (Get-FileHash "$root/.build/signing-receipts/$role.exe").Hash) {
            throw "Packaged $name does not match the signed source build"
        }
    }
} else {
    foreach ($name in 'whisper-cli', 'whisper-quantize') {
        & "$PSScriptRoot/verify-windows.ps1" -Path "$binaryDir/$name-x86_64-pc-windows-msvc.exe"
    }
}

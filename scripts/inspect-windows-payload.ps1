# Static extraction only: this never launches the installer or any payload.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
$installer = @(Get-ChildItem "$root/app/src-tauri/target/release/bundle/nsis/*-setup.exe")
if ($installer.Count -ne 1) { throw 'Expected one NSIS installer' }
if ($env:GATIT_MODE -eq 'ci') {
    $records = @(Get-Content "$root/.build/ci-signing-inputs.jsonl" | ForEach-Object { $_ | ConvertFrom-Json })
    foreach ($role in 'app', 'uninstaller', 'installer', 'NSISdl.dll', 'StartMenu.dll', 'System.dll', 'nsDialogs.dll', 'nsis_tauri_utils.dll') {
        if (@($records | Where-Object role -eq $role).Count -ne 1) {
            throw "Missing/duplicated real Tauri/NSIS callback: $role"
        }
    }
}
$extract = "$root/.build/nsis-verified"
if (Test-Path $extract) { Remove-Item $extract -Recurse -Force }
& 7z x $installer[0].FullName "-o$extract" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Cannot statically extract NSIS payload' }
$receipt = "$root/app/src-tauri/binaries/whisper-provenance.json"
# The Azure adapter signs upstream files and changes their hashes. The free path
# and ordinary CI preserve the exact upstream build bytes.
if ($env:WINDOWS_SIGNING_PROVIDER -ne 'azure-artifact-signing') {
    python "$PSScriptRoot/upstream-integrity.py" verify $extract $receipt
    if ($LASTEXITCODE -ne 0) { throw 'Packaged Whisper bytes differ from this source build' }
    $packaged = @(Get-ChildItem $extract -Recurse -File -Filter 'whisper-provenance.json')
    if ($packaged.Count -ne 1 -or (Get-FileHash $receipt).Hash -cne (Get-FileHash $packaged[0].FullName).Hash) {
        throw 'Missing or altered packaged upstream provenance'
    }
}
foreach ($name in 'gatit', 'gatit-recorder') {
    $files = @(Get-ChildItem $extract -Recurse -File -Filter "$name.exe")
    if ($files.Count -ne 1 -or $files[0].VersionInfo.ProductName -cne 'Gatit' -or
        $files[0].VersionInfo.ProductVersion -cne $env:GATIT_VERSION) {
        throw "Missing/incorrect first-party product metadata: $name"
    }
}
Write-Host 'NSIS payload metadata and upstream provenance validated without execution.'

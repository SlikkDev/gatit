# Unsigned CI only: exercise Tauri/NSIS callbacks and inspect metadata, never sign.
param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GATIT_MODE -ne 'ci') { throw 'Inspection callback cannot replace release signing' }
$root = Split-Path -Parent $PSScriptRoot
$file = Get-Item -LiteralPath $Path
$hash = (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
if ($file.Name -cmatch '^whisper-(cli|quantize)-x86_64-pc-windows-msvc\.exe$') {
    python "$PSScriptRoot/upstream-integrity.py" verify $file.DirectoryName "$root/app/src-tauri/binaries/whisper-provenance.json" --suffixed
    if ($LASTEXITCODE -ne 0) { throw 'Wrong upstream signing input' }
    $role = $file.Name
} elseif ($file.Extension -ieq '.dll') {
    $lock = Get-Content "$root/packaging/windows/nsis-upstream.json" -Raw | ConvertFrom-Json -AsHashtable
    if (-not $lock.files.ContainsKey($file.Name) -or $hash -cne $lock.files[$file.Name]) {
        throw "Unexpected upstream helper in CI: $($file.Name)"
    }
    $role = $file.Name
} else {
    if ($file.VersionInfo.ProductName -cne 'Gatit' -or $file.VersionInfo.ProductVersion -cne $env:GATIT_VERSION) {
        throw "Wrong first-party metadata on signing input: $($file.Name)"
    }
    $role = switch -Regex ($file.Name) {
        '^gatit\.exe$' { 'app'; break }
        '^gatit-recorder-x86_64-pc-windows-msvc\.exe$' { 'recorder'; break }
        '-setup\.exe$' { 'installer'; break }
        default { 'uninstaller' }
    }
}
New-Item "$root/.build" -ItemType Directory -Force | Out-Null
@{ role=$role; sha256=$hash } | ConvertTo-Json -Compress | Add-Content "$root/.build/ci-signing-inputs.jsonl"
Write-Host "Inspected unsigned CI signing input: $role"

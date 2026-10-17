# Policy tests use inert fixtures and hash stubs; no signing service or PE executes.
$ErrorActionPreference = 'Stop'
$hook = Join-Path $PSScriptRoot '../sign-windows.ps1'
$lock = Get-Content (Join-Path $PSScriptRoot '../../packaging/windows/nsis-upstream.json') -Raw | ConvertFrom-Json -AsHashtable
$temporary = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
New-Item $temporary -ItemType Directory | Out-Null
$env:WINDOWS_SIGNING_PROVIDER = 'signpath-foundation'
function Assert-Blocked([string]$Path, [string]$ExpectedMessage = '') {
    $blocked = $false
    try { & $hook -Path $Path } catch {
        if ($ExpectedMessage -and $_.Exception.Message -notlike "*$ExpectedMessage*") { throw }
        $blocked = $true
    }
    if (-not $blocked) { throw "Unexpected signing exception accepted: $Path" }
}
try {
    foreach ($name in $lock.files.Keys) { Set-Content (Join-Path $temporary $name) 'inert fixture' }
    Remove-Item Env:SIGNPATH_NSIS_UPSTREAM_APPROVED -ErrorAction SilentlyContinue
    foreach ($name in $lock.files.Keys) {
        Assert-Blocked (Join-Path $temporary $name) 'awaits explicit owner approval'
    }
    $env:SIGNPATH_NSIS_UPSTREAM_APPROVED = 'false'
    foreach ($name in $lock.files.Keys) {
        Assert-Blocked (Join-Path $temporary $name) 'awaits explicit owner approval'
    }
    $env:SIGNPATH_NSIS_UPSTREAM_APPROVED = 'true'
    # Approval never allows altered bytes, for any of the five helpers.
    foreach ($name in $lock.files.Keys) {
        Assert-Blocked (Join-Path $temporary $name) 'Unknown or modified upstream NSIS helper'
    }
    $unknown = Join-Path $temporary 'unapproved.dll'
    Set-Content $unknown 'inert fixture'
    Assert-Blocked $unknown 'Unknown or modified upstream NSIS helper'
    # Positive cases inject only the exact reviewed hash, never execute DLLs.
    function Get-FileHash { param([string]$LiteralPath, [string]$Path, [string]$Algorithm)
        $name = Split-Path -Leaf $(if ($Path) { $Path } else { $LiteralPath })
        [pscustomobject]@{Hash=$lock.files[$name].ToUpperInvariant()}
    }
    foreach ($name in $lock.files.Keys) { & $hook -Path (Join-Path $temporary $name) }
    # The approved exception must not let any unsigned first-party fixture pass.
    foreach ($name in 'gatit.exe', 'gatit-recorder-x86_64-pc-windows-msvc.exe', 'temporary-uninstaller.exe', 'Gatit_0.2.10_x64-setup.exe') {
        $firstParty = Join-Path $temporary $name
        Set-Content $firstParty 'inert unsigned first-party fixture'
        Assert-Blocked $firstParty 'Only first-party Gatit executables'
    }
    Write-Host 'All five exceptions enforce approval and hashes; unknown helpers and unsigned first-party fixtures are blocked.'
} finally {
    Remove-Item $temporary -Recurse -Force
    Remove-Item Env:WINDOWS_SIGNING_PROVIDER, Env:SIGNPATH_NSIS_UPSTREAM_APPROVED -ErrorAction SilentlyContinue
}

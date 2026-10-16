# Policy tests use inert fixtures and hash stubs; no signing service or PE executes.
$ErrorActionPreference = 'Stop'
$hook = Join-Path $PSScriptRoot '../sign-windows.ps1'
$lock = Get-Content (Join-Path $PSScriptRoot '../../packaging/windows/nsis-upstream.json') -Raw | ConvertFrom-Json -AsHashtable
$temporary = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
New-Item $temporary -ItemType Directory | Out-Null
$env:WINDOWS_SIGNING_PROVIDER = 'signpath-foundation'
function Assert-Blocked([string]$Path) {
    $blocked = $false
    try { & $hook -Path $Path } catch { $blocked = $true }
    if (-not $blocked) { throw "Unexpected signing exception accepted: $Path" }
}
try {
    foreach ($name in $lock.files.Keys) { Set-Content (Join-Path $temporary $name) 'inert fixture' }
    $known = Join-Path $temporary 'NSISdl.dll'
    Remove-Item Env:SIGNPATH_NSIS_UPSTREAM_APPROVED -ErrorAction SilentlyContinue
    Assert-Blocked $known
    $env:SIGNPATH_NSIS_UPSTREAM_APPROVED = 'true'
    # Actual fixture hash differs from the pinned upstream DLL.
    Assert-Blocked $known
    $unknown = Join-Path $temporary 'unapproved.dll'
    Set-Content $unknown 'inert fixture'
    Assert-Blocked $unknown
    # Positive cases inject only the exact reviewed hash, never execute DLLs.
    function Get-FileHash { param([string]$LiteralPath, [string]$Path, [string]$Algorithm)
        $name = Split-Path -Leaf $(if ($Path) { $Path } else { $LiteralPath })
        [pscustomobject]@{Hash=$lock.files[$name].ToUpperInvariant()}
    }
    foreach ($name in $lock.files.Keys) { & $hook -Path (Join-Path $temporary $name) }
    Write-Host 'Upstream signing exceptions reject missing approval, altered bytes and unknown helpers.'
} finally {
    Remove-Item $temporary -Recurse -Force
    Remove-Item Env:WINDOWS_SIGNING_PROVIDER, Env:SIGNPATH_NSIS_UPSTREAM_APPROVED -ErrorAction SilentlyContinue
}

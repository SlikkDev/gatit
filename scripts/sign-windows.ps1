# Tauri calls this for the app, NSIS uninstaller and installer. The workflow also
# calls it for each native sidecar before bundling. Do not sign after updater .sig creation.
param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:WINDOWS_SIGNING_PROVIDER -eq 'signpath-foundation') {
    $root = Split-Path -Parent $PSScriptRoot
    $file = Get-Item -LiteralPath $Path
    # Tauri also invokes its hook for unsigned externalBin sidecars. Preserve the
    # same two approved upstream binaries here, after verifying this build's receipt.
    if ($file.Name -cmatch '^whisper-(cli|quantize)-x86_64-pc-windows-msvc\.exe$') {
        python "$PSScriptRoot/upstream-integrity.py" verify $file.DirectoryName "$root/app/src-tauri/binaries/whisper-provenance.json" --suffixed
        if ($LASTEXITCODE -ne 0) { throw 'Upstream Whisper signing input differs from the pinned build' }
        Write-Host "Preserved verified upstream binary without re-signing: $($file.Name)"
        return
    }
    if ($file.Extension -ieq '.dll') {
        if ($env:SIGNPATH_NSIS_UPSTREAM_APPROVED -ne 'true') {
            throw 'NSIS upstream DLL exception awaits explicit owner approval'
        }
        $lock = Get-Content "$root/packaging/windows/nsis-upstream.json" -Raw | ConvertFrom-Json -AsHashtable
        if (-not $lock.files.ContainsKey($file.Name) -or
            (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -cne $lock.files[$file.Name]) {
            throw "Unknown or modified upstream NSIS helper: $($file.Name)"
        }
        Write-Host "Preserved exact pinned upstream helper without re-signing: $($file.Name)"
        return
    }
    if ($file.VersionInfo.ProductName -cne 'Gatit' -or $file.VersionInfo.ProductVersion -cne $env:GATIT_VERSION) {
        throw 'Only first-party Gatit executables with this build version may be sent to SignPath'
    }
    if (-not $env:GATIT_NODE) { throw 'Signing must run inside the GitHub build action' }
    & $env:GATIT_NODE "$PSScriptRoot/signpath-request.cjs" $file.FullName
    if ($LASTEXITCODE -ne 0) { throw 'SignPath request failed' }
    & "$PSScriptRoot/verify-windows.ps1" -Path $file.FullName
    $role = switch -Regex ($file.Name) {
        '^gatit\.exe$' { 'app'; break }
        '^gatit-recorder-x86_64-pc-windows-msvc\.exe$' { 'recorder'; break }
        '-setup\.exe$' { 'installer'; break }
        # NSIS !uninstfinalize supplies an unpredictable temporary filename.
        # The ProductName/version restrictions above still apply to that PE.
        default { 'uninstaller' }
    }
    $receipts = "$root/.build/signing-receipts"
    New-Item $receipts -ItemType Directory -Force | Out-Null
    Copy-Item $file.FullName "$receipts/$role.exe" -Force
    return
}
if ($env:WINDOWS_SIGNING_PROVIDER -ne 'azure-artifact-signing') {
    throw 'Select an eligible WINDOWS_SIGNING_PROVIDER and implement its adapter before signing'
}
foreach ($name in 'AZURE_SIGNING_ENDPOINT', 'AZURE_SIGNING_ACCOUNT', 'AZURE_CERTIFICATE_PROFILE', 'WINDOWS_PUBLISHER_SUBJECT') {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Missing release configuration: $name" }
}
Import-Module ArtifactSigning -RequiredVersion 0.1.20 -ErrorAction Stop
$parameters = @{
    Endpoint = $env:AZURE_SIGNING_ENDPOINT
    CodeSigningAccountName = $env:AZURE_SIGNING_ACCOUNT
    CertificateProfileName = $env:AZURE_CERTIFICATE_PROFILE
    Files = (Resolve-Path -LiteralPath $Path).Path
    FileDigest = 'SHA256'
    TimestampRfc3161 = 'http://timestamp.acs.microsoft.com'
    TimestampDigest = 'SHA256'
    # azure/login obtains a short-lived token via GitHub OIDC.
    ExcludeAzureCliCredential = $false
    ExcludeEnvironmentCredential = $true
    ExcludeWorkloadIdentityCredential = $true
    ExcludeManagedIdentityCredential = $true
    ExcludeSharedTokenCacheCredential = $true
    ExcludeVisualStudioCredential = $true
    ExcludeVisualStudioCodeCredential = $true
    ExcludeAzurePowerShellCredential = $true
    ExcludeAzureDeveloperCliCredential = $true
    ExcludeInteractiveBrowserCredential = $true
}
Invoke-ArtifactSigning @parameters
& "$PSScriptRoot/verify-windows.ps1" -Path $Path

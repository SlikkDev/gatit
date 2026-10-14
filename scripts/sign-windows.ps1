# Tauri calls this for the app, NSIS uninstaller and installer. The workflow also
# calls it for each native sidecar before bundling. Do not sign after updater .sig creation.
param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
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

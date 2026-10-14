param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $env:WINDOWS_PUBLISHER_SUBJECT) { throw 'Missing WINDOWS_PUBLISHER_SUBJECT' }
$signature = Get-AuthenticodeSignature -LiteralPath $Path
if ($signature.Status -ne 'Valid') { throw "Authenticode verification failed for $Path : $($signature.Status)" }
if ($signature.SignerCertificate.Subject -cne $env:WINDOWS_PUBLISHER_SUBJECT) {
    throw "Unexpected signing publisher for $Path"
}
if (-not $signature.TimeStamperCertificate) { throw "Missing trusted timestamp for $Path" }
if ('1.3.6.1.5.5.7.3.3' -notin $signature.SignerCertificate.EnhancedKeyUsageList.ObjectId.Value) {
    throw "Missing code-signing certificate usage for $Path"
}
Write-Host "Verified Authenticode signature and timestamp: $Path"

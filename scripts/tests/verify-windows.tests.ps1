# Exercises the verifier without certificates, signing access, or executing a binary.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$global:GatitTestSignature = $null
function Get-AuthenticodeSignature { param([string]$LiteralPath) return $global:GatitTestSignature }
$env:WINDOWS_PUBLISHER_SUBJECT = 'CN=Expected Publisher'
$verify = Join-Path $PSScriptRoot '../verify-windows.ps1'
function New-ValidSignature {
    [pscustomobject]@{
        Status = 'Valid'
        SignerCertificate = [pscustomobject]@{
            Subject = 'CN=Expected Publisher'
            EnhancedKeyUsageList = @([pscustomobject]@{ ObjectId = [pscustomobject]@{ Value = '1.3.6.1.5.5.7.3.3' } })
        }
        TimeStamperCertificate = [pscustomobject]@{ Subject = 'CN=Timestamp' }
    }
}
function Assert-Rejected([scriptblock]$Change) {
    $global:GatitTestSignature = New-ValidSignature
    & $Change
    $failed = $false
    try { & $verify -Path 'test.exe' } catch { $failed = $true }
    if (-not $failed) { throw 'Invalid signature was accepted' }
}
$global:GatitTestSignature = New-ValidSignature
& $verify -Path 'test.exe'
Assert-Rejected { $global:GatitTestSignature.Status = 'NotSigned' }
Assert-Rejected { $global:GatitTestSignature.Status = 'HashMismatch' }
Assert-Rejected { $global:GatitTestSignature.Status = 'NotTrusted' }
Assert-Rejected { $global:GatitTestSignature.SignerCertificate.Subject = 'CN=Unexpected Publisher' }
Assert-Rejected { $global:GatitTestSignature.TimeStamperCertificate = $null }
Assert-Rejected { $global:GatitTestSignature.SignerCertificate.EnhancedKeyUsageList[0].ObjectId.Value = '1.2.3' }
Remove-Item Env:WINDOWS_PUBLISHER_SUBJECT
Assert-Rejected { }
# Parse every PowerShell script as well, including the custom signing hook.
Get-ChildItem (Join-Path $PSScriptRoot '..') -Filter '*.ps1' | ForEach-Object {
    $parseErrors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($_.FullName, [ref]$null, [ref]$parseErrors) | Out-Null
    if ($parseErrors) { throw ($parseErrors | Out-String) }
}
Write-Host 'Authenticode verifier failure-path tests passed.'

Remove-Variable GatitTestSignature -Scope Global

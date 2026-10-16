# Package already-built Store-edition binaries. Never signs, installs or executes them.
param(
    [Parameter(Mandatory)][string]$Version,
    [switch]$Submission,
    [string]$OutputDirectory = 'store-artifacts'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
$packageVersion = & "$PSScriptRoot/msix-version.ps1" -Version $Version
if ($Submission) {
    foreach ($name in 'STORE_IDENTITY_NAME', 'STORE_PUBLISHER_ID', 'STORE_PUBLISHER_DISPLAY_NAME') {
        if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Missing $name from Partner Center" }
    }
    $identity = $env:STORE_IDENTITY_NAME
    $publisher = $env:STORE_PUBLISHER_ID
    $display = $env:STORE_PUBLISHER_DISPLAY_NAME
} else {
    $identity = 'Gatit.CI.Preview'
    $publisher = 'CN=Gatit CI Preview'
    $display = 'Gatit CI Preview (not for submission)'
}
if ($identity -notmatch '^[A-Za-z0-9.-]{3,50}$' -or -not $publisher.StartsWith('CN=')) {
    throw 'Invalid Store identity; copy the exact values from Partner Center'
}
$stage = Join-Path $root '.build/msix-stage'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item "$stage/Assets" -ItemType Directory -Force | Out-Null
$binaryDir = Join-Path $root 'app/src-tauri/binaries'
Copy-Item "$root/app/src-tauri/target/release/gatit.exe" "$stage/gatit.exe"
foreach ($name in 'gatit-recorder', 'whisper-cli', 'whisper-quantize') {
    Copy-Item "$binaryDir/$name-x86_64-pc-windows-msvc.exe" "$stage/$name.exe"
}
Copy-Item "$root/LICENSE" "$stage/LICENSE.txt"
Copy-Item "$binaryDir/whisper-LICENSE.txt" "$stage/whisper-LICENSE.txt"
Copy-Item "$binaryDir/whisper-provenance.json" "$stage/whisper-provenance.json"
# Tauri statically links its WebView2 loader by default. Preserve a dynamic loader
# if a future build emits one; no third-party file is re-signed here.
$loader = "$root/app/src-tauri/target/release/WebView2Loader.dll"
if (Test-Path $loader) { Copy-Item $loader $stage }
python "$root/scripts/upstream-integrity.py" verify $stage "$stage/whisper-provenance.json"
if ($LASTEXITCODE -ne 0) { throw 'Upstream provenance verification failed' }
$xml = Get-Content "$root/packaging/windows/AppxManifest.xml.in" -Raw
foreach ($entry in @{
    '@IDENTITY@' = $identity; '@PUBLISHER@' = $publisher;
    '@PUBLISHER_DISPLAY@' = $display; '@VERSION@' = $packageVersion
}.GetEnumerator()) {
    $xml = $xml.Replace($entry.Key, [System.Security.SecurityElement]::Escape($entry.Value))
}
[xml]$parsed = $xml
$xml | Set-Content "$stage/AppxManifest.xml" -Encoding utf8
Add-Type -AssemblyName System.Drawing
$source = [System.Drawing.Image]::FromFile("$root/app/src-tauri/icons/icon.png")
try {
    foreach ($entry in @{StoreLogo=50; Square44x44Logo=44; Square150x150Logo=150}.GetEnumerator()) {
        $bitmap = [System.Drawing.Bitmap]::new($entry.Value, $entry.Value)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.DrawImage($source, 0, 0, $entry.Value, $entry.Value)
            $bitmap.Save("$stage/Assets/$($entry.Key).png", [System.Drawing.Imaging.ImageFormat]::Png)
        } finally { $graphics.Dispose(); $bitmap.Dispose() }
    }
} finally { $source.Dispose() }
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/makeappx.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $sdk) { throw 'Install the Windows SDK (MakeAppx)' }
$out = Join-Path $root $OutputDirectory
New-Item $out -ItemType Directory -Force | Out-Null
$name = if ($Submission) { 'Gatit-StoreSubmission.msix' } else { 'Gatit-CI-Preview.msix' }
$package = Join-Path $out $name
& $sdk.FullName pack /d $stage /p $package /o
if ($LASTEXITCODE -ne 0) { throw 'MakeAppx validation/pack failed' }
$unpacked = Join-Path $root '.build/msix-unpacked'
if (Test-Path $unpacked) { Remove-Item $unpacked -Recurse -Force }
& $sdk.FullName unpack /p $package /d $unpacked /o
if ($LASTEXITCODE -ne 0) { throw 'MakeAppx unpack failed' }
foreach ($file in Get-ChildItem $stage -Recurse -File) {
    $relative = [IO.Path]::GetRelativePath($stage, $file.FullName)
    if ((Get-FileHash $file.FullName).Hash -cne (Get-FileHash (Join-Path $unpacked $relative)).Hash) {
        throw "Packaged bytes differ: $relative"
    }
}
@{
    status = 'unsigned-store-submission-only'; identity = $identity; version = $packageVersion;
    commit = $env:GITHUB_SHA; sha256 = (Get-FileHash $package).Hash.ToLowerInvariant();
    updater = 'Microsoft Store'; submission = [bool]$Submission
} | ConvertTo-Json | Set-Content "$package.verification.json" -Encoding utf8
Write-Host "Validated $package. Microsoft Store must certify and sign it before public distribution."

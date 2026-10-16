param([Parameter(Mandatory)][string]$Version)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)$') { throw 'Expected major.minor.patch' }
# Store requires a nonzero first field and a reserved zero fourth field.
$parts = @(([long]$Matches[1] + 1), [long]$Matches[2], [long]$Matches[3], 0)
if (@($parts | Where-Object { $_ -gt 65535 }).Count) { throw 'Version exceeds MSIX limits' }
$parts -join '.'

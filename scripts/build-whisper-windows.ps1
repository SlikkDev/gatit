# Builds whisper.cpp's command-line tools for Windows and places them where Tauri expects
# its sidecars.
#
#   scripts/build-whisper-windows.ps1
#
# Produces app/src-tauri/binaries/whisper-cli-x86_64-pc-windows-msvc.exe and
# whisper-quantize-x86_64-pc-windows-msvc.exe. The build is static (no ggml DLLs to ship),
# CPU-only with AVX2, and without OpenMP so it doesn't need the Visual C++ OpenMP runtime.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$triple = "x86_64-pc-windows-msvc"
$sourceLock = Get-Content "$root/packaging/windows/upstream.json" -Raw | ConvertFrom-Json

# Never reuse cached objects or source for the unsigned upstream exception.
$src = "$root/.build/whisper-$(New-Guid)"
git clone --depth 1 --branch $sourceLock.tag $sourceLock.repository $src
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$actualCommit = git -C $src rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $actualCommit -cne $sourceLock.commit) {
  throw 'Whisper tag no longer resolves to the reviewed commit'
}
$build = "$src/build-win64"
cmake -S $src -B $build -A x64 `
  -DCMAKE_BUILD_TYPE=Release `
  -DBUILD_SHARED_LIBS=OFF `
  -DGGML_NATIVE=OFF -DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON `
  -DGGML_OPENMP=OFF `
  -DWHISPER_BUILD_TESTS=OFF `
  -DWHISPER_BUILD_SERVER=OFF
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cmake --build $build --config Release --target whisper-cli whisper-quantize
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$out = "$root/app/src-tauri/binaries"
New-Item -ItemType Directory -Force $out | Out-Null
foreach ($tool in "whisper-cli", "whisper-quantize") {
  $exe = Get-ChildItem -Path "$build/bin" -Recurse -Filter "$tool.exe" | Select-Object -First 1
  if (-not $exe) { throw "$tool.exe was not built" }
  Copy-Item $exe.FullName "$out/$tool-$triple.exe" -Force
}
# Make sure nothing needs a DLL we don't ship.
if (Get-ChildItem -Path "$build/bin" -Recurse -Include "ggml*.dll", "whisper*.dll") {
  throw "whisper.cpp built shared libraries; expected a static build"
}
Write-Host "built $out/whisper-cli-$triple.exe and whisper-quantize-$triple.exe"
Copy-Item "$src/LICENSE" "$out/whisper-LICENSE.txt" -Force
python "$root/scripts/upstream-integrity.py" create $out "$out/whisper-provenance.json"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

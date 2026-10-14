#!/usr/bin/env bash
# Gathers the Windows build's downloads into the repository root for upload:
#   Gatit-windows-setup.exe        the installer, for people and for the app's updater
#   Gatit-windows-setup.exe.sig    its update signature (if the build could sign it)
#
#   scripts/ci-collect-windows.sh
set -euo pipefail
mode="${1:?ci or release}"
case "$mode" in ci|release) ;; *) exit 1 ;; esac
bundle="app/src-tauri/target/release/bundle/nsis"
set -- "$bundle"/*-setup.exe
[ "$#" -eq 1 ] && [ -s "$1" ] || { echo "Expected exactly one installer" >&2; exit 1; }
setup="$1"
cp "$setup" Gatit-windows-setup.exe
if [ -f "$setup.sig" ]; then
  cp "$setup.sig" Gatit-windows-setup.exe.sig
  echo "update package signed"
else
  echo "no update signature (no TAURI_SIGNING_PRIVATE_KEY)"
fi
ls -l Gatit-windows-setup.exe*

if [ "$mode" = release ]; then
  # Verification is repeated on the collected bytes, after the updater signature exists.
  pwsh -NoProfile -NonInteractive -File scripts/verify-windows.ps1 -Path Gatit-windows-setup.exe
  python scripts/release-manifest.py write Gatit-windows authenticode "$WINDOWS_PUBLISHER_SUBJECT" Gatit-windows-setup.exe Gatit-windows-setup.exe.sig
fi

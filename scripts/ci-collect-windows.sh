#!/usr/bin/env bash
# Gathers the Windows build's downloads into the repository root for upload:
#   Gatit-windows-setup.exe        the installer, for people and for the app's updater
#   Gatit-windows-setup.exe.sig    its update signature (if the build could sign it)
#
#   scripts/ci-collect-windows.sh
set -euo pipefail
bundle="app/src-tauri/target/release/bundle/nsis"
setup="$(ls "$bundle"/*-setup.exe | head -1)"
cp "$setup" Gatit-windows-setup.exe
if [ -f "$setup.sig" ]; then
  cp "$setup.sig" Gatit-windows-setup.exe.sig
  echo "update package signed"
else
  echo "no update signature (no TAURI_SIGNING_PRIVATE_KEY)"
fi
ls -l Gatit-windows-setup.exe*

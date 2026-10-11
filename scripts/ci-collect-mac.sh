#!/usr/bin/env bash
# Gathers one Mac build's downloads into the repository root for upload:
#   Gatit-macos-<slug>.zip                 the app, for people
#   Gatit-macos-<slug>.app.tar.gz (+.sig)  the update package, for the app itself (if built)
#   Gatit-macos-<slug>.signing             "notarized", "developer-id" or "ad-hoc", for the notes
#
#   scripts/ci-collect-mac.sh <target-triple> <slug>
set -euo pipefail
target="$1"; slug="$2"
bundle="app/src-tauri/target/$target/release/bundle/macos"
app="$bundle/Gatit.app"

ditto -c -k --keepParent "$app" "Gatit-macos-$slug.zip"
if [ -f "$bundle/Gatit.app.tar.gz.sig" ]; then
  cp "$bundle/Gatit.app.tar.gz" "Gatit-macos-$slug.app.tar.gz"
  cp "$bundle/Gatit.app.tar.gz.sig" "Gatit-macos-$slug.app.tar.gz.sig"
fi

if codesign -dv "$app" 2>&1 | grep -q "Authority=Developer ID Application"; then
  if xcrun stapler validate "$app" >/dev/null 2>&1; then
    echo notarized > "Gatit-macos-$slug.signing"
  else
    echo developer-id > "Gatit-macos-$slug.signing"
  fi
else
  echo ad-hoc > "Gatit-macos-$slug.signing"
fi
echo "signing: $(cat "Gatit-macos-$slug.signing")"
codesign -dv --entitlements - "$app" 2>&1 | head -20 || true

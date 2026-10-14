#!/usr/bin/env bash
# Validate the stapled app before archiving, and validate a fresh extraction too.
set -euo pipefail
target="${1:?target}"; slug="${2:?slug}"; mode="${3:?ci or release}"
case "$mode" in ci|release) ;; *) exit 1 ;; esac
bundle="app/src-tauri/target/$target/release/bundle/macos"
app="$bundle/Gatit.app"
stem="Gatit-macos-$slug"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if [ "$mode" = release ]; then
  scripts/verify-macos.sh "$app"
fi
ditto -c -k --keepParent "$app" "$stem.zip"
if [ "$mode" = release ]; then
  ditto -x -k "$stem.zip" "$tmp/zip"
  scripts/verify-macos.sh "$tmp/zip/Gatit.app"
fi
if [ -f "$bundle/Gatit.app.tar.gz.sig" ]; then
  test -s "$bundle/Gatit.app.tar.gz"
  cp "$bundle/Gatit.app.tar.gz" "$stem.app.tar.gz"
  cp "$bundle/Gatit.app.tar.gz.sig" "$stem.app.tar.gz.sig"
  if [ "$mode" = release ]; then
    mkdir "$tmp/update"
    tar -xzf "$stem.app.tar.gz" -C "$tmp/update"
    scripts/verify-macos.sh "$tmp/update/Gatit.app"
  fi
fi
if [ "$mode" = release ]; then
  python3 scripts/release-manifest.py write "$stem" notarized "$APPLE_TEAM_ID" "$stem.zip" "$stem".app.tar.gz*
fi

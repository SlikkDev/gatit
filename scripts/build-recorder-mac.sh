#!/usr/bin/env bash
# Builds the Swift recorder and places it where Tauri expects its sidecar binary.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
case "$triple" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "unsupported target: $triple" >&2; exit 1 ;;
esac
swift build -c release --arch "$arch" --package-path "$root/recorder-mac"
mkdir -p "$root/app/src-tauri/binaries"
cp "$(swift build -c release --arch "$arch" --package-path "$root/recorder-mac" --show-bin-path)/gatit-recorder" \
   "$root/app/src-tauri/binaries/gatit-recorder-$triple"
echo "built app/src-tauri/binaries/gatit-recorder-$triple"

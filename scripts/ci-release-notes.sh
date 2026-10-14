#!/usr/bin/env bash
# Writes <dist-dir>/latest.json (the feed the in-app updater polls) and release-notes.md.
#
#   scripts/ci-release-notes.sh <version> <dist-dir>
#
# latest.json lists every update package found in <dist-dir>, so a platform appears as soon
# as its build produces a signed package. Nothing is written when there are none, and the
# app's update check then finds nothing.
set -euo pipefail
version="${1:?version}"; dist="${2:?dist dir}"
python3 scripts/release-manifest.py verify "$dist"
repo="${GITHUB_REPOSITORY:-SlikkDev/gatit}"
base="https://github.com/$repo/releases/download/latest"

platforms='{}'
for sig in "$dist"/*.sig; do
  [ -e "$sig" ] || continue
  file="$(basename "${sig%.sig}")"
  case "$file" in
    Gatit-macos-intel.app.tar.gz) key=darwin-x86_64 ;;
    Gatit-macos-apple-silicon.app.tar.gz) key=darwin-aarch64 ;;
    Gatit-windows-setup.exe) key=windows-x86_64 ;;
    *) echo "unknown update package $file, skipped" >&2; continue ;;
  esac
  platforms="$(jq --arg k "$key" --arg url "$base/$file" --arg sig "$(cat "$sig")" \
    '. + {($k): {url: $url, signature: $sig}}' <<<"$platforms")"
done
if [ "$platforms" != "{}" ]; then
  jq -n --arg version "$version" --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --argjson platforms "$platforms" \
    '{version: $version, pub_date: $date, platforms: $platforms}' > "$dist/latest.json"
fi

{
  echo "Gatit $version, built from commit ${GITHUB_SHA:0:7}."
  echo
  echo "Pick **Gatit-macos-apple-silicon.zip**, **Gatit-macos-intel.zip**, or **Gatit-windows-setup.exe** for your machine."
  echo
  echo "Both Mac builds passed Developer ID, hardened runtime, timestamp, notarization ticket and Gatekeeper checks, including their embedded helpers and extracted download archives."
  echo "Windows binaries and the NSIS installer passed Authenticode publisher and timestamp checks. SmartScreen reputation and organization policy can still affect a newly signed release."
  echo "The verification JSON files record the build commit, publisher and SHA-256 hashes."
  echo
  if [ -f "$dist/latest.json" ]; then
    echo "All three platforms include signed updater packages. Gatit checks these with its embedded updater public key before installation."
  else
    echo "This release has no in-app update packages. Download the new version manually; see [signing setup](https://github.com/$repo/blob/main/docs/signing.md)."
  fi
} > release-notes.md
cat release-notes.md

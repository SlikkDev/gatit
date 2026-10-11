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

signing="$(cat "$dist"/Gatit-macos-*.signing 2>/dev/null | sort -u | tr '\n' ' ')"
for f in "$dist"/Gatit-macos-*.signing; do [ -e "$f" ] && rm "$f"; done
{
  echo "Gatit $version, built from commit ${GITHUB_SHA:0:7}."
  echo
  echo "Pick **Gatit-macos-apple-silicon.zip** for a Mac with an Apple chip, **Gatit-macos-intel.zip** for an Intel Mac, or **Gatit-windows-setup.exe** for Windows (SmartScreen asks once: More info, then Run anyway)."
  echo
  case "$signing" in
    *notarized*) echo "Signed with an Apple Developer ID and notarized: unzip, drag to Applications and open it like any app." ;;
    *developer-id*) echo "Signed with an Apple Developer ID but not notarized, so macOS still asks before the first launch: see [Getting the app](https://github.com/$repo#getting-the-app)." ;;
    *) echo "The app has only an ad-hoc signature, so macOS blocks the first launch: see [Getting the app](https://github.com/$repo#getting-the-app) for the two clicks that allow it." ;;
  esac
  echo
  if [ -f "$dist/latest.json" ]; then
    echo "Gatit updates itself: an installed copy picks this build up within a few hours and shows a Restart button. The \`.app.tar.gz\` files, the \`.sig\` files and \`latest.json\` are what it uses."
  else
    echo "Gatit's updater cannot pick this build up yet (the update signing key isn't in the repository's secrets, see [docs/signing.md](https://github.com/$repo/blob/main/docs/signing.md)), so download it by hand."
  fi
} > release-notes.md
cat release-notes.md

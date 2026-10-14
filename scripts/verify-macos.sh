#!/usr/bin/env bash
# Checks trust without launching the app or changing OS security settings.
set -euo pipefail
app="${1:?app bundle}"
: "${APPLE_TEAM_ID:?expected Apple Team ID}"
verify_code() {
  local metadata
  codesign --verify --strict --verbose=2 "$1"
  metadata="$(codesign -dvv "$1" 2>&1)"
  grep -q '^Authority=Developer ID Application:' <<< "$metadata"
  grep -Fxq "TeamIdentifier=$APPLE_TEAM_ID" <<< "$metadata"
  grep -q 'flags=.*runtime' <<< "$metadata"
  grep -q '^Timestamp=' <<< "$metadata"
}
codesign --verify --deep --strict --verbose=2 "$app"
verify_code "$app"
for executable in gatit gatit-recorder whisper-cli whisper-quantize; do
  verify_code "$app/Contents/MacOS/$executable"
done
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app"

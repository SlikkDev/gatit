#!/usr/bin/env bash
# Prints the Tauri config overrides for a CI build (`cargo tauri build --config`).
#
#   scripts/ci-build-config.sh <version>
#
# Signing: when APPLE_CERTIFICATE (a base64 .p12 with a "Developer ID Application" certificate)
# and APPLE_CERTIFICATE_PASSWORD are set, the app is signed with that identity under the
# hardened runtime, with the entitlements in app/src-tauri/Entitlements.plist, and Tauri
# notarizes it when APPLE_ID, APPLE_PASSWORD and APPLE_TEAM_ID are set too. Otherwise the build
# keeps the ad-hoc signature from tauri.macos.conf.json.
#
# Updates: when TAURI_SIGNING_PRIVATE_KEY is set, the build also produces the signed
# Gatit.app.tar.gz the in-app updater downloads.
set -euo pipefail
version="$1"
identity=""
if [ -n "${APPLE_CERTIFICATE:-}" ]; then
  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    identity="$APPLE_SIGNING_IDENTITY"
  else
    # Read the certificate's name from the .p12 so it doesn't have to be a secret of its own.
    tmp="$(mktemp -d)"
    keychain="$tmp/identity.keychain-db"
    echo "$APPLE_CERTIFICATE" | base64 --decode > "$tmp/cert.p12"
    security create-keychain -p "" "$keychain"
    security import "$tmp/cert.p12" -k "$keychain" -P "${APPLE_CERTIFICATE_PASSWORD:-}" -T /usr/bin/codesign >/dev/null
    identity="$(security find-identity -v -p codesigning "$keychain" | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)"
    security delete-keychain "$keychain"
    rm -rf "${tmp:?}"
    if [ -z "$identity" ]; then
      echo "APPLE_CERTIFICATE holds no \"Developer ID Application\" certificate" >&2
      exit 1
    fi
  fi
  echo "signing with: $identity" >&2
  if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
    echo "notarizing as $APPLE_ID" >&2
  else
    echo "APPLE_ID, APPLE_PASSWORD or APPLE_TEAM_ID missing: signed but not notarized" >&2
  fi
else
  echo "no APPLE_CERTIFICATE: ad-hoc signature" >&2
fi
updater=false
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  updater=true
else
  echo "no TAURI_SIGNING_PRIVATE_KEY: no update package" >&2
fi

jq -n --arg version "$version" --arg identity "$identity" --argjson updater "$updater" '
  { version: $version, bundle: { createUpdaterArtifacts: $updater } }
  + (if $identity == "" then {} else
      { bundle: { createUpdaterArtifacts: $updater,
                  macOS: { signingIdentity: $identity, hardenedRuntime: true, entitlements: "Entitlements.plist" } } }
    end)'

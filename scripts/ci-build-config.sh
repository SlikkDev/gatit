#!/usr/bin/env bash
# scripts/ci-build-config.sh <version> <macos|windows> <ci|release>
# CI artifacts never consume credentials. Releases require complete configuration.
set -euo pipefail
version="${1:?version}"; platform="${2:?platform}"; mode="${3:?mode}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Invalid version" >&2; exit 1; }
case "$platform" in macos|windows) ;; *) echo "Invalid platform" >&2; exit 1 ;; esac
case "$mode" in ci|release) ;; *) echo "Invalid mode" >&2; exit 1 ;; esac
require() {
  local missing=0 name
  for name in "$@"; do
    if [ -z "${!name:-}" ]; then echo "Missing release configuration: $name" >&2; missing=1; fi
  done
  [ "$missing" -eq 0 ]
}
updater=false
if [ "$mode" = release ]; then
  if [ "$platform" = macos ]; then
    require APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
    [[ "$APPLE_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || { echo "Invalid APPLE_TEAM_ID" >&2; exit 1; }
    [[ "$APPLE_SIGNING_IDENTITY" == "Developer ID Application: "*" ($APPLE_TEAM_ID)" ]] || {
      echo "APPLE_SIGNING_IDENTITY must be a Developer ID Application identity for APPLE_TEAM_ID" >&2; exit 1;
    }
  else
    require AZURE_CLIENT_ID AZURE_TENANT_ID AZURE_SUBSCRIPTION_ID AZURE_SIGNING_ENDPOINT AZURE_SIGNING_ACCOUNT AZURE_CERTIFICATE_PROFILE WINDOWS_PUBLISHER_SUBJECT
  fi
  [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || updater=true
fi
jq -n --arg version "$version" --arg platform "$platform" --arg mode "$mode" \
  --arg identity "${APPLE_SIGNING_IDENTITY:-}" --argjson updater "$updater" '
  {version: $version, bundle: {createUpdaterArtifacts: $updater}}
  | if $platform == "macos" then
      .bundle.macOS = (if $mode == "release" then
        {signingIdentity: $identity, hardenedRuntime: true, entitlements: "Entitlements.plist"}
        else {signingIdentity: "-", hardenedRuntime: false} end)
    elif $mode == "release" then
      .bundle.windows = {signCommand: {cmd: "pwsh", args:
        ["-NoProfile", "-NonInteractive", "-File", "../../scripts/sign-windows.ps1", "-Path", "%1"]}}
    else . end'

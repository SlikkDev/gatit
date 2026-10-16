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
# NSIS executes !uninstfinalize from its output directory, not src-tauri.
# Both the release signer and the CI inspection callback need native absolute paths.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
hook="$script_dir/sign-windows.ps1"
[ "$mode" != ci ] || hook="$script_dir/inspect-signing-input.ps1"
if command -v cygpath >/dev/null 2>&1; then hook="$(cygpath -w "$hook")"; fi
if [ "$mode" = release ]; then
  if [ "$platform" = macos ]; then
    require APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
    [[ "$APPLE_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || { echo "Invalid APPLE_TEAM_ID" >&2; exit 1; }
    [[ "$APPLE_SIGNING_IDENTITY" == "Developer ID Application: "*" ($APPLE_TEAM_ID)" ]] || {
      echo "APPLE_SIGNING_IDENTITY must be a Developer ID Application identity for APPLE_TEAM_ID" >&2; exit 1;
    }
  else
    require WINDOWS_SIGNING_PROVIDER
    case "$WINDOWS_SIGNING_PROVIDER" in
      azure-artifact-signing)
        require AZURE_CLIENT_ID AZURE_TENANT_ID AZURE_SUBSCRIPTION_ID AZURE_SIGNING_ENDPOINT AZURE_SIGNING_ACCOUNT AZURE_CERTIFICATE_PROFILE WINDOWS_PUBLISHER_SUBJECT ;;
      signpath-foundation)
        require SIGNPATH_API_TOKEN SIGNPATH_ORGANIZATION_ID SIGNPATH_PROJECT_SLUG SIGNPATH_SIGNING_POLICY_SLUG SIGNPATH_ARTIFACT_CONFIGURATION_SLUG WINDOWS_PUBLISHER_SUBJECT
        [ "${SIGNPATH_NSIS_UPSTREAM_APPROVED:-}" = true ] || {
          echo "SIGNPATH_NSIS_UPSTREAM_APPROVED requires owner approval of the pinned helper exceptions" >&2; exit 1;
        } ;;
      *) echo "Unsupported WINDOWS_SIGNING_PROVIDER" >&2; exit 1 ;;
    esac
  fi
  [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ] || updater=true
fi
jq -n --arg version "$version" --arg platform "$platform" --arg mode "$mode" \
  --arg identity "${APPLE_SIGNING_IDENTITY:-}" --arg hook "$hook" --argjson updater "$updater" '
  {version: $version, bundle: {createUpdaterArtifacts: $updater}}
  | if $platform == "macos" then
      .bundle.macOS = (if $mode == "release" then
        {signingIdentity: $identity, hardenedRuntime: true, entitlements: "Entitlements.plist"}
        else {signingIdentity: "-", hardenedRuntime: false} end)
    else
      .bundle.windows = {signCommand: {cmd: "pwsh", args:
        ["-NoProfile", "-NonInteractive", "-File", $hook, "-Path", "%1"]}}
    end'

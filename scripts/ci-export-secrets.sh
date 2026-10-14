#!/usr/bin/env bash
# Passes the named environment variables on to the later steps of a GitHub Actions job, but
# only those that are non-empty. A missing repository secret arrives as an empty variable,
# and Tauri's bundler takes any APPLE_CERTIFICATE (even "") as a certificate to import.
#
#   scripts/ci-export-secrets.sh NAME [NAME...]
set -euo pipefail
for name in "$@"; do
  value="${!name:-}"
  if [ -n "$value" ]; then
    # The heredoc form carries multi-line values (the updater key is two lines).
    delimiter="GATIT_$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
    printf '%s<<%s\n%s\n%s\n' "$name" "$delimiter" "$value" "$delimiter" >> "$GITHUB_ENV"
    echo "$name: set"
  else
    echo "$name: not set"
  fi
done

# Signed releases

Public downloads must pass all three platform checks before the `latest` prerelease is replaced. Missing credentials, rejected notarization, an unstapled app, an untrusted/wrong Windows publisher, missing timestamps, or changed artifact bytes stop publication. Pull requests still build unsigned **CI-only** artifacts without signing credentials; they cannot publish.

## Account-owner setup required

No private keys or passwords belong in source control, chat, screenshots, or logs. An account owner must approve enrollment, payment, identity verification, new credentials/access grants, and uploading persistent secrets. The workflow only references configuration; it does not create accounts or credentials.

### macOS

Use an active [Apple Developer Program](https://developer.apple.com/programs/enroll/) membership and a **Developer ID Application** certificate with its private key (not Apple Development or Mac App Distribution). Apple lists USD 99 per membership year; local pricing and eligibility for fee waivers vary. The account holder handles enrollment and agreements.

Set these **repository Actions secrets** after approval:

| Secret | Required value |
| --- | --- |
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` containing the Developer ID Application certificate and private key |
| `APPLE_CERTIFICATE_PASSWORD` | Nonempty password protecting that `.p12` |
| `APPLE_ID` | Apple account used for notarization |
| `APPLE_PASSWORD` | App-specific password for that account |

Set these **repository Actions variables**:

| Variable | Required value |
| --- | --- |
| `APPLE_TEAM_ID` | Certificate team's 10-character Team ID |
| `APPLE_SIGNING_IDENTITY` | Full `Developer ID Application: Legal Name (TEAMID)` identity |

[Tauri's bundler](https://v2.tauri.app/distribute/sign/macos/) imports the certificate in the ephemeral runner, signs the app and sidecars under hardened runtime with `Entitlements.plist`, submits to Apple's notary service, waits for acceptance, and staples the ticket before generating updater archives. Do not enable `--skip-stapling` or `--no-sign` for releases.

`verify-macos.sh` checks every native executable, the expected team, Developer ID authority, hardened runtime, timestamp, stapled ticket, and Gatekeeper assessment. The collector repeats verification on a fresh extraction of the user ZIP and, if present, the updater archive. Nothing launches the app. Local development can continue using the ad-hoc platform configuration.

### Windows: Microsoft Artifact Signing

The integration uses Microsoft's [Artifact Signing](https://learn.microsoft.com/en-us/azure/artifact-signing/overview) service (formerly Trusted Signing), a **Public Trust** certificate profile, and the official `ArtifactSigning` PowerShell module pinned to 0.1.20. Private Trust and test profiles are unsuitable for public downloads.

The account owner must provide an Azure subscription, Entra tenant, an Artifact Signing account with completed identity validation, a Public Trust profile, and a signing identity. Confirm [current geographic eligibility](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart) before creating a paid resource: currently individuals must be in the US or Canada; organizations have a broader listed set of countries, including Israel. An individual's residence or an organization's eligibility must not be inferred from this repository.

The owner configures GitHub OIDC federation, scoped exactly to:

- Issuer: `https://token.actions.githubusercontent.com`
- Audience: `api://AzureADTokenExchange`
- Subject: `repo:SlikkDev/gatit:ref:refs/heads/main`
- RBAC: **Artifact Signing Certificate Profile Signer**, scoped to the chosen profile, not subscription-wide administration.

This is a persistent access grant requiring owner approval. The workflow requests a short-lived token via `azure/login`; no Azure client secret is stored. Do not grant the PR subject or a wildcard subject access. Restrict changes to `main` and review workflow changes before merging.

Set these **repository Actions variables**:

| Variable | Required value |
| --- | --- |
| `AZURE_CLIENT_ID` | Federated signing identity's client ID |
| `AZURE_TENANT_ID` | Entra tenant ID |
| `AZURE_SUBSCRIPTION_ID` | Subscription containing the signing account |
| `AZURE_SIGNING_ENDPOINT` | Regional HTTPS signing endpoint from the account |
| `AZURE_SIGNING_ACCOUNT` | Artifact Signing account name |
| `AZURE_CERTIFICATE_PROFILE` | Public Trust certificate profile name |
| `WINDOWS_PUBLISHER_SUBJECT` | Exact certificate subject string for the approved publisher, as returned by Windows `SignerCertificate.Subject` |

The build signs and verifies all three native sidecars before packaging. Tauri's `signCommand` signs the main app, NSIS uninstaller and outer installer; the hook verifies each result. Signing uses SHA-256 and Microsoft's RFC 3161 timestamp endpoint. The final collected installer is checked again before recording its hash. Authenticode precedes Tauri's updater signature, so the updater authenticates the final signed bytes.

[Microsoft's integration documentation](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations) describes the service/module requirements. Its [official price page](https://azure.microsoft.com/en-us/pricing/details/artifact-signing/) lists Basic with 5,000 signatures/month and Premium with 100,000. The page's dollar amounts were not available in the retrieved pricing table on 2026-10-14; confirm an actual regional quote before approving payment. Billing begins when the signing account is created, and the page states that the monthly charge is not prorated.

If the publisher is ineligible, use an existing eligible CA-issued code-signing service or hardware-backed certificate and adapt `sign-windows.ps1` to that provider. Do not substitute a self-signed certificate or a new exportable software key for public Windows signing.

A valid signature identifies the publisher; it cannot guarantee every SmartScreen reputation or organization-policy decision. Investigate a warning and verify the release/publisher rather than bypassing the protection.

## Optional in-app update signatures

These are separate from OS code signing. `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` are optional repository secrets. The private key must match the existing public key in `app/src-tauri/tauri.conf.json`. Recover the original key through its owner; do not silently rotate it. Previously installed apps cannot verify updates signed with a different key.

Without the updater key, the release contains verified manual downloads and no update feed. With it, all three platforms must provide updater signatures or publication fails. `latest.json` continues to use the literal `latest` tag, because this is a prerelease and GitHub's `/releases/latest` redirect does not select it.

## Validation and first signed release

1. Run `python3 -m unittest discover -s scripts/tests -v`. Windows CI also runs `scripts/tests/verify-windows.tests.ps1` against simulated signature failures.
2. Review the workflow and merge after account-owner setup. A push to `main`, or a manual **Build** dispatch on `main`, attempts a signed release. Missing configuration fails early and preserves the existing release.
3. Require successful Intel, Apple Silicon, Windows, and test jobs. `*.verification.json` records expected publisher, source commit and artifact SHA-256 hashes; the release gate checks all three receipts and rejects changed or extra files.
4. Download the published files afresh and match their SHA-256 hashes to the receipts. On macOS, extract the ZIP then run `codesign --verify --deep --strict`, `xcrun stapler validate`, and `spctl --assess --type execute`. On Windows, inspect `Get-AuthenticodeSignature` for a valid expected publisher and timestamp. These checks do not execute installers.
5. Verify all live site links still resolve to the three matching assets. A signed release is complete only after these platform and download checks; adding CI configuration alone is not completion.

The 0.2.9 release inspected on 2026-10-14 had ad-hoc signatures on both Mac builds and no embedded Authenticode signature on the Windows installer. GitHub's repository, environment and organization secret/variable lists available to Gatit were empty. Existing Apple/Azure accounts outside GitHub have not been established by that observation.

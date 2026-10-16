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

### Windows: free distribution paths

The selected plan is **SignPath Foundation for direct downloads plus Microsoft Store MSIX**. Both require acceptance/verification before distribution. No paid subscription is authorized. The older Azure adapter remains available only under its explicit provider value; nothing in this change enrolls or activates it.

[SignPath Foundation](https://signpath.org/terms) offers free signing to accepted open-source projects. Its publisher certificate, reputation review, MFA, manual approvals and source-verifiable builds are requirements. Gatit is a new project; acceptance is not guaranteed. See the proposed [code signing policy](https://slikkdev.github.io/gatit/code-signing.html) and [privacy policy](https://slikkdev.github.io/gatit/privacy.html). Do not state that Gatit already receives Foundation signing until acceptance and a verified release.

After acceptance and owner approval, configure these repository Actions references:

| Kind | Name | Value |
| --- | --- | --- |
| Variable | `WINDOWS_SIGNING_PROVIDER` | `signpath-foundation` |
| Secret | `SIGNPATH_API_TOKEN` | Owner-entered token restricted to the accepted signing policy |
| Variable | `SIGNPATH_ORGANIZATION_ID` | Organization ID issued by SignPath |
| Variable | `SIGNPATH_PROJECT_SLUG` | Approved Gatit project slug |
| Variable | `SIGNPATH_SIGNING_POLICY_SLUG` | Release policy requiring a human approver |
| Variable | `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | Configuration created from `packaging/windows/signpath-artifact.xml` |
| Variable | `WINDOWS_PUBLISHER_SUBJECT` | Exact accepted certificate subject returned by Windows; do not guess it |
| Variable | `SIGNPATH_NSIS_UPSTREAM_APPROVED` | `true` only after explicit approval of the five named, pinned upstream DLL exceptions below |

The owner must approve any SignPath/GitHub access grant and enter credentials directly. Never paste tokens into chat. Configure trusted GitHub builds for this repository, GitHub-hosted runners and `main`; configure the Foundation-approved certificate and approver. Confirm with SignPath that the nested official-action integration and artifact policy are accepted before relying on it for production.

The Tauri signing hook runs the **pinned official GitHub upload action and SignPath action** inside a Node GitHub action so the uploader receives the runner's ephemeral artifact credentials. Each first-party PE is uploaded with GitHub-origin evidence and restricted to `ProductName=Gatit` and the current `ProductVersion`. It waits up to one hour for each approval. Four requests are expected: recorder, app, NSIS uninstaller, installer. Signing must not change executable content outside the Authenticode table/checksum. Windows then verifies trust, exact publisher, code-signing EKU and timestamp. Retained signed bytes and static installer extraction are checked before the existing release hash gate. No installer is launched by these checks.

**Upstream exceptions:** `whisper-cli.exe` and `whisper-quantize.exe` are built afresh from the exact whisper.cpp commit in `packaging/windows/upstream.json`. Their license and a source/commit/SHA-256 receipt are included in the package. Both hashes must survive packaging unchanged. They are not sent to SignPath as Gatit's code.

Tauri 2.12.3 also asks its hook to sign five upstream NSIS DLLs: `NSISdl.dll`, `StartMenu.dll`, `System.dll`, `nsDialogs.dll`, and `nsis_tauri_utils.dll`. They may only be preserved unsigned after separate owner approval, and only when every byte matches `packaging/windows/nsis-upstream.json`. An unknown or modified DLL fails. Approval of Whisper does not automatically approve these helpers. Gatit's app, recorder, installer and uninstaller still require signatures.

### Microsoft Store MSIX

[Microsoft's current onboarding flow](https://learn.microsoft.com/en-us/windows/apps/publish/partner-center/open-a-developer-account) has no registration fee for either Individual or Company accounts when started at [storedeveloper.microsoft.com](https://storedeveloper.microsoft.com/). Company registration requires business and contact verification and agreement acceptance. The account owner completes those steps; this repository does not create an account or accept terms.

`Microsoft Store package` builds a separate edition with `--features microsoft-store`. The direct-download updater module and plugin initialization are excluded. Store builds cannot download or launch the NSIS updater. The package includes the recorder and the same pinned Whisper tools. Windows SDK MakeAppx validates it, unpacks it and compares every payload hash without installing it.

PR/main CI produces `CI-preview-MSIX` with a deliberately nonproduction identity. **Do not distribute or submit that preview.** After reserving Gatit in Partner Center, copy the exact identity values to these non-secret repository variables:

- `STORE_IDENTITY_NAME`: Package/Identity/Name.
- `STORE_PUBLISHER_ID`: Package/Identity/Publisher, including the full `CN=...` string.
- `STORE_PUBLISHER_DISPLAY_NAME`: publisher display name shown by Partner Center.

Dispatch `Microsoft Store package` on `main` with `submission=true` to obtain `Store-submission-MSIX`. It remains unsigned; [Microsoft Store signs MSIX packages during its publication process](https://learn.microsoft.com/en-us/windows/msix/package/signing-package-overview). It is not a signed GitHub download. Complete the Store listing, privacy URL, age ratings, capability explanations and certification in Partner Center before making a Store link public. No Store submission is automated here.

Store package versions map app `major.minor.patch` to `(major+1).minor.patch.0` because Store requires a nonzero first field and reserves the fourth. For example, `0.2.17` becomes `1.2.17.0`. The Store workflow has its own monotonically increasing run number; it is separate from the direct-download workflow. Before the first submission, confirm its package version exceeds any prior Partner Center package version. Keep it below the 16-bit field limits.

The MSIX currently targets x64 Windows 10 version 2004 or newer. It requires WebView2. A native startup check opens Microsoft's official runtime download page only if the user chooses to, then exits; it does not launch an installer. App Installer can also interpret the manifest's external dependency; Store deployment may ignore that declaration. Windows App Certification Kit and clean-machine launch/recording/privacy checks remain prerequisites for the first Store submission. Packaging CI does not claim runtime certification.

The MSIX and NSIS editions have distinct installation/update ownership. Back up the meetings folder before switching editions; this PR does not implement a data migration or guarantee that Windows package virtualization exposes the same library. Remove the old edition through Windows Settings after preserving the data. Uninstall the Store edition through Windows Settings/Microsoft Store.

### Existing paid adapter (inactive)

`azure-artifact-signing` remains explicit opt-in for an already authorized service. It requires `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERTIFICATE_PROFILE` and `WINDOWS_PUBLISHER_SUBJECT` repository variables. Its OIDC subject is `repo:SlikkDev/gatit:ref:refs/heads/main`, with only the profile signer role. The workflow uses the official ArtifactSigning 0.1.20 module. No paid Azure setup is part of the selected plan.

A valid signature identifies the publisher; it does not guarantee every SmartScreen reputation or organization-policy decision. Investigate the warning and release identity without bypassing the protection.

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

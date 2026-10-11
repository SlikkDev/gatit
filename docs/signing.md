# Signing, notarization and automatic updates

Gatit's builds come out of GitHub Actions. Two things make them behave like a normal Mac app, and both turn on by themselves once the matching secrets exist in the repository (**Settings → Secrets and variables → Actions → New repository secret**). Until then the workflow falls back to today's ad-hoc build.

| Secret | What it is |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | The updater's private key (see below). With it, every build ships an update package Gatit installs by itself. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | The password that key was generated with. |
| `APPLE_CERTIFICATE` | Your "Developer ID Application" certificate with its private key, as a base64 `.p12`. With it, the app is signed under the hardened runtime. |
| `APPLE_CERTIFICATE_PASSWORD` | The password you gave the `.p12` when exporting it. |
| `APPLE_ID` | The Apple ID email of the developer account. With the next two, Apple notarizes every build and macOS opens it without any warning. |
| `APPLE_PASSWORD` | An app-specific password for that Apple ID (not your real password). |
| `APPLE_TEAM_ID` | The 10-character Team ID shown on the developer account's Membership page. |

The repository is public, so nothing here goes into a file in the repository: only into Actions secrets.

## Automatic updates

Gatit checks the [latest release](https://github.com/SlikkDev/gatit/releases/latest) half a minute after it starts and every six hours after that. When a newer build is there, it downloads it, checks its signature against the public key built into the app, replaces `Gatit.app` on disk and shows a **Restart** button in the window and a **Restart to update** item in the menu bar. It never restarts on its own, so a recording is never cut off. Settings shows the installed version at the bottom.

The release only carries an update package (`Gatit-macos-*.app.tar.gz`, its `.sig` and `latest.json`) when the build can sign it. The key pair was generated with Tauri's signer; the public half is `plugins.updater.pubkey` in `app/src-tauri/tauri.conf.json`, the private half belongs in the two `TAURI_SIGNING_PRIVATE_KEY*` secrets. To start over with a new pair:

```sh
cargo install tauri-cli --version "^2" --locked
cargo tauri signer generate -w ~/.tauri/gatit.key
```

It prints the public key (replace `pubkey` in `tauri.conf.json` with it) and writes the private key to the file (its contents become `TAURI_SIGNING_PRIVATE_KEY`, the password you typed becomes `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). An app built with the old public key cannot verify updates signed with the new one, so after a key change download the next build by hand once.

Updates are not signed by Apple unless the Apple secrets exist too; an ad-hoc-signed update replaces an ad-hoc-signed app just fine, since macOS only checks the signature on the first launch of a downloaded app.

## Apple Developer ID and notarization

This needs the [Apple Developer Program](https://developer.apple.com/programs/enroll/) (99 USD a year). Enrollment as an individual takes a day or two to be approved. Then, on your Mac:

1. **Team ID**: on [developer.apple.com/account](https://developer.apple.com/account), the Membership details show a 10-character Team ID. That is `APPLE_TEAM_ID`.
2. **Certificate**: open Keychain Access → menu Keychain Access → Certificate Assistant → **Request a Certificate From a Certificate Authority**. Enter your email and name, choose **Saved to disk**, and save the `.certSigningRequest` file. Then on [Certificates, Identifiers & Profiles → Certificates](https://developer.apple.com/account/resources/certificates/add) click **+**, choose **Developer ID Application** (under Software, not "Mac App Distribution"), pick the latest profile type, upload the request file and download the `.cer`. Double-click the `.cer` to add it to your login keychain.
3. **Export it**: in Keychain Access, under **My Certificates**, find "Developer ID Application: Your Name (TEAMID)", right-click → **Export**, choose the `.p12` format and give it a password. That password is `APPLE_CERTIFICATE_PASSWORD`. Turn the file into one line of text and copy it:

   ```sh
   base64 -i ~/Downloads/Certificates.p12 | pbcopy
   ```

   Paste it as `APPLE_CERTIFICATE`, then delete the `.p12` (the certificate stays in your keychain).
4. **App-specific password**: on [account.apple.com](https://account.apple.com/account/manage), under Sign-In and Security → **App-Specific Passwords**, create one named "Gatit notarization". That is `APPLE_PASSWORD`; your Apple ID email is `APPLE_ID`.

The next push to `main` (or re-running the latest workflow from the Actions tab) then publishes a signed and notarized build. The release notes say which kind of build it is, and the build log's "Choose signing" step says what it found. Notarization adds a few minutes to each build.

What changes in the app: Developer ID builds run under Apple's hardened runtime, so `app/src-tauri/Entitlements.plist` grants the microphone entitlement to the app and its helper programs (the Swift recorder and whisper.cpp). Everything else (system audio, the call window, Keychain, the Claude and Google APIs) needs no entitlement outside the App Store sandbox. Ad-hoc builds keep the hardened runtime off, as before.

The certificate expires after five years; notarized apps keep working after that, but new builds need a new certificate. Signing fails loudly in CI when it does.

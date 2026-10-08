# Gatit

A personal Mac (then Windows) app that records, transcribes and summarizes Zoom, Google Meet and WhatsApp calls **without adding a bot to the call**. It captures your microphone and your computer's audio directly, the same way Timeless and Granola do.

## Plan

1. **Mac recorder** *(this step)*: menu-bar app, one click records your mic ("Me") and system audio ("Them") as two separate tracks.
2. **Transcription**: local Whisper tuned for Hebrew (with English support), merged into a timestamped Me/Them transcript.
3. **Summaries and library**: Claude summaries with action items, a meeting list and search.
4. **Windows**: the same app, recording through WASAPI loopback.

## Privacy

This repository is public. Recordings, transcripts and API keys never live in it:

- Recordings are saved to `~/Library/Application Support/dev.slikk.gatit/recordings/<date time>/` as `mic.m4a` and `system.m4a`.
- API keys (from step 3) are stored in the macOS Keychain.
- `.gitignore` blocks audio files and `.env` files as a safety net.

Some places require telling the other people on a call that you are recording.

## Running it on a Mac

Requirements: macOS 14.2 or later, Xcode command-line tools, Rust, and the Tauri CLI (`cargo install tauri-cli --version "^2"`).

```sh
scripts/build-recorder-mac.sh      # builds the Swift recorder into the app
cd app/src-tauri
cargo tauri dev                    # run in development
cargo tauri build --bundles app    # or build Gatit.app
```

On the first recording macOS asks for two permissions: **Microphone** and **System Audio Recording**. In development the prompts name your terminal app instead of Gatit.

Each push also builds `Gatit.app` on GitHub Actions; download it from the run's **Gatit-macos** artifact. The build is unsigned, so the first launch needs right-click → Open.

## Layout

- `recorder-mac/`: Swift command-line recorder. It uses a Core Audio process tap for system audio (no screen-recording permission and no purple indicator) and AVAudioEngine for the mic, and talks to the app over JSON lines on stdout.
- `app/`: Tauri 2 app with the menu-bar item and recording window.

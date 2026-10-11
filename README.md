# Gatit

A personal Mac (then Windows) app that records, transcribes and summarizes Zoom, Google Meet and WhatsApp calls **without adding a bot to the call**. It captures your microphone and your computer's audio directly, the same way Timeless and Granola do.

## What it does

1. **Record** with one click, from the menu bar or with a keyboard shortcut (Control-Option-Command-R by default). Your microphone is saved as "Me" and everything the Mac plays as "Them", in two files, so the transcript knows who said what with no speaker-detection AI.
2. **Transcribe** on your Mac with whisper.cpp and [ivrit.ai](https://huggingface.co/ivrit-ai)'s Hebrew fine-tune of Whisper. Hebrew calls with English terms mixed in (CRO, ROI, AI, profit share...) keep those terms in English letters, using a vocabulary list you can edit in Settings. English-only calls use the stock English model. Audio never leaves the machine.
3. **Summarize** with Claude: a title, overview, key points, decisions, action items (yours or theirs) and open questions, always in English. Only the transcript text is sent. Your API key lives in the macOS Keychain.
4. **Library**: all calls in one list, grouped by day, with search across every transcript and summary. Hebrew search finds words with or without attached prefix letters (ה, ו, ב, ש...). Click a transcript line to hear that moment. Export any call as Markdown.

Windows (step 4 of the plan) comes next: the same app, recording through WASAPI loopback.

## Requirements

- macOS 14.2 or later, Intel or Apple Silicon. (The system audio capture uses a Core Audio process tap, which arrived in 14.2.)
- About 2 GB of disk for the speech models, downloaded on first use.
- A Claude API key for summaries (optional; everything else works without one).

On an Intel Mac, transcription runs on the CPU and takes roughly as long as the call itself. Gatit defaults to a compressed model there, and Settings can defer transcription until the Mac is plugged in.

## Privacy

This repository is public. Recordings, transcripts and API keys never live in it:

- Each call is a folder under `~/Library/Application Support/dev.slikk.gatit/meetings/` with `mic.m4a`, `system.m4a`, `transcript.json`, `summary.json` and `meeting.md`. Plain files, so syncing the library between machines later only needs to copy folders.
- The search index (`index.sqlite`) is a cache rebuilt from those folders on every launch.
- The Claude API key is stored in the macOS Keychain and read from nowhere else.
- `.gitignore` blocks audio, models, databases and `.env` files as a safety net.

Some places require telling the other people on a call that you are recording.

## Getting the app

Every push builds `Gatit.app` on GitHub Actions. Download the **Gatit-macos-intel** (2019 MacBook Pro) or **Gatit-macos-apple-silicon** artifact from the run, unzip it, and right-click → Open the first time, because the build is unsigned.

On the first recording macOS asks for two permissions: **Microphone** and **System Audio Recording**.

## Building it yourself

Requirements: Xcode command-line tools, CMake, Rust, and the Tauri CLI (`cargo install tauri-cli --version "^2"`).

```sh
scripts/build-recorder-mac.sh      # builds the Swift recorder into the app
scripts/build-whisper-mac.sh       # builds whisper.cpp's whisper-cli and whisper-quantize
cd app/src-tauri
cargo test                         # transcript merge, Hebrew search, summary parsing
cargo tauri dev                    # run in development
cargo tauri build --bundles app    # or build Gatit.app
```

In development the permission prompts name your terminal app instead of Gatit.

## Layout

- `recorder-mac/`: Swift command-line recorder. It uses a Core Audio process tap for system audio (no screen-recording permission and no purple indicator) and AVAudioEngine for the mic, and talks to the app over JSON lines on stdout. It finishes both files cleanly on stop, quit, or when the Mac goes to sleep.
- `app/src-tauri/`: the Tauri 2 app.
  - `recorder.rs` starts and stops the recorder sidecar and files each recording as a meeting.
  - `transcribe.rs` runs `whisper-cli` on both tracks (with silence detection) and `transcript.rs` merges them into one Me/Them transcript, drops the other side's voice leaking into your mic, and rewrites Hebrew-letter spellings of known terms.
  - `summary.rs` calls the Claude API with a structured-output schema. The instructions are in `prompts/summary.md`; "Edit the summary instructions" in Settings gives you an editable copy.
  - `library.rs` is the meetings folder plus the SQLite full-text index (trigram tokenizer, for Hebrew prefixes).
  - `jobs.rs` runs transcription and summaries one at a time in the background and resumes unfinished work on launch.
  - `models.json` lists the model downloads.
- `app/ui/`: the window, plain HTML and JavaScript.
- `scripts/`: builds for the Swift recorder and whisper.cpp.

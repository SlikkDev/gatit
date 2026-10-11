# Gatit

A personal Mac (then Windows) app that records, transcribes and summarizes Zoom, Google Meet and WhatsApp calls **without adding a bot to the call**. It captures your microphone and your computer's audio directly, the same way Timeless and Granola do.

## What it does

1. **Record** when a call starts: Gatit notices Zoom, WhatsApp or a browser (Google Meet) starting to use the microphone and shows a small "Record this call?" pop-up in the top-right corner. Or record with one click, from the menu bar or with a keyboard shortcut (Control-Option-Command-R by default). Your microphone is saved as "Me" and everything the Mac plays as "Them", in two files, so the transcript knows who said what with no speaker-detection AI.
   Set your name in Settings and it replaces "Me". The other people on a call are listed on the call (type them in, or let Gatit fill them in: from the call app's window when it starts, and from what Claude hears in the conversation). With one other person on the call, their lines carry their name; on a group call the other side stays "Them", since everyone else shares one track.
2. **Transcribe** on your Mac with whisper.cpp and [ivrit.ai](https://huggingface.co/ivrit-ai)'s Hebrew fine-tune of Whisper. Hebrew calls with English terms mixed in keep those terms in English letters: a built-in list of about 200 business, marketing and tech terms Israelis say in English (CRO, ROI, KPI, profit share, landing page, LinkedIn...), plus your own terms in Settings. Acronyms are recognized when spelled out letter by letter ("קיי פי איי" becomes KPI). Everyday loanwords Israelis write in Hebrew (מייל, פידבק, דדליין) stay in Hebrew. English-only calls use the stock English model. Audio never leaves the machine.
3. **Summarize** with Claude: a title, overview, who was on the call, key points, decisions, action items (yours, or by the name of whoever took them) and open questions, always in English. Only the transcript text is sent, with the names it already knows. Your API key lives in the macOS Keychain.
4. **Library**: all calls in one list, grouped by day, with search across every transcript and summary. Hebrew search finds words with or without attached prefix letters (ה, ו, ב, ש...). Click a transcript line to hear that moment. Export any call as Markdown.
5. **Google Drive**: every call is uploaded to your Drive as `Gatit / date / time and title`, with the recording, transcript and summary, as soon as it is recorded and again when it changes. Set up once with your own Google Cloud OAuth client: see [docs/google-drive.md](docs/google-drive.md).

Windows (step 4 of the plan) comes next: the same app, recording through WASAPI loopback.

## Requirements

- macOS 14.2 or later, Intel or Apple Silicon. (The system audio capture uses a Core Audio process tap, which arrived in 14.2.)
- About 2 GB of disk for the speech models, downloaded on first use.
- A Claude API key for summaries (optional; everything else works without one).
- A Google Cloud OAuth client for Drive uploads (optional; [five-minute setup](docs/google-drive.md)).

On an Intel Mac, transcription runs on the CPU and takes roughly as long as the call itself. Gatit defaults to a compressed model there, and Settings can defer transcription until the Mac is plugged in.

## Privacy

This repository is public. Recordings, transcripts and API keys never live in it:

- Each call is a folder under `~/Library/Application Support/dev.slikk.gatit/meetings/` with `mic.m4a`, `system.m4a`, `video.mp4` (when the call had a window to record), `transcript.json`, `summary.json` and `meeting.md`. Plain files, so syncing the library between machines later only needs to copy folders.
- The search index (`index.sqlite`) is a cache rebuilt from those folders on every launch.
- The Claude API key and the Google Drive sign-in (OAuth client and refresh token) are stored in the macOS Keychain and read from nowhere else. Drive access is limited to files Gatit created (`drive.file` scope).
- `.gitignore` blocks audio, models, databases and `.env` files as a safety net.

Some places require telling the other people on a call that you are recording.

## Getting the app

The short version is at [slikkdev.github.io/gatit](https://slikkdev.github.io/gatit/). Every push to `main` publishes a build on the [latest release](https://github.com/SlikkDev/gatit/releases/latest) page. Download the one for your Mac (Apple menu → About This Mac shows the chip):

- **Gatit-macos-apple-silicon.zip** for a Mac with an Apple chip (M1 or later).
- **Gatit-macos-intel.zip** for an Intel Mac, such as a 2019 MacBook Pro.

Unzip it and drag `Gatit.app` into Applications. The app carries only an ad-hoc signature (no Apple developer certificate, no notarization), so macOS blocks the first launch:

- **macOS 15 (Sequoia):** double-click Gatit and click **Done** on the warning. Open System Settings → Privacy & Security, scroll down to the line about Gatit and click **Open Anyway**, then **Open** on the next warning.
- **macOS 14 (Sonoma):** Control-click (right-click) Gatit → **Open** → **Open**.
- If macOS says Gatit is "damaged and can't be opened", the download got quarantined before it was signed. Run `xattr -cr /Applications/Gatit.app` in Terminal and open it again.

macOS remembers the choice, so later launches are a normal double-click. On the first recording macOS asks for two permissions: **Microphone** and **System Audio Recording**. With video on (Settings > Recording, on by default) it also asks for **Screen & System Audio Recording**; until that's allowed, calls are recorded as audio only, and allowing it takes effect from the next call.

Builds for pull requests are workflow artifacts on the Actions run instead. GitHub wraps those in a second zip, so unzip twice.

Reading the call app's window title (to tell a Zoom call from a Meet call, and to pick up the host's name from "Dana's Zoom Meeting" or a WhatsApp contact from the call window) works only for apps with **Screen Recording** permission. Gatit doesn't ask for it; grant it in System Settings > Privacy & Security if you want those filled in automatically. Without it, names still come from Claude's reading of the call and from what you type.

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

- `recorder-mac/`: Swift command-line recorder. It uses a Core Audio process tap for system audio (no screen-recording permission and no purple indicator) and AVAudioEngine for the mic, and talks to the app over JSON lines on stdout. It finishes both files cleanly on stop, quit, or when the Mac goes to sleep. On start it also reports the call apps' window titles (`CallWindows.swift`). With `--video` it also records the call's window (the Zoom meeting, the browser window showing Meet, or WhatsApp) with ScreenCaptureKit, picking the window up again if it closes and reopens; after the call, `--mux` copies that video and both audio tracks, mixed, into one `video.mp4`. Run as `gatit-recorder --watch-calls`, it instead reports which call apps are using the microphone (`CallWatcher.swift`), which needs no permission.
- `site/`: the one-page website, published to GitHub Pages by `.github/workflows/pages.yml` on every push to `main` that touches it.
- `app/src-tauri/`: the Tauri 2 app.
  - `calls.rs` runs the call watcher and shows the "Record this call?" pop-up (`ui/prompt.html`).
  - `recorder.rs` starts and stops the recorder sidecar and files each recording as a meeting. `hints.rs` turns the window titles it reports into the call app and the people on the call.
  - `transcribe.rs` runs `whisper-cli` on both tracks (with silence detection) and `transcript.rs` merges them into one Me/Them transcript, drops the other side's voice leaking into your mic and labels the tracks with people's names when they're known.
  - `vocabulary.rs` keeps English terms in English letters: it builds Whisper's prompt and rewrites Hebrew-letter spellings ("סי אר או") into the term ("CRO"). The built-in terms are in `vocabulary/hebrew-english.json`. After editing that list, check it doesn't catch ordinary Hebrew words by running the ignored `vocabulary_collisions` test with a Hebrew word-frequency list, such as [FrequencyWords](https://github.com/hermitdave/FrequencyWords)' `he_50k.txt`: `GATIT_WORDLIST=he_50k.txt cargo test vocabulary_collisions -- --ignored --nocapture`.
  - `summary.rs` calls the Claude API with a structured-output schema. The instructions are in `prompts/summary.md`; "Edit the summary instructions" in Settings gives you an editable copy.
  - `library.rs` is the meetings folder plus the SQLite full-text index (trigram tokenizer, for Hebrew prefixes).
  - `jobs.rs` runs transcription and summaries one at a time in the background and resumes unfinished work on launch.
  - `drive.rs` signs in to Google (OAuth with PKCE, loopback redirect) and mirrors each meeting folder to Drive, one meeting at a time, retrying failures.
  - `models.json` lists the model downloads.
- `app/ui/`: the window, plain HTML and JavaScript.
- `scripts/`: builds for the Swift recorder and whisper.cpp.

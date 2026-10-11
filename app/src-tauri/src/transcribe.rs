//! Local transcription with whisper.cpp. Audio never leaves the machine.
//!
//! whisper.cpp ships with the app as two sidecar binaries built in CI: `whisper-cli`
//! (transcription) and `whisper-quantize` (makes the compressed Hebrew model on first use).
//! Models are downloaded once into the app's data folder, never into the repo.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use futures_util::StreamExt;
use regex::Regex;
use serde::Deserialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;
use tokio::io::AsyncWriteExt;

use crate::settings::Settings;
use crate::transcript::{self, Speaker, Transcript, VocabFixer};

#[derive(Clone, Debug, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub file: String,
    pub url: String,
    pub size_mb: u64,
}

#[derive(Debug, Deserialize)]
struct Catalog {
    hebrew: ModelInfo,
    english: ModelInfo,
    vad: ModelInfo,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../models.json")).expect("models.json is valid")
    })
}

/// Which model a transcription needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Hebrew,
    HebrewCompressed,
    English,
    Vad,
}

impl Model {
    fn info(self) -> &'static ModelInfo {
        match self {
            Model::Hebrew | Model::HebrewCompressed => &catalog().hebrew,
            Model::English => &catalog().english,
            Model::Vad => &catalog().vad,
        }
    }

    pub fn file_name(self) -> String {
        match self {
            Model::HebrewCompressed => catalog().hebrew.file.replace(".bin", "-q5_0.bin"),
            _ => self.info().file.clone(),
        }
    }

    pub fn label(self) -> String {
        match self {
            Model::HebrewCompressed => format!("{}, compressed", catalog().hebrew.name),
            _ => self.info().name.clone(),
        }
    }

    pub fn hebrew(settings: &Settings) -> Model {
        if settings.hebrew_model == "full" {
            Model::Hebrew
        } else {
            Model::HebrewCompressed
        }
    }
}

pub fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn model_path(app: &AppHandle, model: Model) -> Result<PathBuf, String> {
    Ok(models_dir(app)?.join(model.file_name()))
}

pub fn is_installed(app: &AppHandle, model: Model) -> bool {
    model_path(app, model).map(|p| p.exists()).unwrap_or(false)
}

/// Progress callback: (what is happening, percent 0-100).
pub type Progress<'a> = &'a (dyn Fn(&str, f64) + Send + Sync);

/// Makes sure a model is on disk, downloading (and compressing) it if needed.
pub async fn ensure_model(
    app: &AppHandle,
    model: Model,
    progress: Progress<'_>,
) -> Result<PathBuf, String> {
    let path = model_path(app, model)?;
    if path.exists() {
        return Ok(path);
    }
    if model == Model::HebrewCompressed {
        let full = model_path(app, Model::Hebrew)?;
        let had_full = full.exists();
        let full = ensure_downloaded(app, Model::Hebrew, progress).await?;
        progress("Compressing the Hebrew model (one time)", 0.0);
        let tmp = path.with_extension("part");
        run_sidecar(
            app,
            "whisper-quantize",
            &[full.as_os_str(), tmp.as_os_str(), "q5_0".as_ref()],
            |_| {},
        )
        .await?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        if !had_full {
            // Only the compressed copy is used; don't keep 1.6 GB around for nothing.
            let _ = std::fs::remove_file(&full);
        }
        return Ok(path);
    }
    ensure_downloaded(app, model, progress).await
}

async fn ensure_downloaded(
    app: &AppHandle,
    model: Model,
    progress: Progress<'_>,
) -> Result<PathBuf, String> {
    let path = model_path(app, model)?;
    if !path.exists() {
        let info = model.info();
        let label = format!(
            "Downloading {} ({:.1} GB)",
            model.label(),
            info.size_mb as f64 / 1000.0
        );
        download(&info.url, &path, &label, progress).await?;
    }
    Ok(path)
}

async fn download(
    url: &str,
    path: &Path,
    label: &str,
    progress: Progress<'_>,
) -> Result<(), String> {
    let response = reqwest::get(url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Download failed: {e}"))?;
    let total = response.content_length().unwrap_or(0);
    let tmp = path.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut done: u64 = 0;
    let mut last_report = 0.0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
        if total > 0 {
            let pct = done as f64 * 100.0 / total as f64;
            if pct - last_report >= 1.0 {
                last_report = pct;
                progress(label, pct);
            }
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    if total > 0 && done != total {
        let _ = std::fs::remove_file(&tmp);
        return Err("Download was incomplete. It will retry next time.".into());
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Runs a bundled sidecar to completion, passing each stderr line to `on_line`.
async fn run_sidecar(
    app: &AppHandle,
    name: &str,
    args: &[&std::ffi::OsStr],
    mut on_line: impl FnMut(&str),
) -> Result<String, String> {
    let (mut events, _child) = app
        .shell()
        .sidecar(name)
        .map_err(|e| e.to_string())?
        .args(args)
        .spawn()
        .map_err(|e| format!("couldn't start {name}: {e}"))?;
    let mut stderr_tail: Vec<String> = Vec::new();
    while let Some(event) = events.recv().await {
        match event {
            CommandEvent::Stderr(line) | CommandEvent::Stdout(line) => {
                let line = String::from_utf8_lossy(&line).trim_end().to_string();
                on_line(&line);
                stderr_tail.push(line);
                if stderr_tail.len() > 20 {
                    stderr_tail.remove(0);
                }
            }
            CommandEvent::Terminated(status) => {
                return if status.code == Some(0) {
                    Ok(stderr_tail.join("\n"))
                } else {
                    Err(format!(
                        "{name} failed ({:?}): {}",
                        status.code,
                        stderr_tail.join(" | ")
                    ))
                };
            }
            CommandEvent::Error(e) => return Err(e),
            _ => {}
        }
    }
    Err(format!("{name} stopped unexpectedly"))
}

/// Converts an .m4a track to WAV, which whisper.cpp can read (it resamples and downmixes
/// to 16 kHz mono itself). Uses macOS's built-in afconvert, so no ffmpeg is needed.
#[cfg(target_os = "macos")]
async fn to_wav(input: &Path, output: &Path) -> Result<(), String> {
    let status = tokio::process::Command::new("/usr/bin/afconvert")
        .args(["-f", "WAVE", "-d", "LEI16@16000"])
        .arg(input)
        .arg(output)
        .status()
        .await
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("couldn't convert {}", input.display()));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn to_wav(input: &Path, _output: &Path) -> Result<(), String> {
    Err(format!(
        "audio conversion of {} is only implemented on macOS so far",
        input.display()
    ))
}

fn physical_cores() -> usize {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.physicalcpu"])
        .output()
    {
        if let Ok(n) = String::from_utf8_lossy(&out.stdout).trim().parse::<usize>() {
            return n.clamp(1, 8);
        }
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8)
}

fn progress_percent(line: &str) -> Option<f64> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"progress\s*=\s*(\d+)%").unwrap());
    re.captures(line)?.get(1)?.as_str().parse().ok()
}

struct TrackJob<'a> {
    wav: &'a Path,
    out_base: &'a Path,
    model: &'a Path,
    vad: &'a Path,
    language: &'a str,
    prompt: &'a str,
}

/// Transcribes one track and returns the JSON whisper-cli wrote.
async fn whisper(
    app: &AppHandle,
    job: TrackJob<'_>,
    on_percent: impl Fn(f64),
) -> Result<Vec<u8>, String> {
    let threads = physical_cores().to_string();
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "-m".as_ref(),
        job.model.as_os_str(),
        "-f".as_ref(),
        job.wav.as_os_str(),
        "-l".as_ref(),
        job.language.as_ref(),
        "-t".as_ref(),
        threads.as_ref(),
        "-oj".as_ref(),
        "-of".as_ref(),
        job.out_base.as_os_str(),
        "-pp".as_ref(),
        // Skip silence before decoding: faster, and stops Whisper inventing text in pauses.
        "--vad".as_ref(),
        "-vm".as_ref(),
        job.vad.as_os_str(),
        "--suppress-nst".as_ref(),
    ];
    if !job.prompt.is_empty() {
        args.push("--prompt".as_ref());
        args.push(job.prompt.as_ref());
    }
    run_sidecar(app, "whisper-cli", &args, |line| {
        if let Some(p) = progress_percent(line) {
            on_percent(p);
        }
    })
    .await?;
    let json_path = job.out_base.with_extension("json");
    std::fs::read(&json_path).map_err(|e| format!("whisper wrote no output: {e}"))
}

/// Transcribes a meeting folder's two tracks into one Me/Them transcript.
///
/// `language` is "he", "en" or "auto". Auto transcribes the other side with the Hebrew
/// model first; if Whisper hears English, both tracks are redone with the English model.
pub async fn transcribe_meeting(
    app: &AppHandle,
    folder: &Path,
    work_dir: &Path,
    language: &str,
    settings: &Settings,
    progress: Progress<'_>,
) -> Result<Transcript, String> {
    std::fs::create_dir_all(work_dir).map_err(|e| e.to_string())?;
    let tracks: Vec<(Speaker, PathBuf)> = [(Speaker::Them, "system.m4a"), (Speaker::Me, "mic.m4a")]
        .into_iter()
        .map(|(s, f)| (s, folder.join(f)))
        .filter(|(_, p)| p.exists())
        .collect();
    if tracks.is_empty() {
        return Err("This meeting has no audio files.".into());
    }

    let vad = ensure_model(app, Model::Vad, progress).await?;
    let mut model = if language == "en" {
        Model::English
    } else {
        Model::hebrew(settings)
    };
    let mut lang = if language == "auto" {
        "auto".to_string()
    } else {
        language.to_string()
    };

    progress("Preparing audio", 0.0);
    let mut wavs = Vec::new();
    for (speaker, m4a) in &tracks {
        let wav = work_dir.join(format!(
            "{}.wav",
            if *speaker == Speaker::Me {
                "mic"
            } else {
                "system"
            }
        ));
        to_wav(m4a, &wav).await?;
        wavs.push((*speaker, wav));
    }

    let prompt = transcript::whisper_prompt(&settings.vocabulary);
    let fixer = VocabFixer::new(&settings.vocabulary);
    let mut detected = String::new();
    let mut results: Vec<(Speaker, Vec<transcript::Segment>)> = Vec::new();
    let mut i = 0;
    while i < wavs.len() {
        let (speaker, wav) = &wavs[i];
        let model_path = ensure_model(app, model, progress).await?;
        let share = 100.0 / wavs.len() as f64;
        let base = share * i as f64;
        let who = if *speaker == Speaker::Me {
            "your side"
        } else {
            "the other side"
        };
        let label = format!("Transcribing {who}");
        progress(&label, base);
        let out_base = work_dir.join(format!(
            "{}-out",
            if *speaker == Speaker::Me {
                "mic"
            } else {
                "system"
            }
        ));
        let job = TrackJob {
            wav,
            out_base: &out_base,
            model: &model_path,
            vad: &vad,
            language: &lang,
            // The Hebrew prompt only helps Hebrew; leave English untouched.
            prompt: if model == Model::English { "" } else { &prompt },
        };
        let json = whisper(app, job, |p| progress(&label, base + p * share / 100.0)).await?;
        let (heard, segments) = transcript::parse_whisper_json(&json, *speaker)?;

        if lang == "auto" {
            // Decide once, from the first track, then use that language for both.
            lang = if heard == "en" {
                "en".into()
            } else {
                "he".into()
            };
            if lang == "en" && model != Model::English {
                model = Model::English;
                continue; // Redo this track with the English model.
            }
        }
        if detected.is_empty() {
            detected = if heard.is_empty() || heard == "auto" {
                lang.clone()
            } else {
                heard
            };
        }
        let segments = segments
            .into_iter()
            .map(|mut s| {
                s.text = fixer.apply(&s.text);
                s
            })
            .collect();
        results.push((*speaker, segments));
        i += 1;
    }

    let _ = std::fs::remove_dir_all(work_dir);
    let mut me = Vec::new();
    let mut them = Vec::new();
    for (speaker, segs) in results {
        match speaker {
            Speaker::Me => me = segs,
            Speaker::Them => them = segs,
        }
    }
    Ok(Transcript {
        language: detected,
        model: model.label(),
        segments: transcript::merge(me, them),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses() {
        assert!(catalog().hebrew.url.starts_with("https://huggingface.co/"));
        assert_eq!(
            Model::HebrewCompressed.file_name(),
            "ggml-ivrit-large-v3-turbo-q5_0.bin"
        );
    }

    #[test]
    fn reads_whisper_progress_lines() {
        assert_eq!(
            progress_percent("whisper_print_progress_callback: progress =  45%"),
            Some(45.0)
        );
        assert_eq!(progress_percent("whisper_init_from_file"), None);
    }
}

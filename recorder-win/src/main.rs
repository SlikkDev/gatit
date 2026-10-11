//! gatit-recorder --out <folder>
//!
//! The Windows counterpart of `recorder-mac`. Records the microphone to <folder>/mic.wav
//! and everything the computer plays to <folder>/system.wav (16 kHz mono, which is what
//! whisper.cpp wants) until it reads "stop" on stdin or stdin closes because the app went
//! away. Progress goes to stdout as one JSON object per line, the same events as the Mac
//! recorder: started, levels, stopped, error.

#[cfg(windows)]
mod audio;
mod dsp;

use std::io::Write;

use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Mic,
    System,
}

impl Track {
    pub fn file_name(self) -> &'static str {
        match self {
            Track::Mic => "mic.wav",
            Track::System => "system.wav",
        }
    }

    pub fn device_name(self) -> &'static str {
        match self {
            Track::Mic => "microphone",
            Track::System => "output",
        }
    }
}

/// Loudest level seen on each track since the last report.
#[derive(Default)]
pub struct Levels {
    mic: f32,
    system: f32,
}

impl Levels {
    pub fn update(&mut self, track: Track, rms: f32) {
        match track {
            Track::Mic => self.mic = self.mic.max(rms),
            Track::System => self.system = self.system.max(rms),
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn drain(&mut self) -> (f32, f32) {
        let out = (self.mic, self.system);
        *self = Levels::default();
        out
    }
}

fn emit(value: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn fail(message: &str) -> ! {
    emit(json!({ "event": "error", "message": message }));
    std::process::exit(1);
}

#[cfg(windows)]
enum Msg {
    Stop,
    Done(Track, Result<f64, String>),
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let folder = match args.iter().position(|a| a == "--out") {
        Some(i) if i + 1 < args.len() => std::path::PathBuf::from(&args[i + 1]),
        _ => fail("usage: gatit-recorder --out <folder>"),
    };
    if let Err(e) = std::fs::create_dir_all(&folder) {
        fail(&format!("couldn't create {}: {e}", folder.display()));
    }
    run(folder);
}

#[cfg(not(windows))]
fn run(_folder: std::path::PathBuf) {
    fail("this recorder only works on Windows");
}

#[cfg(windows)]
fn run(folder: std::path::PathBuf) {
    use std::io::BufRead;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    let stop = Arc::new(AtomicBool::new(false));
    let levels = Arc::new(Mutex::new(Levels::default()));
    let (tx, rx) = mpsc::channel::<Msg>();

    // One thread per track; each reports once it is actually capturing.
    let mut ready = Vec::new();
    for track in [Track::System, Track::Mic] {
        let (ready_tx, ready_rx) = mpsc::channel();
        ready.push((track, ready_rx));
        let path = folder.join(track.file_name());
        let stop = stop.clone();
        let levels = levels.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = audio::record(track, &path, stop, levels, ready_tx);
            let _ = tx.send(Msg::Done(track, result));
        });
    }
    for (track, ready_rx) in ready {
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                stop.store(true, Ordering::Relaxed);
                fail(&message);
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                fail(&format!(
                    "the {} recorder stopped before it started",
                    track.device_name()
                ));
            }
        }
    }
    emit(json!({ "event": "started", "folder": folder.to_string_lossy() }));

    // Report levels twice a second.
    {
        let levels = levels.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                let (mic, system) = levels.lock().unwrap().drain();
                emit(json!({ "event": "levels", "mic": mic, "system": system }));
            }
        });
    }

    // Stop on "stop" from stdin, or when the app goes away and stdin closes.
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) if l.trim() == "stop" => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            let _ = tx.send(Msg::Stop);
        });
    }

    let mut remaining = 2;
    let mut error: Option<String> = None;
    while remaining > 0 {
        match rx.recv() {
            Ok(Msg::Stop) => stop.store(true, Ordering::Relaxed),
            Ok(Msg::Done(track, result)) => {
                remaining -= 1;
                match &result {
                    Ok(secs) => eprintln!("{} track: {secs:.1} s written", track.device_name()),
                    Err(e) => eprintln!("{} track failed: {e}", track.device_name()),
                }
                // One track failing ends the recording; the other finishes its file first.
                stop.store(true, Ordering::Relaxed);
                if let Err(message) = result {
                    error.get_or_insert(message);
                }
            }
            Err(_) => break,
        }
    }
    if let Some(message) = error {
        emit(json!({ "event": "error", "message": message }));
    }
    emit(json!({ "event": "stopped", "folder": folder.to_string_lossy(), "reason": "user" }));
}

//! WASAPI capture of one track on its own thread, written as a 16 kHz mono WAV.
//!
//! "Me" is the default microphone. "Them" is a loopback stream on the default output
//! device, which hears whatever the computer plays, so no virtual cable or driver is needed.
//! A loopback stream only produces data while something is playing, so a silent render
//! stream is kept open on the same device: that way the file keeps the right length through
//! quiet stretches and both tracks stay in step.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wasapi::{
    initialize_mta, AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat,
};

use crate::dsp::{self, Resampler, SampleKind, OUT_RATE};
use crate::{Levels, Track};

/// How long WASAPI may buffer before we read (100 ns units). Reads happen every 10 ms, so
/// 200 ms of slack survives a busy moment without dropping audio.
const BUFFER_HNS: i64 = 2_000_000;
const POLL: Duration = Duration::from_millis(10);
/// Rewrite the WAV header this often so a file is playable even if the process dies.
const CHECKPOINT: Duration = Duration::from_secs(5);

fn sample_kind(format: &WaveFormat) -> Result<SampleKind, String> {
    let bits = format.get_bitspersample();
    match (format.get_subformat().map_err(|e| e.to_string())?, bits) {
        (SampleType::Float, 32) => Ok(SampleKind::Float32),
        (SampleType::Int, 16) => Ok(SampleKind::Int16),
        (SampleType::Int, 24) => Ok(SampleKind::Int24),
        (SampleType::Int, 32) => Ok(SampleKind::Int32),
        (kind, bits) => Err(format!("unsupported device format: {kind:?} {bits}-bit")),
    }
}

fn open_capture(track: Track) -> Result<(AudioClient, WaveFormat), String> {
    let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
    let direction = match track {
        Track::Mic => Direction::Capture,
        Track::System => Direction::Render,
    };
    let device = enumerator
        .get_default_device(&direction)
        .map_err(|e| format!("no default {} device: {e}", track.device_name()))?;
    let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
    let format = client.get_mixformat().map_err(|e| e.to_string())?;
    let mode = StreamMode::PollingShared {
        autoconvert: false,
        buffer_duration_hns: BUFFER_HNS,
    };
    // Asking a render device for a capture stream is how the crate sets up loopback.
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|e| match track {
            Track::Mic => format!(
                "couldn't open the microphone ({e}). If Windows blocks it, allow desktop apps \
                 to use the microphone in Settings > Privacy & security > Microphone."
            ),
            Track::System => format!("couldn't capture the computer's audio ({e})"),
        })?;
    Ok((client, format))
}

/// Plays silence on the default output until `stop` is set, to keep loopback flowing.
fn keep_alive(stop: Arc<AtomicBool>) -> Result<(), String> {
    initialize_mta().ok().map_err(|e| e.to_string())?;
    let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
    let device = enumerator
        .get_default_device(&Direction::Render)
        .map_err(|e| e.to_string())?;
    let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
    let format = client.get_mixformat().map_err(|e| e.to_string())?;
    let (default_period, _) = client.get_device_period().map_err(|e| e.to_string())?;
    let mode = StreamMode::EventsShared {
        autoconvert: false,
        buffer_duration_hns: default_period,
    };
    client
        .initialize_client(&format, &Direction::Render, &mode)
        .map_err(|e| e.to_string())?;
    let event = client.set_get_eventhandle().map_err(|e| e.to_string())?;
    let render = client.get_audiorenderclient().map_err(|e| e.to_string())?;
    let block = format.get_blockalign() as usize;
    let fill = |client: &AudioClient| -> Result<(), String> {
        let frames = client
            .get_available_space_in_frames()
            .map_err(|e| e.to_string())? as usize;
        if frames > 0 {
            render
                .write_to_device(frames, &vec![0u8; frames * block], None)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    fill(&client)?;
    client.start_stream().map_err(|e| e.to_string())?;
    while !stop.load(Ordering::Relaxed) {
        // A timeout just means the device is idle; keep topping the buffer up.
        let _ = event.wait_for_event(200);
        fill(&client)?;
    }
    let _ = client.stop_stream();
    Ok(())
}

/// Records one track until `stop` is set. Sends `Ok(())` on `ready` once audio is flowing
/// (or the error that prevented it), and returns the number of seconds written.
pub fn record(
    track: Track,
    path: &Path,
    stop: Arc<AtomicBool>,
    levels: Arc<Mutex<Levels>>,
    ready: Sender<Result<(), String>>,
) -> Result<f64, String> {
    let mut ready = Some(ready);
    let result = record_track(track, path, stop, levels, &mut ready);
    if let (Some(ready), Err(e)) = (ready.take(), &result) {
        let _ = ready.send(Err(e.clone()));
    }
    result
}

fn record_track(
    track: Track,
    path: &Path,
    stop: Arc<AtomicBool>,
    levels: Arc<Mutex<Levels>>,
    ready: &mut Option<Sender<Result<(), String>>>,
) -> Result<f64, String> {
    initialize_mta().ok().map_err(|e| e.to_string())?;
    let (client, format) = open_capture(track)?;
    let kind = sample_kind(&format)?;
    let channels = format.get_nchannels() as usize;
    let block = format.get_blockalign() as usize;
    let capture = client.get_audiocaptureclient().map_err(|e| e.to_string())?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: OUT_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| format!("couldn't create {}: {e}", path.display()))?;

    let keep_alive_thread = match track {
        Track::System => {
            let stop = stop.clone();
            Some(std::thread::spawn(move || keep_alive(stop)))
        }
        Track::Mic => None,
    };
    client.start_stream().map_err(|e| e.to_string())?;
    if let Some(ready) = ready.take() {
        let _ = ready.send(Ok(()));
    }

    let mut resampler = Resampler::new(format.get_samplespersec(), OUT_RATE);
    let mut pending: VecDeque<u8> = VecDeque::new();
    let mut raw: Vec<u8> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    let mut resampled: Vec<f32> = Vec::new();
    let mut last_checkpoint = Instant::now();
    let mut written: u64 = 0;
    let result = 'outer: loop {
        // Pull every packet the engine has queued, then convert what makes whole frames.
        loop {
            match capture.get_next_packet_size() {
                Ok(Some(0)) | Ok(None) => break,
                Ok(Some(_)) => {
                    let before = pending.len();
                    match capture.read_from_device_to_deque(&mut pending) {
                        Ok(info) => {
                            // The engine says the packet is silence; its bytes may be anything.
                            if info.flags.silent {
                                pending.range_mut(before..).for_each(|b| *b = 0);
                            }
                        }
                        Err(e) => {
                            break 'outer Err(format!(
                                "{} capture failed: {e}",
                                track.device_name()
                            ))
                        }
                    }
                }
                Err(e) => break 'outer Err(format!("{} capture failed: {e}", track.device_name())),
            }
        }
        let whole = pending.len() / block * block;
        raw.clear();
        raw.extend(pending.drain(..whole));
        mono.clear();
        dsp::downmix(&raw, kind, channels, &mut mono);
        if !mono.is_empty() {
            levels.lock().unwrap().update(track, dsp::rms(&mono));
            resampled.clear();
            resampler.process(&mono, &mut resampled);
            for s in &resampled {
                if let Err(e) = writer.write_sample(dsp::to_i16(*s)) {
                    break 'outer Err(format!("couldn't write {}: {e}", path.display()));
                }
            }
            written += resampled.len() as u64;
        }
        if last_checkpoint.elapsed() >= CHECKPOINT {
            let _ = writer.flush();
            last_checkpoint = Instant::now();
        }
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        std::thread::sleep(POLL);
    };
    let _ = client.stop_stream();
    stop.store(true, Ordering::Relaxed);
    if let Some(t) = keep_alive_thread {
        let _ = t.join();
    }
    resampled.clear();
    resampler.finish(&mut resampled);
    for s in &resampled {
        let _ = writer.write_sample(dsp::to_i16(*s));
    }
    written += resampled.len() as u64;
    writer
        .finalize()
        .map_err(|e| format!("couldn't finish {}: {e}", path.display()))?;
    result.map(|_| written as f64 / OUT_RATE as f64)
}

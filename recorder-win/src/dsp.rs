//! Pure signal processing shared by both tracks: interleaved device frames in, 16 kHz mono
//! 16-bit samples out. Kept free of Windows types so it can be unit-tested anywhere.

use std::f64::consts::PI;

/// The rate both WAV files are written at. whisper.cpp wants 16 kHz mono, so writing it
/// directly saves a conversion step later and keeps the files small.
pub const OUT_RATE: u32 = 16_000;

/// How samples are laid out in the bytes WASAPI hands over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    Float32,
    Int16,
    /// 24 valid bits in 3 bytes.
    Int24,
    /// 32-bit container (24 or 32 valid bits, left-aligned either way).
    Int32,
}

impl SampleKind {
    pub fn bytes(self) -> usize {
        match self {
            SampleKind::Float32 | SampleKind::Int32 => 4,
            SampleKind::Int24 => 3,
            SampleKind::Int16 => 2,
        }
    }

    fn read(self, b: &[u8]) -> f32 {
        match self {
            SampleKind::Float32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            SampleKind::Int16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            SampleKind::Int24 => i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32 / 2_147_483_648.0,
            SampleKind::Int32 => {
                i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
            }
        }
    }
}

/// Turns interleaved frames into mono by averaging the channels. Only whole frames are
/// read; returns how many bytes were consumed.
pub fn downmix(bytes: &[u8], kind: SampleKind, channels: usize, out: &mut Vec<f32>) -> usize {
    let frame = kind.bytes() * channels.max(1);
    let frames = bytes.len() / frame;
    for i in 0..frames {
        let mut sum = 0.0f32;
        for c in 0..channels.max(1) {
            let at = i * frame + c * kind.bytes();
            sum += kind.read(&bytes[at..at + kind.bytes()]);
        }
        out.push(sum / channels.max(1) as f32);
    }
    frames * frame
}

/// Root mean square of a block, used for the level meters.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

pub fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Windowed-sinc resampler for one mono stream. Good enough for speech and simple enough
/// to trust: a low-pass at the output's Nyquist frequency, interpolated at each output time.
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Low-pass cutoff in cycles per input sample.
    cutoff: f64,
    /// Taps on each side of the output time.
    half: usize,
    /// Input not yet fully consumed; `pos` is the next output time within it.
    buf: Vec<f32>,
    pos: f64,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let step = in_rate as f64 / out_rate as f64;
        Self {
            step,
            // Slightly under Nyquist so the transition band stays inside it.
            cutoff: 0.46 / step.max(1.0),
            half: ((16.0 * step.max(1.0)).ceil() as usize).min(64),
            buf: Vec::new(),
            pos: 0.0,
        }
    }

    pub fn is_identity(&self) -> bool {
        self.step == 1.0
    }

    fn kernel(&self, t: f64) -> f64 {
        let half = self.half as f64;
        if t.abs() >= half {
            return 0.0;
        }
        let window = 0.5 * (1.0 + (PI * t / half).cos());
        let x = 2.0 * self.cutoff * t;
        let sinc = if x.abs() < 1e-9 {
            1.0
        } else {
            (PI * x).sin() / (PI * x)
        };
        sinc * window
    }

    /// Feeds input samples and appends every output sample that can now be computed.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        self.drain(out, false);
    }

    /// Flushes what is left at the end of the stream.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.is_identity() {
            return;
        }
        self.buf.extend(std::iter::repeat_n(0.0, self.half + 1));
        self.drain(out, true);
        self.buf.clear();
        self.pos = 0.0;
    }

    fn drain(&mut self, out: &mut Vec<f32>, flushing: bool) {
        loop {
            let center = self.pos.floor() as usize;
            if center + self.half >= self.buf.len() {
                break;
            }
            let first = center.saturating_sub(self.half - 1);
            let last = center + self.half;
            let mut acc = 0.0f64;
            let mut weight = 0.0f64;
            for k in first..=last {
                let w = self.kernel(k as f64 - self.pos);
                acc += w * self.buf[k] as f64;
                weight += w;
            }
            out.push(if weight.abs() > 1e-12 {
                (acc / weight) as f32
            } else {
                0.0
            });
            self.pos += self.step;
        }
        if !flushing {
            // Keep only the history the next output sample still needs.
            let keep_from = (self.pos.floor() as usize).saturating_sub(self.half);
            if keep_from > 0 {
                self.buf.drain(..keep_from);
                self.pos -= keep_from as f64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels_and_reads_each_format() {
        let mut out = Vec::new();
        let bytes: Vec<u8> = [0.5f32, -0.5, 1.0, 0.0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let used = downmix(&bytes, SampleKind::Float32, 2, &mut out);
        assert_eq!(used, 16);
        assert_eq!(out, vec![0.0, 0.5]);

        out.clear();
        let bytes = [16384i16, 16384, -32768]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect::<Vec<u8>>();
        // Three samples, mono: the odd trailing bytes of a half frame would be left over.
        let used = downmix(&bytes[..5], SampleKind::Int16, 1, &mut out);
        assert_eq!(used, 4);
        assert_eq!(out, vec![0.5, 0.5]);

        out.clear();
        downmix(
            &0x40_00_00i32.to_le_bytes()[..3],
            SampleKind::Int24,
            1,
            &mut out,
        );
        assert!((out[0] - 0.5).abs() < 1e-6);
        out.clear();
        downmix(
            &(i32::MAX / 2).to_le_bytes(),
            SampleKind::Int32,
            1,
            &mut out,
        );
        assert!((out[0] - 0.5).abs() < 1e-6);
    }

    fn sine(rate: u32, hz: f64, seconds: f64) -> Vec<f32> {
        (0..(rate as f64 * seconds) as usize)
            .map(|i| (2.0 * PI * hz * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    #[test]
    fn resampler_keeps_a_voice_band_tone_and_the_right_length() {
        for in_rate in [48_000u32, 44_100, 22_050, 96_000] {
            let mut r = Resampler::new(in_rate, OUT_RATE);
            let input = sine(in_rate, 440.0, 1.0);
            let mut out = Vec::new();
            // Feed in uneven chunks, like device packets.
            for chunk in input.chunks(439) {
                r.process(chunk, &mut out);
            }
            r.finish(&mut out);
            let expected = OUT_RATE as usize;
            assert!(
                (out.len() as i64 - expected as i64).abs() <= 2,
                "{in_rate}: got {} samples",
                out.len()
            );
            // Skip the edges and check the tone survived with its amplitude.
            let middle = &out[2000..14000];
            let level = rms(middle);
            assert!((level - 0.707).abs() < 0.02, "{in_rate}: rms {level}");
            // And that it is still a 440 Hz sine: compare with the ideal.
            let err: f32 = middle
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let t = (i + 2000) as f64 / OUT_RATE as f64;
                    (s - (2.0 * PI * 440.0 * t).sin() as f32).abs()
                })
                .fold(0.0, f32::max);
            assert!(err < 0.03, "{in_rate}: max error {err}");
        }
    }

    #[test]
    fn resampler_removes_what_would_alias() {
        let mut r = Resampler::new(48_000, OUT_RATE);
        let mut out = Vec::new();
        r.process(&sine(48_000, 12_000.0, 0.5), &mut out);
        r.finish(&mut out);
        assert!(rms(&out[1000..7000]) < 0.01);
    }

    #[test]
    fn identity_rate_passes_through() {
        let mut r = Resampler::new(OUT_RATE, OUT_RATE);
        let mut out = Vec::new();
        r.process(&[0.1, 0.2, 0.3], &mut out);
        r.finish(&mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn i16_conversion_clamps() {
        assert_eq!(to_i16(2.0), 32767);
        assert_eq!(to_i16(-2.0), -32767);
        assert_eq!(to_i16(0.0), 0);
    }
}

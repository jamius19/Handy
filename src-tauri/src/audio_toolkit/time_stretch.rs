/*
 * Author: Jamius Siam
 * Since: 12/11/2026
 */

//! Speeds mono audio up with Sonic (vendor/sonic), pitch kept, so cloud
//! models are sent (and bill) less audio. Sonic holds back ~30 ms, released
//! by `finish`.

use log::{error, info};
use std::os::raw::{c_float, c_int};
use std::ptr::NonNull;

#[repr(C)]
struct SonicStream {
    _private: [u8; 0],
}

// Only these: sonicSetPitch/Rate/SampleRate/NumChannels/UserData are not
// needed, and Get/SetUserData don't link at this commit.
extern "C" {
    fn sonicCreateStream(sample_rate: c_int, num_channels: c_int) -> *mut SonicStream;
    fn sonicDestroyStream(stream: *mut SonicStream);
    fn sonicSetSpeed(stream: *mut SonicStream, speed: c_float);
    fn sonicSetQuality(stream: *mut SonicStream, quality: c_int);
    fn sonicWriteFloatToStream(
        stream: *mut SonicStream,
        samples: *mut c_float,
        num_samples: c_int,
    ) -> c_int;
    fn sonicReadFloatFromStream(
        stream: *mut SonicStream,
        samples: *mut c_float,
        max_samples: c_int,
    ) -> c_int;
    fn sonicSamplesAvailable(stream: *mut SonicStream) -> c_int;
    fn sonicFlushStream(stream: *mut SonicStream) -> c_int;
}

/// Most samples written to Sonic at once (~4 s at 16 kHz).
const MAX_WRITE: usize = 1 << 16;

/// One stream of 16 kHz (or `rate`) mono audio sped up `percent`/100 times.
pub struct Stretcher {
    /// `None` once Sonic ran out of memory: the rest is sent as it is.
    stream: Option<NonNull<SonicStream>>,
    percent: u8,
    rate: u32,
    samples_in: usize,
    samples_out: usize,
}

// SAFETY: the stream is owned, used only through `&mut self`, and Sonic
// keeps no thread-local state. Not Sync.
unsafe impl Send for Stretcher {}

impl Stretcher {
    /// `None` at 100 % or less: the audio is sent untouched, with no Sonic.
    pub fn new(percent: u8, rate: u32) -> Option<Self> {
        let percent = percent.min(200);
        if percent <= 100 {
            return None;
        }
        // SAFETY: plain constructor; a NULL result is handled below.
        let Some(stream) = NonNull::new(unsafe { sonicCreateStream(rate as c_int, 1) }) else {
            error!("Sonic ran out of memory; sending the audio at 1.0x");
            return None;
        };
        // SAFETY: `stream` is a live stream. Pitch, rate and volume stay at
        // Sonic's default 1.0, so the output rate is the input rate.
        unsafe {
            sonicSetSpeed(stream.as_ptr(), percent as f32 / 100.0);
            sonicSetQuality(stream.as_ptr(), 0);
        }
        Some(Self {
            stream: Some(stream),
            percent,
            rate,
            samples_in: 0,
            samples_out: 0,
        })
    }

    /// Feeds `pcm` and returns what Sonic has ready; `None` when nothing is
    /// (never an empty Vec). Clamps `pcm` to -1..1 in place first: Sonic's
    /// float-to-short conversion is UB out of range.
    pub fn push(&mut self, mut pcm: Vec<f32>) -> Option<Vec<f32>> {
        self.samples_in += pcm.len();
        let out = match self.stream {
            Some(stream) => {
                // Not `clamp`: this also maps NaN to -1.0.
                #[allow(clippy::manual_clamp)]
                for s in pcm.iter_mut() {
                    *s = s.max(-1.0).min(1.0);
                }
                // Sonic sizes its buffers with `int` math, so one huge write
                // would overflow. Pieces, each drained, keep them small too.
                let (len, mut out) = (pcm.len(), Vec::new());
                for start in (0..len).step_by(MAX_WRITE) {
                    let piece = &mut pcm[start..(start + MAX_WRITE).min(len)];
                    // SAFETY: `stream` is live and `piece` holds `len` samples.
                    let written = unsafe {
                        sonicWriteFloatToStream(
                            stream.as_ptr(),
                            piece.as_mut_ptr(),
                            piece.len() as c_int,
                        )
                    };
                    if written != 1 {
                        self.fail();
                        out.extend_from_slice(&pcm[start..]);
                        break;
                    }
                    out.extend(self.read());
                }
                out
            }
            None => pcm,
        };
        self.samples_out += out.len();
        (!out.is_empty()).then_some(out)
    }

    /// Flushes the held-back tail and logs `Speed {:.1}x: sent A ms as B ms`.
    /// Call only when the audio ended normally; drop it otherwise.
    pub fn finish(mut self) -> Option<Vec<f32>> {
        let tail = match self.stream {
            // SAFETY: `stream` is live.
            Some(stream) => match unsafe { sonicFlushStream(stream.as_ptr()) } {
                1 => self.read(),
                _ => {
                    self.fail();
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        self.samples_out += tail.len();
        let ms = |samples: usize| samples as u64 * 1000 / self.rate as u64;
        info!(
            "Speed {:.1}x: sent {} ms as {} ms",
            self.percent as f32 / 100.0,
            ms(self.samples_in),
            ms(self.samples_out)
        );
        (!tail.is_empty()).then_some(tail)
    }

    /// Everything Sonic has ready.
    fn read(&mut self) -> Vec<f32> {
        let Some(stream) = self.stream else {
            return Vec::new();
        };
        // SAFETY: `stream` is live.
        let available = unsafe { sonicSamplesAvailable(stream.as_ptr()) };
        if available <= 0 {
            return Vec::new();
        }
        let mut out = vec![0.0; available as usize];
        // SAFETY: `out` has room for `available` samples.
        let read =
            unsafe { sonicReadFloatFromStream(stream.as_ptr(), out.as_mut_ptr(), available) };
        out.truncate(read.max(0) as usize);
        out
    }

    /// Drops the stream after Sonic ran out of memory.
    fn fail(&mut self) {
        error!("Sonic ran out of memory; sending the rest at 1.0x");
        if let Some(stream) = self.stream.take() {
            // SAFETY: `stream` is live and not used again.
            unsafe { sonicDestroyStream(stream.as_ptr()) };
        }
    }
}

impl Drop for Stretcher {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            // SAFETY: `stream` is live and not used again.
            unsafe { sonicDestroyStream(stream.as_ptr()) };
        }
    }
}

/// `seconds` of a voice-like 150 Hz tone (harmonics 1-5 at 1/h, ~0.18 peak)
/// at `rate`, in 50 ms chunks.
#[cfg(test)]
pub(crate) fn voice(rate: u32, seconds: u32) -> Vec<Vec<f32>> {
    let harmonics: f32 = (1..=5).map(|h| 1.0 / h as f32).sum();
    let scale = 0.18 / harmonics;
    let samples: Vec<f32> = (0..rate * seconds)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let s: f32 = (1..=5)
                .map(|h| (2.0 * std::f32::consts::PI * 150.0 * h as f32 * t).sin() / h as f32)
                .sum();
            s * scale
        })
        .collect();
    samples
        .chunks(rate as usize / 20)
        .map(<[f32]>::to_vec)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything `chunks` gives out, `finish` included unless `finish` is false.
    fn run(percent: u8, rate: u32, chunks: Vec<Vec<f32>>, finish: bool) -> Vec<f32> {
        let mut stretcher = Stretcher::new(percent, rate).unwrap();
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(stretcher.push(chunk).unwrap_or_default());
        }
        if finish {
            out.extend(stretcher.finish().unwrap_or_default());
        }
        out
    }

    fn within(actual: usize, expected: f64, tolerance: f64) -> bool {
        (actual as f64 - expected).abs() <= expected * tolerance
    }

    fn sine(rate: u32, seconds: u32) -> Vec<f32> {
        (0..rate * seconds)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 200.0 * i as f32 / rate as f32).sin())
            .collect()
    }

    fn zero_crossing_rate(samples: &[f32]) -> f64 {
        let crossings = samples
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        crossings as f64 / samples.len() as f64
    }

    #[test]
    fn no_sonic_at_or_below_100() {
        assert!(Stretcher::new(100, 16_000).is_none());
        assert!(Stretcher::new(90, 16_000).is_none());
    }

    #[test]
    fn output_length_is_input_divided_by_speed() {
        for rate in [16_000, 24_000] {
            for percent in [150, 200] {
                let input = (rate * 5) as f64;
                let out = run(percent, rate, voice(rate, 5), true);
                let expected = input * 100.0 / percent as f64;
                assert!(
                    within(out.len(), expected, 0.01),
                    "{rate} Hz at {percent}%: {} samples, expected {expected}",
                    out.len()
                );
            }
        }
    }

    #[test]
    fn tail_comes_from_finish() {
        let mut stretcher = Stretcher::new(200, 16_000).unwrap();
        let mut total = 0;
        for chunk in voice(16_000, 1) {
            total += stretcher.push(chunk).map_or(0, |out| out.len());
        }
        let tail = stretcher.finish().expect("a tail");
        assert!(!tail.is_empty());
        // Sonic ends ~5 ms short, over 1% of a second's output.
        assert!(
            within(total + tail.len(), 8_000.0, 0.02),
            "{total} + {}",
            tail.len()
        );
    }

    #[test]
    fn dropping_flushes_nothing() {
        let out = run(200, 16_000, voice(16_000, 1), false);
        assert!(out.len() < 8_000);
        assert!(out.len() as f64 > 0.9 * 8_000.0);
    }

    #[test]
    fn finish_without_audio_is_none() {
        assert!(Stretcher::new(200, 16_000).unwrap().finish().is_none());
    }

    #[test]
    fn never_returns_empty() {
        let mut stretcher = Stretcher::new(110, 24_000).unwrap();
        assert!(stretcher.push(vec![0.1]).is_none());
        let mut outputs = 0;
        for chunk in voice(24_000, 1) {
            if let Some(out) = stretcher.push(chunk) {
                assert!(!out.is_empty());
                outputs += 1;
            }
        }
        if let Some(tail) = stretcher.finish() {
            assert!(!tail.is_empty());
        }
        assert!(outputs > 0);
    }

    #[test]
    fn pitch_is_kept() {
        let input = sine(16_000, 2);
        let out = run(200, 16_000, vec![input.clone()], true);
        let (before, after) = (zero_crossing_rate(&input), zero_crossing_rate(&out));
        assert!(
            (after - before).abs() <= before * 0.05,
            "zero-crossing rate {after} vs {before}"
        );
    }

    #[test]
    fn out_of_range_input_is_clamped() {
        // Unclamped, 1.5 comes out of Sonic as about -0.5.
        let out = run(200, 16_000, vec![vec![1.5; 800]; 20], true);
        assert!(out.iter().all(|s| *s >= 0.0));
        assert!(out.iter().copied().fold(f32::MIN, f32::max) >= 0.99);
    }
}

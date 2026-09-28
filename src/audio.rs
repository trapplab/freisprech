//! Microphone capture: device format -> 16 kHz mono i16 chunks of ~100 ms, with the
//! channels that carry signal mixed down and quiet speech raised to a steady level.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use tokio::sync::mpsc;

use crate::asr::SAMPLE_RATE;

/// Chunks per second sent to the ASR (100 ms each).
const CHUNKS_PER_SECOND: u32 = 10;

/// Names of all input devices, for the microphone selection.
pub fn input_devices() -> Vec<String> {
    let Ok(devices) = cpal::default_host().input_devices() else {
        return Vec::new();
    };
    devices.filter_map(|d| device_name(&d)).collect()
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_owned())
}

/// Running capture from an input device. [`stop`](Self::stop) it to also get the audio
/// that is still buffered; dropping it closes the microphone without that.
pub struct Microphone {
    stream: cpal::Stream,
    converter: Arc<Mutex<Converter>>,
    tx: mpsc::Sender<Vec<i16>>,
}

impl Microphone {
    /// Starts capturing from the named device (system default if `None` or not found);
    /// chunks are sent to `tx` and dropped if the receiver lags.
    pub fn start(name: Option<&str>, tx: mpsc::Sender<Vec<i16>>) -> Result<Self> {
        let host = cpal::default_host();
        let named = name.and_then(|name| {
            let device = host
                .input_devices()
                .ok()?
                .find(|d| device_name(d).as_deref() == Some(name));
            if device.is_none() {
                tracing::warn!(name, "Microphone not found, using system default");
            }
            device
        });
        let device = match named {
            Some(device) => device,
            None => host.default_input_device().context("No microphone found")?,
        };
        let supported = device.default_input_config()?;
        let config: StreamConfig = supported.config();
        tracing::info!(
            device = device_name(&device).unwrap_or_default(),
            rate = config.sample_rate,
            channels = config.channels,
            format = ?supported.sample_format(),
            "Microphone opened"
        );

        let converter = Arc::new(Mutex::new(Converter::new(
            config.sample_rate,
            config.channels as usize,
        )?));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, converter.clone(), tx.clone())?,
            SampleFormat::I16 => build::<i16>(&device, config, converter.clone(), tx.clone())?,
            SampleFormat::I32 => build::<i32>(&device, config, converter.clone(), tx.clone())?,
            SampleFormat::U16 => build::<u16>(&device, config, converter.clone(), tx.clone())?,
            other => bail!("Unsupported sample format: {other:?}"),
        };
        stream.play()?;
        Ok(Self {
            stream,
            converter,
            tx,
        })
    }

    /// Closes the microphone and sends what is still buffered: up to one chunk plus the
    /// resampler's delay, i.e. the end of the last word.
    pub fn stop(self) {
        let Self {
            stream,
            converter,
            tx,
        } = self;
        // Stop the callbacks first, so nothing arrives after the tail.
        drop(stream);
        let mut converter = lock(&converter);
        for chunk in converter.finish() {
            let _ = tx.try_send(chunk);
        }
        converter.log_summary();
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    converter: Arc<Mutex<Converter>>,
    tx: mpsc::Sender<Vec<i16>>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let samples: Vec<f32> = data.iter().map(|&s| f32::from_sample(s)).collect();
            for chunk in lock(&converter).feed(&samples) {
                // Capture must never block; a full channel means the consumer is gone or stalled.
                let _ = tx.try_send(chunk);
            }
        },
        |err| tracing::error!(%err, "Microphone error"),
        None,
    )?;
    Ok(stream)
}

/// The lock is only contended while stopping; a panicked callback leaves usable state.
fn lock(converter: &Mutex<Converter>) -> MutexGuard<'_, Converter> {
    converter.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Mixes down, buffers mono samples at device rate and emits resampled, level-adjusted
/// 16 kHz i16 chunks.
struct Converter {
    downmix: Downmix,
    resampler: Option<Fft<f32>>,
    agc: Agc,
    chunk_in: usize,
    pending: Vec<f32>,
}

impl Converter {
    fn new(device_rate: u32, channels: usize) -> Result<Self> {
        let chunk_in = (device_rate / CHUNKS_PER_SECOND) as usize;
        let resampler = if device_rate == SAMPLE_RATE {
            None
        } else {
            let fft = Fft::new(
                device_rate as usize,
                SAMPLE_RATE as usize,
                chunk_in,
                1,
                FixedSync::Input,
            )
            .map_err(|e| anyhow!("Resampler: {e}"))?;
            Some(fft)
        };
        Ok(Self {
            downmix: Downmix::new(channels),
            resampler,
            agc: Agc::default(),
            chunk_in,
            pending: Vec::with_capacity(chunk_in * 2),
        })
    }

    /// Takes interleaved samples in the device's channel layout.
    fn feed(&mut self, interleaved: &[f32]) -> Vec<Vec<i16>> {
        self.downmix.mix(interleaved, &mut self.pending);
        self.drain()
    }

    /// Pads what is still buffered with silence, plus enough to push the resampler's
    /// delay out, and emits it.
    fn finish(&mut self) -> Vec<Vec<i16>> {
        let delay_chunks = self
            .resampler
            .as_ref()
            .map_or(0, |r| r.output_delay().div_ceil(r.output_frames_next()));
        let pad = (self.chunk_in - self.pending.len() % self.chunk_in) % self.chunk_in
            + delay_chunks * self.chunk_in;
        self.pending.extend(std::iter::repeat_n(0.0, pad));
        self.drain()
    }

    fn drain(&mut self) -> Vec<Vec<i16>> {
        let mut out = Vec::new();
        while self.pending.len() >= self.chunk_in {
            let block: Vec<f32> = self.pending.drain(..self.chunk_in).collect();
            let mut resampled = match &mut self.resampler {
                None => block,
                Some(r) => {
                    let input = InterleavedSlice::new(&block, 1, block.len())
                        .expect("buffer size matches channel count");
                    match r.process(&input, None) {
                        Ok(buf) => buf.take_data(),
                        Err(err) => {
                            tracing::error!(%err, "Resampling failed");
                            continue;
                        }
                    }
                }
            };
            self.agc.process(&mut resampled);
            out.push(
                resampled
                    .into_iter()
                    .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .collect(),
            );
        }
        out
    }

    /// Levels in the log, to tell a quiet microphone from other problems.
    fn log_summary(&self) {
        let dbfs = |level: f32| (20.0 * level.log10()).round();
        tracing::info!(
            channels_used = ?self.downmix.active,
            speech_dbfs = self.agc.speech.map(dbfs),
            noise_dbfs = dbfs(self.agc.noise),
            gain_db = dbfs(self.agc.gain),
            "Microphone closed"
        );
    }
}

/// Mono mix of the channels that carry signal. Channels more than 10 dB below the loudest
/// are left out: some devices carry the microphone on only one channel, and mixing in the
/// others would halve the level and add their noise.
struct Downmix {
    channels: usize,
    /// Signal energy per channel, decaying per callback.
    energy: Vec<f32>,
    /// Indices of the channels in the mix.
    active: Vec<usize>,
}

impl Downmix {
    const DECAY: f32 = 0.9;
    /// 10 dB in energy.
    const MAX_DISTANCE: f32 = 10.0;

    fn new(channels: usize) -> Self {
        Self {
            channels,
            energy: vec![0.0; channels],
            active: (0..channels).collect(),
        }
    }

    fn mix(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        for (channel, energy) in self.energy.iter_mut().enumerate() {
            let block: f32 = interleaved
                .iter()
                .skip(channel)
                .step_by(self.channels)
                .map(|s| s * s)
                .sum();
            *energy = *energy * Self::DECAY + block;
        }
        let loudest = self.energy.iter().copied().fold(0.0, f32::max);
        self.active.clear();
        self.active
            .extend((0..self.channels).filter(|&c| self.energy[c] * Self::MAX_DISTANCE >= loudest));
        let n = self.active.len() as f32;
        out.extend(
            interleaved
                .chunks_exact(self.channels)
                .map(|frame| self.active.iter().map(|&c| frame[c]).sum::<f32>() / n),
        );
    }
}

/// Automatic gain control on the 16 kHz signal. Raises quiet speech to a steady level
/// before the conversion to 16 bit, which would otherwise cost a quiet microphone its
/// detail: recognition degrades from about -40 dBFS speech level. Only amplifies; a
/// limiter keeps the raised signal from clipping.
struct Agc {
    gain: f32,
    /// Smoothed RMS of speech frames before the gain; `None` until speech was heard.
    speech: Option<f32>,
    /// Background RMS: follows quieter frames at once and louder ones slowly.
    noise: f32,
}

impl Default for Agc {
    fn default() -> Self {
        Self {
            gain: 1.0,
            speech: None,
            noise: f32::INFINITY,
        }
    }
}

impl Agc {
    /// Speech RMS to aim for, about -26 dBFS: the level of the test recording.
    const TARGET: f32 = 0.05;
    /// +30 dB.
    const MAX_GAIN: f32 = 31.6;
    /// Highest peak after the gain.
    const LIMIT: f32 = 0.9;
    /// 10 ms.
    const FRAME: usize = SAMPLE_RATE as usize / 100;
    /// A frame is speech 12 dB above the background …
    const SPEECH_OVER_NOISE: f32 = 4.0;
    /// … and above -80 dBFS, so digital silence is never taken for speech.
    const MIN_SPEECH: f32 = 1e-4;
    /// Per frame: the speech level follows with a time constant of ~0.3 s …
    const SPEECH_SMOOTHING: f32 = 0.97;
    /// … and the background rises by at most ~6 dB/s.
    const NOISE_RISE: f32 = 1.007;

    fn process(&mut self, samples: &mut [f32]) {
        for frame in samples.chunks_mut(Self::FRAME) {
            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
            self.noise = if rms < self.noise {
                rms.max(1e-6)
            } else {
                self.noise * Self::NOISE_RISE
            };
            if rms > Self::MIN_SPEECH && rms > self.noise * Self::SPEECH_OVER_NOISE {
                self.speech = Some(match self.speech {
                    Some(level) => {
                        level * Self::SPEECH_SMOOTHING + rms * (1.0 - Self::SPEECH_SMOOTHING)
                    }
                    None => rms,
                });
            }
            let wanted = self.speech.map_or(1.0, |level| {
                (Self::TARGET / level).clamp(1.0, Self::MAX_GAIN)
            });
            let peak = frame.iter().fold(0.0, |max: f32, s| max.max(s.abs()));
            let target = wanted
                .min(Self::LIMIT / peak.max(f32::MIN_POSITIVE))
                .max(1.0);
            if target < self.gain {
                // Down at once, so the peak in this frame doesn't clip …
                self.gain = target;
                frame.iter_mut().for_each(|s| *s *= target);
            } else {
                // … up gradually, so there are no clicks.
                let step = (target - self.gain) / frame.len() as f32;
                for s in frame.iter_mut() {
                    self.gain += step;
                    *s *= self.gain;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// 1 s of a 200 Hz tone, standing in for speech, after 0.5 s of quiet background.
    fn quiet_speech(level: f32) -> Vec<f32> {
        let background = (0..8_000).map(|i| if i % 2 == 0 { 1e-5 } else { -1e-5 });
        let tone = (0..16_000).map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            level * std::f32::consts::SQRT_2 * (2.0 * std::f32::consts::PI * 200.0 * t).sin()
        });
        background.chain(tone).collect()
    }

    #[test]
    fn agc_raises_quiet_speech_to_the_target() {
        let mut samples = quiet_speech(0.005);
        Agc::default().process(&mut samples);
        let level = rms(&samples[16_000..]);
        assert!((level / Agc::TARGET - 1.0).abs() < 0.1, "level {level}");
    }

    #[test]
    fn agc_leaves_loud_speech_and_silence_alone() {
        let original = quiet_speech(0.2);
        let mut samples = original.clone();
        Agc::default().process(&mut samples);
        assert_eq!(samples, original);

        let mut silence = vec![0.0; 16_000];
        Agc::default().process(&mut silence);
        assert!(silence.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn agc_does_not_clip() {
        let mut samples = quiet_speech(0.005);
        samples.extend(vec![0.5; 160]);
        Agc::default().process(&mut samples);
        assert!(samples.iter().all(|s| s.abs() <= Agc::LIMIT + 1e-6));
    }

    #[test]
    fn downmix_skips_a_silent_channel() {
        let mut downmix = Downmix::new(2);
        let interleaved: Vec<f32> = (0..480).flat_map(|_| [0.5, 0.0]).collect();
        let mut out = Vec::new();
        downmix.mix(&interleaved, &mut out);
        assert_eq!(downmix.active, [0]);
        assert!(out.iter().all(|&s| s == 0.5));
    }

    #[test]
    fn downmix_averages_similar_channels() {
        let mut downmix = Downmix::new(2);
        let interleaved: Vec<f32> = (0..480).flat_map(|_| [0.5, 0.3]).collect();
        let mut out = Vec::new();
        downmix.mix(&interleaved, &mut out);
        assert_eq!(downmix.active, [0, 1]);
        assert!(out.iter().all(|&s| (s - 0.4).abs() < 1e-6));
    }

    #[test]
    fn finish_emits_the_buffered_tail() {
        let mut converter = Converter::new(48_000, 1).unwrap();
        // 150 ms of signal: one full chunk now, the rest only on finish.
        let signal = vec![0.1; 7_200];
        let early: usize = converter.feed(&signal).iter().map(Vec::len).sum();
        let tail = converter.finish();
        let tail_samples: Vec<i16> = tail.concat();
        let late_signal = tail_samples.iter().filter(|&&s| s != 0).count();
        // 150 ms at 16 kHz = 2400 samples in total, minus resampler edge effects.
        assert!(
            early + late_signal >= 2_300,
            "early {early}, late {late_signal}"
        );
    }
}

//! Microphone capture: device format -> 16 kHz mono i16 chunks of ~100 ms.

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

/// Running capture from an input device. Dropping it closes the microphone.
pub struct Microphone {
    _stream: cpal::Stream,
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

        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, tx)?,
            SampleFormat::I16 => build::<i16>(&device, config, tx)?,
            SampleFormat::I32 => build::<i32>(&device, config, tx)?,
            SampleFormat::U16 => build::<u16>(&device, config, tx)?,
            other => bail!("Unsupported sample format: {other:?}"),
        };
        stream.play()?;
        Ok(Self { _stream: stream })
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    tx: mpsc::Sender<Vec<i16>>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let mut converter = Converter::new(config.sample_rate)?;
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mono = data
                .chunks_exact(channels)
                .map(|frame| frame.iter().map(|&s| f32::from_sample(s)).sum::<f32>() / channels as f32);
            for chunk in converter.feed(mono) {
                // Capture must never block; a full channel means the consumer is gone or stalled.
                let _ = tx.try_send(chunk);
            }
        },
        |err| tracing::error!(%err, "Microphone error"),
        None,
    )?;
    Ok(stream)
}

/// Buffers mono samples at device rate and emits resampled 16 kHz i16 chunks.
struct Converter {
    resampler: Option<Fft<f32>>,
    chunk_in: usize,
    pending: Vec<f32>,
}

impl Converter {
    fn new(device_rate: u32) -> Result<Self> {
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
            resampler,
            chunk_in,
            pending: Vec::with_capacity(chunk_in * 2),
        })
    }

    fn feed(&mut self, samples: impl Iterator<Item = f32>) -> Vec<Vec<i16>> {
        self.pending.extend(samples);
        let mut out = Vec::new();
        while self.pending.len() >= self.chunk_in {
            let block: Vec<f32> = self.pending.drain(..self.chunk_in).collect();
            let resampled = match &mut self.resampler {
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
            out.push(
                resampled
                    .into_iter()
                    .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .collect(),
            );
        }
        out
    }
}

//! Experimental: transcribes the recording again with Whisper for the final text. Whisper
//! can't stream, but is more accurate than the live model and punctuates on its own.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use foundry_local_sdk::{AudioSession, FoundryLocalManager, Item, Model, Request, RequestOptions};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::asr::SAMPLE_RATE;

pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
/// Faster, less accurate.
pub const SMALL_MODEL: &str = "whisper-small";
/// Between the two.
pub const MEDIUM_MODEL: &str = "whisper-medium";

pub struct Whisper {
    _manager: Arc<FoundryLocalManager>,
    model: Arc<Model>,
}

impl Whisper {
    /// Downloads the model on first use (reporting progress in percent) and loads it.
    pub async fn load(alias: &str, on_progress: impl FnMut(f64) + Send + 'static) -> Result<Self> {
        let (manager, model) = crate::asr::fetch_model(alias, on_progress).await?;
        model.load().await?;
        Ok(Self {
            _manager: manager,
            model,
        })
    }

    /// Transcribes 16 kHz mono audio. `language` is e.g. `"de"`, `"de-DE"` or `"auto"`.
    pub async fn transcribe(&self, samples: &[i16], language: &str) -> Result<String> {
        // Whisper only reads audio from files; inline data needs a streaming model.
        let file = TempWav::write(samples)?;
        let session = AudioSession::new(&self.model).await?;
        let audio = Item::audio_uri(file.path_str(), Some("wav"), SAMPLE_RATE as i32, 1);
        let mut request = Request::from_items(vec![audio]);
        if language != crate::language::DETECT {
            // Whisper knows languages, not regions.
            let code = language.split('-').next().unwrap_or(language);
            request = request.with_options(RequestOptions {
                additional_options: vec![("language".into(), code.into())],
                ..Default::default()
            });
        }
        let response = session.process_request(request).await?;
        tracing::debug!(?response, "Whisper response");
        let mut text = String::new();
        for item in &response.items {
            if let Some(result) = item.as_speech_result() {
                text.push_str(&result.text);
            } else if let Some(t) = item.as_text() {
                text.push_str(t);
            }
        }
        Ok(text.trim().to_owned())
    }
}

/// A WAV file in the temp directory, deleted when dropped.
struct TempWav(PathBuf);

impl TempWav {
    fn write(samples: &[i16]) -> Result<Self> {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("freisprech-{}-{n}.wav", std::process::id()));
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // bytes per second
        bytes.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        std::fs::write(&path, bytes).with_context(|| format!("Cannot write {}", path.display()))?;
        Ok(Self(path))
    }

    fn path_str(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Transcribes a running recording with Whisper: cuts it into sections at pauses and
/// transcribes each finished section in the background, so that after stopping only the
/// last one is left. Whisper always processes 30 s, so a call costs the same however short
/// the section (about 14 s on a 4-core laptop CPU): sections are as long as possible.
pub struct Sections {
    cutter: Cutter,
    sections: mpsc::UnboundedSender<Vec<i16>>,
    worker: JoinHandle<Result<String>>,
}

impl Sections {
    pub fn start(whisper: Arc<Whisper>, language: &str) -> Self {
        let (sections, mut rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let language = language.to_owned();
        let worker = tokio::spawn(async move {
            let mut text = String::new();
            while let Some(section) = rx.recv().await {
                let start = Instant::now();
                let piece = whisper.transcribe(&section, &language).await?;
                tracing::debug!(
                    seconds = section.len() as f32 / SAMPLE_RATE as f32,
                    ms = start.elapsed().as_millis(),
                    piece,
                    "Whisper section"
                );
                if !piece.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&piece);
                }
            }
            Ok(text)
        });
        Self {
            cutter: Cutter::default(),
            sections,
            worker,
        }
    }

    pub fn push(&mut self, samples: &[i16]) {
        for section in self.cutter.push(samples) {
            let _ = self.sections.send(section);
        }
    }

    /// Transcribes the rest and returns the whole text.
    pub async fn finish(mut self) -> Result<String> {
        if let Some(rest) = self.cutter.finish() {
            let _ = self.sections.send(rest);
        }
        drop(self.sections);
        self.worker.await?
    }
}

/// Analysis frame: 30 ms.
const FRAME: usize = SAMPLE_RATE as usize * 30 / 1000;
/// Sections shorter than this aren't cut yet: with short sections Whisper gets more words
/// wrong and makes up text ("Vielen Dank.") at their quiet ends.
const MIN_SECTION: usize = SAMPLE_RATE as usize * 20;
/// Whisper sees at most 30 s at once.
const MAX_SECTION: usize = SAMPLE_RATE as usize * 28;
/// Loud frames a section needs to count as speech, i.e. 300 ms: Whisper makes up text for
/// silence and noise.
const MIN_SPEECH_FRAMES: usize = 300 / 30;
/// Silence that counts as a pause, in frames.
const PAUSE_FRAMES: usize = 400 / 30;

/// Splits audio at pauses. Quiet is measured against the noise floor, which adapts to the
/// microphone: it follows quieter frames at once and louder ones slowly.
#[derive(Default)]
struct Cutter {
    /// Audio of the current section.
    section: Vec<i16>,
    /// Level of each complete frame in `section`.
    levels: Vec<f32>,
    /// Frames since the last loud one.
    quiet_run: usize,
    /// Loud frames in the section.
    speech_frames: usize,
    noise: Option<f32>,
}

impl Cutter {
    /// Takes more audio and returns the sections it completes.
    fn push(&mut self, samples: &[i16]) -> Vec<Vec<i16>> {
        let mut done = Vec::new();
        self.section.extend_from_slice(samples);
        while self.section.len() >= (self.levels.len() + 1) * FRAME {
            let start = self.levels.len() * FRAME;
            let level = rms(&self.section[start..start + FRAME]);
            self.levels.push(level);
            let noise = self.noise.get_or_insert(level);
            *noise = if level < *noise {
                level
            } else {
                *noise + (level - *noise) * 0.002
            };
            if level < *noise * 2.0 {
                self.quiet_run += 1;
            } else {
                self.quiet_run = 0;
                self.speech_frames += 1;
            }
            let end = self.levels.len() * FRAME;
            if end >= MIN_SECTION && self.quiet_run >= PAUSE_FRAMES {
                // Cut in the middle of the pause.
                let cut = end - self.quiet_run / 2 * FRAME;
                done.extend(self.cut(cut));
            } else if end >= MAX_SECTION {
                // No pause: cut at the quietest frame of the last 8 s.
                let from = self.levels.len().saturating_sub(8000 / 30);
                let quietest = (from..self.levels.len())
                    .min_by(|&a, &b| self.levels[a].total_cmp(&self.levels[b]))
                    .unwrap_or(from);
                done.extend(self.cut(quietest * FRAME));
            }
        }
        done
    }

    /// The rest at the end of the recording.
    fn finish(&mut self) -> Option<Vec<i16>> {
        let len = self.section.len();
        self.cut(len)
    }

    /// Ends the section at sample `at` and starts the next with the rest. Returns the
    /// section unless it is only silence: Whisper makes up text for silence.
    fn cut(&mut self, at: usize) -> Option<Vec<i16>> {
        let rest = self.section.split_off(at);
        let section = std::mem::replace(&mut self.section, rest);
        let speech_frames = self.speech_frames;
        // Re-measure the rest on the next push.
        self.levels.clear();
        self.quiet_run = 0;
        self.speech_frames = 0;
        (speech_frames >= MIN_SPEECH_FRAMES).then_some(section)
    }
}

fn rms(frame: &[i16]) -> f32 {
    let sum: f64 = frame.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / frame.len() as f64).sqrt() as f32
}

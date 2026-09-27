//! Speech recognition via Foundry Local (streaming Nemotron ASR).

use std::io::{IsTerminal, Write};
use std::sync::Arc;

use anyhow::{Context, Result};
use foundry_local_sdk::{
    AudioSession, FoundryLocalConfig, FoundryLocalManager, Item, ItemQueue, ItemStream, LogLevel,
    Model, Request, RequestOptions,
};
use tokio_stream::StreamExt;

pub const DEFAULT_MODEL: &str = "nemotron-3.5-asr-streaming-0.6b";

/// PCM format the streaming session expects.
pub const SAMPLE_RATE: u32 = 16_000;

const APP_NAME: &str = "freisprech";

/// A loaded ASR model. Load once at startup, then start one [`Dictation`] per recording.
pub struct AsrEngine {
    _manager: Arc<FoundryLocalManager>,
    model: Arc<Model>,
}

impl AsrEngine {
    /// Downloads the model on first use (reporting progress in percent) and loads it.
    pub async fn load(alias: &str, on_progress: impl FnMut(f64) + Send + 'static) -> Result<Self> {
        let log_level = if tracing::enabled!(tracing::Level::DEBUG) {
            LogLevel::Debug
        } else {
            LogLevel::Warn
        };
        let mut config = FoundryLocalConfig::new(APP_NAME).log_level(log_level);
        if let Some(dir) = native_lib_dir()? {
            config = config.library_path(dir);
        }
        let manager = FoundryLocalManager::create(config)
            .context("Failed to initialize Foundry Local")?;
        let model = manager
            .catalog()
            .get_model(alias)
            .await
            .with_context(|| format!("Model '{alias}' not in catalog"))?;
        tracing::info!(id = model.id(), "Model found");

        if !model.is_cached().await? {
            let mut on_progress = on_progress;
            let mut progress = DownloadProgress::new();
            let terminal = progress.terminal;
            tracing::info!("Downloading model …");
            let result = model
                .download(Some(move |p| {
                    progress.update(p);
                    on_progress(p);
                }))
                .await;
            if terminal {
                eprintln!();
            }
            result?;
            tracing::info!("Model downloaded");
        }
        model.load().await?;

        Ok(Self {
            _manager: manager,
            model,
        })
    }

    /// Opens a streaming transcription. `language` is e.g. `"de"`, `"de-DE"` or `"auto"`;
    /// `None` leaves the engine default (English).
    pub async fn start_dictation(&self, language: Option<&str>) -> Result<Dictation> {
        let session = AudioSession::new(&self.model).await?;
        let queue = session.create_input_queue()?;

        // The first item describes the raw PCM format of everything pushed into the queue.
        let format = Item::audio_data(Vec::new(), Some("pcm"), SAMPLE_RATE as i32, 1);
        let mut request = Request::from_items(vec![format]).with_input_queue(queue.clone());
        if let Some(lang) = language {
            request = request.with_options(RequestOptions {
                additional_options: vec![("language".into(), lang.into())],
                ..Default::default()
            });
        }
        let stream = session.process_streaming_request(request);

        Ok(Dictation {
            _session: session,
            queue,
            stream,
            text: String::new(),
        })
    }
}

/// One running transcription: push audio via [`AudioInput`], read text via [`Dictation::next`].
pub struct Dictation {
    _session: AudioSession,
    queue: ItemQueue,
    stream: ItemStream,
    text: String,
}

impl Dictation {
    /// Handle for the audio pump; can be moved to another task.
    pub fn input(&self) -> AudioInput {
        AudioInput(self.queue.clone())
    }

    /// Next piece of recognized text, or `None` once the session has ended.
    ///
    /// Nemotron emits append-only token text: once returned, a piece is never revised.
    pub async fn next(&mut self) -> Option<Result<String>> {
        loop {
            let item = match self.stream.next().await? {
                Ok(item) => item,
                Err(e) => return Some(Err(e.into())),
            };
            tracing::debug!(?item, "ASR-Item");
            let piece = match item {
                Item::SpeechSegment(seg) => seg.text,
                Item::Text { text, .. } => text,
                _ => continue,
            };
            if piece.is_empty() || is_language_tag(&piece) {
                continue;
            }
            self.text.push_str(&piece);
            return Some(Ok(piece));
        }
    }

    /// Signals end of audio; [`next`](Self::next) then yields the remaining text and ends.
    pub fn end_audio(&self) {
        self.queue.mark_finished();
    }

    /// Signals end of audio, waits for the remaining text and returns the full transcript.
    pub async fn finish(mut self) -> Result<String> {
        self.end_audio();
        while let Some(result) = self.next().await {
            result?;
        }
        // Removed language tags leave double spaces behind.
        Ok(self.text.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

/// With `language = "auto"` the model emits the detected locale as a token, e.g. `" <de-DE>"`.
fn is_language_tag(piece: &str) -> bool {
    let t = piece.trim();
    t.len() > 2 && t.starts_with('<') && t.ends_with('>')
}

/// Download progress on the command line, like the Foundry Local samples. Without a
/// terminal (started from the desktop), every 10 % goes to the log instead.
struct DownloadProgress {
    terminal: bool,
    /// Last reported value: tenths of a percent on a terminal, else tens.
    shown: Option<u32>,
}

impl DownloadProgress {
    const BAR_WIDTH: usize = 30;

    fn new() -> Self {
        Self {
            terminal: std::io::stderr().is_terminal(),
            shown: None,
        }
    }

    fn update(&mut self, percent: f64) {
        let percent = percent.clamp(0.0, 100.0);
        let step = if self.terminal { percent * 10.0 } else { percent / 10.0 } as u32;
        if self.shown.is_some_and(|shown| step <= shown) {
            return;
        }
        self.shown = Some(step);
        if self.terminal {
            let filled = (percent / 100.0 * Self::BAR_WIDTH as f64) as usize;
            eprint!(
                "\r  Model download [{}{}] {percent:5.1} %",
                "#".repeat(filled),
                "-".repeat(Self::BAR_WIDTH - filled)
            );
            let _ = std::io::stderr().flush();
        } else {
            tracing::info!("Model download: {} %", step * 10);
        }
    }
}

/// Write end of a [`Dictation`]: accepts 16 kHz mono PCM samples.
#[derive(Clone)]
pub struct AudioInput(ItemQueue);

impl AudioInput {
    pub fn push(&self, samples: &[i16]) -> Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.0.push(&Item::bytes(bytes))?;
        Ok(())
    }
}

/// Where the native Foundry engine lives, unless `FOUNDRY_LOCAL_LIB_DIR` says otherwise:
/// `native/` next to the executable, else the copy embedded with `bundle-native`, else the
/// project's `native/` (development). Needed because autostart runs the binary directly,
/// without cargo's environment.
fn native_lib_dir() -> Result<Option<String>> {
    if std::env::var_os("FOUNDRY_LOCAL_LIB_DIR").is_some() {
        return Ok(None);
    }
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("native")))
        .filter(|dir| dir.is_dir());
    #[cfg(feature = "bundle-native")]
    let fallback = match beside_exe {
        Some(_) => None,
        None => Some(crate::install::unpack_native()?),
    };
    #[cfg(not(feature = "bundle-native"))]
    let fallback = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("native"));
    Ok(beside_exe
        .into_iter()
        .chain(fallback)
        .find(|dir| dir.is_dir())
        .map(|dir| dir.to_string_lossy().into_owned()))
}

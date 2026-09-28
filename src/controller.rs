//! The dictation loop: shortcut -> microphone -> ASR -> live typing.
//! Runs on its own thread with its own tokio runtime, next to the UI thread.

use std::io::Write;
use std::time::Duration;

use anyhow::{bail, Result};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::asr::{self, AsrEngine, Dictation};
use crate::audio;
use crate::config::Config;
use crate::hotkey::{Hotkey, HotkeyEvent};
use crate::language;
use crate::typer::Typer;

#[cfg(target_os = "linux")]
pub const APP_ID: &str = "io.github.trapplab.Freisprech";

/// Longest wait for the user to let go of the shortcut before typing anyway.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);
/// Longest wait for the controller to release the engine when quitting.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Starting,
    Downloading(f64),
    LoadingModel,
    /// `shortcut`: whether the global shortcut is bound, else only `--toggle` works.
    Ready { shortcut: bool },
    Recording,
    /// The last dictation failed; the next one may still work.
    Error(String),
    /// Setup failed; nothing works until restart.
    Fatal(String),
}

impl Status {
    pub fn describe(&self) -> String {
        match self {
            Status::Starting => "Starting …".into(),
            Status::Downloading(p) => format!("Downloading model … {p:.0} %"),
            Status::LoadingModel => "Loading model …".into(),
            Status::Ready { shortcut: true } => "Ready – Ctrl+Alt+D starts dictation".into(),
            Status::Ready { shortcut: false } => {
                "Ready – no global shortcut, see settings".into()
            }
            Status::Recording => "Recording – press the shortcut again to stop".into(),
            Status::Error(msg) => format!("Last dictation failed: {msg}"),
            Status::Fatal(msg) => format!("Error: {msg}"),
        }
    }
}

/// Handle to the controller thread.
pub struct Controller {
    shutdown: oneshot::Sender<()>,
    done: std::sync::mpsc::Receiver<()>,
}

impl Controller {
    /// Stops the controller and waits until the ASR engine is released. The native
    /// Foundry engine must be torn down before the process exits: left to the library's
    /// static destructors, its telemetry shutdown crashes.
    pub fn shutdown(self) {
        let _ = self.shutdown.send(());
        if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
            self.done.recv_timeout(SHUTDOWN_TIMEOUT)
        {
            tracing::warn!("Controller did not stop in time");
        }
    }
}

/// Starts the controller thread. Settings are read at the start of each dictation;
/// `toggles` start/stop dictations like the shortcut.
pub fn spawn(
    config: watch::Receiver<Config>,
    toggles: mpsc::UnboundedReceiver<()>,
    status: mpsc::UnboundedSender<Status>,
) -> Controller {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("controller".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = status.send(Status::Fatal(err.to_string()));
                    return;
                }
            };
            // Returning from `run` early drops the engine, the portal sessions and any
            // running dictation.
            let result = runtime.block_on(async {
                tokio::select! {
                    _ = shutdown_rx => Ok(()),
                    result = run(config, toggles, status.clone()) => result,
                }
            });
            if let Err(err) = result {
                tracing::error!("{err:#}");
                let _ = status.send(Status::Fatal(format!("{err:#}")));
            }
            // Blocking ASR calls may still hold the engine; give them a moment to finish.
            runtime.shutdown_timeout(Duration::from_secs(2));
            let _ = done_tx.send(());
        });
    if let Err(err) = spawned {
        tracing::error!(%err, "Failed to start controller thread");
    }
    Controller {
        shutdown: shutdown_tx,
        done: done_rx,
    }
}

async fn run(
    config: watch::Receiver<Config>,
    toggles: mpsc::UnboundedReceiver<()>,
    status: mpsc::UnboundedSender<Status>,
) -> Result<()> {
    let _ = status.send(Status::Starting);

    #[cfg(target_os = "linux")]
    crate::install::register_app().await?;
    let mut typer = Typer::connect().await?;
    let mut hotkey = Hotkey::bind(toggles).await;
    let ready = Status::Ready {
        shortcut: hotkey.has_shortcut(),
    };

    let _ = status.send(Status::LoadingModel);
    let progress = status.clone();
    let engine = AsrEngine::load(asr::DEFAULT_MODEL, move |p| {
        let _ = progress.send(Status::Downloading(p));
    })
    .await?;
    let _ = status.send(ready.clone());

    loop {
        match hotkey.next().await {
            Some(HotkeyEvent::Pressed) => {
                let settings = config.borrow().clone();
                let _ = status.send(Status::Recording);
                match dictate(&engine, &settings, &mut typer, &mut hotkey).await {
                    Ok(()) => {
                        let _ = status.send(ready.clone());
                    }
                    Err(err) => {
                        tracing::error!("Dictation failed: {err:#}");
                        let _ = status.send(Status::Error(format!("{err:#}")));
                    }
                }
            }
            Some(HotkeyEvent::Released) => {}
            None => bail!("Lost connection to the shortcut"),
        }
    }
}

/// One dictation from shortcut press to shortcut press, typing text as it is recognized.
async fn dictate(
    engine: &AsrEngine,
    settings: &Config,
    typer: &mut Typer,
    hotkey: &mut Hotkey,
) -> Result<()> {
    let language = language::resolve(&settings.language);
    tracing::info!(setting = settings.language, language, "Dictation started");
    let mut dictation = engine.start_dictation(Some(&language)).await?;
    let (mic, pump) = start_microphone(&dictation, settings.microphone.as_deref())?;
    let mut out = LiveOutput::new(typer);

    // Recording: until the shortcut is pressed again.
    loop {
        tokio::select! {
            event = hotkey.next() => match event {
                Some(HotkeyEvent::Pressed) => break,
                Some(HotkeyEvent::Released) => out.release().await?,
                None => bail!("Lost connection to the shortcut"),
            },
            _ = out.hold_expired() => out.release().await?,
            piece = dictation.next() => match piece {
                Some(piece) => out.push(&piece?).await?,
                None => break,
            },
        }
    }

    // Stopping: close the microphone, then type what the model still has buffered.
    out.hold();
    mic.stop();
    pump.await?;
    dictation.end_audio();
    loop {
        tokio::select! {
            event = hotkey.next() => if event == Some(HotkeyEvent::Released) { out.release().await? },
            _ = out.hold_expired() => out.release().await?,
            piece = dictation.next() => match piece {
                Some(piece) => out.push(&piece?).await?,
                None => break,
            },
        }
    }
    out.wait_and_flush().await?;
    tracing::info!("Dictation stopped");
    Ok(())
}

/// Types recognized text live, but holds it back while the shortcut keys are still down:
/// otherwise the held Ctrl+Alt would turn the typed letters into shortcuts.
struct LiveOutput<'a> {
    typer: &'a mut Typer,
    pending: String,
    held_until: Option<Instant>,
    at_start: bool,
}

impl<'a> LiveOutput<'a> {
    fn new(typer: &'a mut Typer) -> Self {
        let mut out = Self {
            typer,
            pending: String::new(),
            held_until: None,
            at_start: true,
        };
        out.hold();
        out
    }

    fn hold(&mut self) {
        self.held_until = Some(Instant::now() + RELEASE_TIMEOUT);
    }

    async fn release(&mut self) -> Result<()> {
        self.held_until = None;
        self.flush().await
    }

    /// Resolves when the hold times out; never while nothing is held.
    async fn hold_expired(&self) {
        match self.held_until {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending().await,
        }
    }

    async fn push(&mut self, piece: &str) -> Result<()> {
        eprint!("{piece}");
        let _ = std::io::stderr().flush();
        self.pending.push_str(piece);
        if self.held_until.is_none() {
            self.flush().await?;
        }
        Ok(())
    }

    async fn wait_and_flush(mut self) -> Result<()> {
        if self.held_until.is_some() {
            self.hold_expired().await;
        }
        self.release().await
    }

    async fn flush(&mut self) -> Result<()> {
        // The model starts words with a space; don't put one at the start of the field.
        let text = if self.at_start {
            self.pending.trim_start()
        } else {
            self.pending.as_str()
        };
        if !text.is_empty() {
            self.typer.type_text(text).await?;
            self.at_start = false;
        }
        self.pending.clear();
        Ok(())
    }
}

/// Opens the microphone and forwards its audio into the dictation.
/// Stopping or dropping the returned microphone ends the pump task.
fn start_microphone(
    dictation: &Dictation,
    device: Option<&str>,
) -> Result<(audio::Microphone, JoinHandle<()>)> {
    let input = dictation.input();
    let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<i16>>(32);
    let pump = tokio::spawn(async move {
        while let Some(chunk) = pcm_rx.recv().await {
            if let Err(err) = input.push(&chunk) {
                tracing::error!(%err, "Failed to pass audio to the ASR");
                break;
            }
        }
    });
    let mic = audio::Microphone::start(device, pcm_tx)?;
    Ok((mic, pump))
}

mod asr;
mod audio;
mod config;
mod controller;
mod hotkey;
mod install;
#[cfg(target_os = "linux")]
mod ipc;
mod language;
mod logging;
mod polish;
mod rewrite;
mod tray;
mod typer;
mod ui;
mod update;
mod whisper;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::{mpsc, watch};

use asr::AsrEngine;
use config::Config;
use rewrite::Rewriter;
use typer::Typer;

/// Local dictation assistant: Ctrl+Alt+D starts/stops, speech is typed live.
/// Without options it runs in the background with a tray icon.
#[derive(Parser)]
struct Cli {
    /// Open the settings window on start
    #[arg(long)]
    settings: bool,

    /// Start/stop dictation in the running instance – for a custom shortcut on
    /// desktops without the GlobalShortcuts portal (GNOME before 48)
    #[cfg(target_os = "linux")]
    #[arg(long)]
    toggle: bool,

    /// Remove the installed copy, its menu entry and autostart (model, settings and logs
    /// stay)
    #[arg(long)]
    uninstall: bool,

    /// Test: only transcribe an audio file and print the text (raw PCM, 16 kHz, mono, s16le;
    /// e.g. `ffmpeg -i in.wav -ar 16000 -ac 1 -f s16le in.pcm`)
    #[arg(long)]
    file: Option<PathBuf>,

    /// With --file: feed the file in real time instead of as fast as possible
    #[arg(long)]
    realtime: bool,

    /// Test: correct TEXT with a local LLM (experimental) and print both versions. Without
    /// TEXT, with --file or --record: correct the transcript
    #[arg(long, value_name = "TEXT", num_args = 0..=1)]
    polish: Option<Option<String>>,

    /// With --file: also transcribe with Whisper (experimental), in sections as the audio comes
    /// in, and print both versions. MODEL is e.g. `whisper-small`
    #[arg(long, value_name = "MODEL", num_args = 0..=1, default_missing_value = whisper::DEFAULT_MODEL)]
    whisper: Option<String>,

    /// Test: dictate from the microphone until Enter, print the text instead of typing it,
    /// and save the audio as the model received it (format as for --file)
    #[arg(long, value_name = "FILE")]
    record: Option<PathBuf>,

    /// With --file or --record: language instead of the setting, e.g. `en`, `de-DE`,
    /// `auto` (detect from speech) or `system` (system language)
    #[arg(long, value_name = "CODE")]
    language: Option<String>,

    /// Test: only type this text, without speech recognition
    #[arg(long, value_name = "TEXT")]
    type_text: Option<String>,

    /// With --type-text: seconds to wait, to switch to another window
    #[arg(long, default_value_t = 3)]
    type_delay: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    #[cfg(target_os = "linux")]
    if cli.toggle {
        anyhow::ensure!(ipc::send(ipc::Command::Toggle), "Freisprech is not running");
        return Ok(());
    }
    let _log_guard = logging::init()?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), log_dir = %logging::log_dir().display(), "Start");

    let result = if cli.uninstall {
        uninstall()
    } else if cli.file.is_some()
        || cli.record.is_some()
        || cli.polish.is_some()
        || cli.type_text.is_some()
    {
        tokio::runtime::Runtime::new()?.block_on(run_test_mode(&cli))
    } else {
        run_app(cli.settings)
    };
    if let Err(err) = &result {
        tracing::error!("{err:#}");
    }
    result
}

/// Normal operation: controller thread in the background, tray + settings on the main thread.
fn run_app(show_settings: bool) -> Result<()> {
    let (toggle_tx, toggle_rx) = mpsc::unbounded_channel();
    let (settings_tx, settings_rx) = mpsc::unbounded_channel();
    #[cfg(target_os = "linux")]
    {
        if ipc::send(ipc::Command::Settings) {
            tracing::info!("Already running – opened its settings");
            return Ok(());
        }
        if let Err(err) = ipc::listen(toggle_tx, settings_tx) {
            tracing::warn!("Control via --toggle not available: {err:#}");
        }
    }
    #[cfg(not(target_os = "linux"))]
    drop((toggle_tx, settings_tx));

    let config = Config::load();
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (status_tx, status_rx) = mpsc::unbounded_channel();
    let controller = controller::spawn(config_rx, toggle_rx, status_tx);
    let result = ui::run(config, config_tx, status_rx, settings_rx, show_settings);
    controller.shutdown();
    install::spawn_after_exit();
    Ok(result?)
}

fn uninstall() -> Result<()> {
    #[cfg(target_os = "linux")]
    anyhow::ensure!(
        !ipc::send(ipc::Command::Settings),
        "Freisprech is running – use Uninstall in the settings window that just opened"
    );
    #[cfg(windows)]
    install::stop_other_instances();
    install::uninstall()?;
    install::spawn_after_exit();
    println!("Freisprech uninstalled. Model, settings and logs were kept.");
    Ok(())
}

async fn run_test_mode(cli: &Cli) -> Result<()> {
    let config = Config::load();
    let language = language::resolve(cli.language.as_deref().unwrap_or(&config.language));
    let polisher = match cli.polish {
        Some(_) => Some(polish::Polisher::load(polish::DEFAULT_MODEL, |_| {}).await?),
        None => None,
    };
    if cli.file.is_some() || cli.record.is_some() {
        tracing::info!(language, "Language");
        let engine = AsrEngine::load(asr::DEFAULT_MODEL, |_| {}).await?;
        let whisper = match &cli.whisper {
            Some(model) => Some(Arc::new(whisper::Whisper::load(model, |_| {}).await?)),
            None => None,
        };
        let rewriter = || Rewriter::new(&language, &config);
        if let Some(path) = &cli.file {
            let samples = read_pcm(path)?;
            let mut sections = whisper.map(|w| whisper::Sections::start(w, &language));
            let text = transcribe_file(&engine, &language, &samples, cli.realtime, |chunk| {
                if let Some(sections) = &mut sections {
                    sections.push(chunk);
                }
            })
            .await?;
            let whispered = match sections {
                Some(sections) => {
                    let start = std::time::Instant::now();
                    let text = sections.finish().await?;
                    Some((text, start.elapsed()))
                }
                None => None,
            };
            print_text(&rewriter().apply(&text), polisher.as_ref()).await?;
            if let Some((text, waited)) = whispered {
                println!("Whisper: {}", rewriter().apply(&text));
                println!("({} ms after the end of the audio)", waited.as_millis());
            }
        }
        if let Some(path) = &cli.record {
            let microphone = config.microphone.as_deref();
            let text = record(&engine, &language, microphone, path).await?;
            print_text(&rewriter().apply(&text), polisher.as_ref()).await?;
        }
    }
    if let Some(Some(text)) = &cli.polish {
        print_text(text, polisher.as_ref()).await?;
    }
    if let Some(text) = &cli.type_text {
        #[cfg(target_os = "linux")]
        install::register_app().await?;
        let mut typer = Typer::connect().await?;
        countdown(cli.type_delay).await;
        typer.type_text(text).await?;
    }
    Ok(())
}

/// Prints the transcript, and with a polisher also its corrected version.
async fn print_text(text: &str, polisher: Option<&polish::Polisher>) -> Result<()> {
    let Some(polisher) = polisher else {
        println!("{text}");
        return Ok(());
    };
    let start = std::time::Instant::now();
    let polished = polisher.polish(text).await?;
    println!("Original: {text}");
    println!("Polished: {polished}");
    println!("({} ms)", start.elapsed().as_millis());
    Ok(())
}

/// Reads raw 16 kHz mono s16le audio, see `--file`.
fn read_pcm(path: &Path) -> Result<Vec<i16>> {
    let bytes = std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))?;
    Ok(bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect())
}

/// Transcribes audio as if it came from the microphone, also passing it to `on_chunk`.
async fn transcribe_file(
    engine: &AsrEngine,
    language: &str,
    samples: &[i16],
    realtime: bool,
    mut on_chunk: impl FnMut(&[i16]),
) -> Result<String> {
    let dictation = engine.start_dictation(Some(language)).await?;
    let input = dictation.input();
    for chunk in samples.chunks(asr::SAMPLE_RATE as usize / 10) {
        input.push(chunk)?;
        on_chunk(chunk);
        if realtime {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    dictation.finish().await
}

/// Dictates from the microphone until Enter, showing the text as it is recognized, and
/// saves the audio that went into the model.
async fn record(
    engine: &AsrEngine,
    language: &str,
    microphone: Option<&str>,
    path: &Path,
) -> Result<String> {
    let file = File::create(path).with_context(|| format!("Cannot create {}", path.display()))?;
    let mut file = BufWriter::new(file);
    let mut dictation = engine.start_dictation(Some(language)).await?;
    let input = dictation.input();
    let (pcm_tx, mut pcm_rx) = mpsc::channel::<Vec<i16>>(32);
    let pump = tokio::spawn(async move {
        while let Some(chunk) = pcm_rx.recv().await {
            input.push(&chunk)?;
            let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
            file.write_all(&bytes)?;
        }
        file.flush()?;
        anyhow::Ok(())
    });
    let mic = audio::Microphone::start(microphone, pcm_tx)?;

    eprintln!("Recording – press Enter to stop");
    let mut enter = tokio::task::spawn_blocking(|| std::io::stdin().read_line(&mut String::new()));
    loop {
        tokio::select! {
            _ = &mut enter => break,
            piece = dictation.next() => match piece {
                Some(piece) => {
                    eprint!("{}", piece?);
                    let _ = std::io::stderr().flush();
                }
                None => break,
            },
        }
    }
    mic.stop();
    pump.await??;
    let text = dictation.finish().await?;
    eprintln!("\nSaved to {}", path.display());
    Ok(text)
}

async fn countdown(seconds: u64) {
    for s in (1..=seconds).rev() {
        eprint!("\rTyping in {s} s – click into the target window now … ");
        let _ = std::io::stderr().flush();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    eprintln!("\rTyping …{:40}", "");
}

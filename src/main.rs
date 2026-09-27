mod asr;
mod audio;
mod config;
mod controller;
mod hotkey;
mod install;
#[cfg(target_os = "linux")]
mod ipc;
mod logging;
mod tray;
mod typer;
mod ui;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use tokio::sync::{mpsc, watch};

use asr::AsrEngine;
use config::Config;
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
    } else if cli.file.is_some() || cli.type_text.is_some() {
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
    let language = Config::load().language;
    if let Some(path) = &cli.file {
        let engine = AsrEngine::load(asr::DEFAULT_MODEL, |_| {}).await?;
        println!("{}", transcribe_file(&engine, &language, path, cli.realtime).await?);
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

async fn transcribe_file(
    engine: &AsrEngine,
    language: &str,
    path: &Path,
    realtime: bool,
) -> Result<String> {
    let dictation = engine.start_dictation(Some(language)).await?;
    let input = dictation.input();
    let bytes = std::fs::read(path)?;
    let samples: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    for chunk in samples.chunks(asr::SAMPLE_RATE as usize / 10) {
        input.push(chunk)?;
        if realtime {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    dictation.finish().await
}

async fn countdown(seconds: u64) {
    for s in (1..=seconds).rev() {
        eprint!("\rTyping in {s} s – click into the target window now … ");
        let _ = std::io::stderr().flush();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    eprintln!("\rTyping …{:40}", "");
}

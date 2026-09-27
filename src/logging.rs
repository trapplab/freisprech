//! Logs to stderr and to a daily rotating file, so autostarted runs (no terminal) can be
//! inspected. Recognized text itself is never logged.

use std::path::PathBuf;

use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Linux: `~/.local/share/freisprech/logs`, Windows: `%LOCALAPPDATA%\freisprech\logs`.
pub fn log_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_default()
        .join("freisprech")
        .join("logs")
}

/// Keep the returned guard alive until exit; dropping it flushes the file.
pub fn init() -> Result<WorkerGuard> {
    let file = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("freisprech")
        .filename_suffix("log")
        .max_log_files(7)
        .build(log_dir())?;
    let (file_writer, guard) = tracing_appender::non_blocking(file);

    tracing_subscriber::registry()
        // zbus warns about every portal proxy without properties, and iced_winit dumps all
        // window attributes on every open; both are expected noise.
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,zbus=error,iced_winit=warn".into()),
        )
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(fmt::layer().with_ansi(false).with_writer(file_writer))
        .init();
    Ok(guard)
}

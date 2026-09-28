//! Linux: the menu entry is a desktop entry in `~/.local/share/applications`, autostart
//! a copy of it in `~/.config/autostart`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use super::ignore_missing;
use crate::controller::APP_ID;

pub fn install_path() -> Result<PathBuf> {
    Ok(dirs::executable_dir()
        .context("No folder for executables")?
        .join("freisprech"))
}

/// Registers our app ID with the portals. Must precede any other portal call so
/// permissions are stored under it.
pub async fn register_app() -> Result<()> {
    const ATTEMPTS: u32 = 20;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

    // The portals only accept an app ID backed by a desktop entry whose `Exec` exists.
    // Point it at the installed copy if there is one, else at this executable, wherever
    // it currently lives.
    let exe = super::install_path()
        .ok()
        .filter(|path| path.is_file())
        .map_or_else(std::env::current_exe, Ok)?;
    if let Err(err) = add_menu_entry(&exe, env!("CARGO_PKG_VERSION")) {
        tracing::warn!("Failed to write desktop entry: {err:#}");
    }
    // The portal picks up a newly written desktop entry only after a moment; until then
    // it rejects the app ID with "App info not found".
    let mut attempt = 1;
    loop {
        match ashpd::register_host_app(APP_ID.try_into()?).await {
            Ok(()) => return Ok(()),
            Err(err) if attempt < ATTEMPTS => {
                tracing::debug!(%err, attempt, "Portal does not know the app ID yet, retrying");
                tokio::time::sleep(RETRY_DELAY).await;
                attempt += 1;
            }
            Err(err) => return Err(err).context("Failed to register app ID with the portal"),
        }
    }
}

/// Launching the entry from the app menu opens the settings (of the running instance,
/// if there is one). The version is only shown on Windows.
pub fn add_menu_entry(exe: &Path, _version: &str) -> Result<()> {
    write_if_changed(&menu_entry_path()?, &desktop_entry(exe, &["--settings"]))
}

pub fn remove_menu_entry() -> Result<()> {
    Ok(ignore_missing(std::fs::remove_file(menu_entry_path()?))?)
}

pub fn autostart_enabled() -> bool {
    autostart_path().is_ok_and(|path| path.is_file())
}

/// Autostart runs without `--settings`, so the app just starts in the tray.
pub fn add_autostart(exe: &Path) -> Result<()> {
    write_if_changed(&autostart_path()?, &desktop_entry(exe, &[]))
}

pub fn remove_autostart() -> Result<()> {
    Ok(ignore_missing(std::fs::remove_file(autostart_path()?))?)
}

/// A running executable and loaded libraries can be deleted right away.
pub fn remove_files(exe: &Path, native: &Path) -> Result<()> {
    ignore_missing(std::fs::remove_file(exe))?;
    ignore_missing(std::fs::remove_dir_all(native))?;
    Ok(())
}

/// Waits until this process is gone (at most 30 s), so the new one does not find it
/// still running: shutting down can take a while.
pub fn delayed_start(exe: &Path) -> Command {
    const SCRIPT: &str = r#"i=0
while kill -0 "$1" 2>/dev/null && [ $i -lt 150 ]; do sleep 0.2; i=$((i + 1)); done
exec "$0" --settings"#;
    let mut command = Command::new("sh");
    command
        .args(["-c", SCRIPT])
        .arg(exe)
        .arg(std::process::id().to_string());
    command
}

fn menu_entry_path() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("No data directory")?
        .join("applications")
        .join(format!("{APP_ID}.desktop")))
}

fn autostart_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("No config directory")?
        .join("autostart")
        .join(format!("{APP_ID}.desktop")))
}

/// Our desktop entry, with `Exec` running `exe` with `args`.
fn desktop_entry(exe: &Path, args: &[&str]) -> String {
    let exe = exe.display().to_string();
    let exe = if exe.contains(char::is_whitespace) {
        format!("\"{exe}\"")
    } else {
        exe
    };
    let exec: Vec<&str> = std::iter::once(exe.as_str()).chain(args.iter().copied()).collect();
    let exec = format!("Exec={}", exec.join(" "));
    include_str!("../../assets/io.github.trapplab.Freisprech.desktop")
        .lines()
        .map(|line| if line.starts_with("Exec=") { exec.as_str() } else { line })
        .flat_map(|line| [line, "\n"])
        .collect()
}

fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, content)?;
    tracing::info!(path = %path.display(), "Desktop entry written");
    Ok(())
}

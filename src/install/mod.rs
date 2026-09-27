//! Makes the executable self-sufficient: it registers with the desktop (Linux) and, when
//! built with `--features bundle-native`, unpacks the native Foundry engine embedded into
//! it. On request it installs itself to a fixed location with a menu entry and autostart.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "linux")]
pub use linux::register_app;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;
#[cfg(windows)]
pub use windows::stop_other_instances;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::{Context, Result};

/// Started once the app has shut down, e.g. the copy just installed.
static AFTER_EXIT: Mutex<Option<Command>> = Mutex::new(None);

/// What the settings window shows about the installation.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// Folder of the running executable.
    pub running_from: PathBuf,
    /// The running executable is the installed copy.
    pub running_installed: bool,
    /// An installed copy exists, so autostart has something to start.
    pub can_autostart: bool,
    pub autostart: bool,
}

impl State {
    pub fn read() -> Self {
        let exe = std::env::current_exe().unwrap_or_default();
        let target = install_path().ok();
        Self {
            running_from: exe.parent().map(Path::to_path_buf).unwrap_or_default(),
            running_installed: target.as_deref().is_some_and(|target| same_file(&exe, target)),
            can_autostart: target.is_some_and(|target| target.is_file()),
            autostart: platform::autostart_enabled(),
        }
    }
}

/// Linux: `~/.local/bin/freisprech`, Windows:
/// `%LOCALAPPDATA%\Programs\Freisprech\freisprech.exe`.
pub fn install_path() -> Result<PathBuf> {
    platform::install_path()
}

/// Copies the running executable to [`install_path`] and adds the menu entry. An installed
/// copy is replaced, so this also updates. Returns the installed path.
pub fn install() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let target = install_path()?;
    if !same_file(&exe, &target) {
        std::fs::create_dir_all(target.parent().context("Install path has no folder")?)?;
        // Copy beside and rename, so an interrupted copy never leaves a broken app.
        let tmp = target.with_extension("part");
        std::fs::copy(&exe, &tmp)
            .with_context(|| format!("Failed to copy the app to {}", tmp.display()))?;
        std::fs::rename(&tmp, &target)
            .with_context(|| format!("Failed to replace {}", target.display()))?;
    }
    platform::add_menu_entry(&target)?;
    tracing::info!(path = %target.display(), "Installed");
    Ok(target)
}

/// Removes the installed copy, its menu entry, autostart and the unpacked native engine.
/// Model, settings and logs stay. On Windows the files go once the app has exited.
pub fn uninstall() -> Result<()> {
    set_autostart(false)?;
    platform::remove_menu_entry()?;
    platform::remove_files(&install_path()?, &native_root()?)?;
    tracing::info!("Uninstalled");
    Ok(())
}

/// Autostart always runs the installed copy, never the one running now.
pub fn set_autostart(enabled: bool) -> Result<()> {
    if enabled {
        platform::add_autostart(&install_path()?)?;
    } else {
        platform::remove_autostart()?;
    }
    tracing::info!(enabled, "Autostart changed");
    Ok(())
}

/// Starts `exe --settings` once this process has exited.
pub fn relaunch_after_exit(exe: &Path) {
    run_after_exit(platform::delayed_start(exe));
}

/// Call right before the process exits. The command waits for the exit itself, so it runs
/// after the exit, when the shortcut and open files are released.
pub fn spawn_after_exit() {
    let command = AFTER_EXIT.lock().ok().and_then(|mut command| command.take());
    if let Some(mut command) = command {
        if let Err(err) = command.spawn() {
            tracing::error!(%err, "Failed to run command after exit");
        }
    }
}

fn run_after_exit(command: Command) {
    if let Ok(mut slot) = AFTER_EXIT.lock() {
        *slot = Some(command);
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Treats a file that is already gone as removed.
fn ignore_missing(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Linux: `~/.local/share/freisprech/native`, Windows: `%LOCALAPPDATA%\freisprech\native`.
fn native_root() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .context("No data directory")?
        .join("freisprech")
        .join("native"))
}

#[cfg(feature = "bundle-native")]
mod bundled {
    include!(concat!(env!("OUT_DIR"), "/bundled_native.rs"));
}

/// Unpacks the embedded native engine into the data directory, once per build of it,
/// and removes older copies.
#[cfg(feature = "bundle-native")]
pub fn unpack_native() -> Result<PathBuf> {
    use std::fs;

    let root = native_root()?;
    let dir = root.join(bundled::VERSION);
    if dir.is_dir() {
        return Ok(dir);
    }

    tracing::info!(dir = %dir.display(), "Unpacking native libraries");
    // Unpack beside and rename, so an interrupted start never leaves a partial copy.
    let tmp = root.join(format!("{}.tmp", bundled::VERSION));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    for (name, compressed) in bundled::FILES {
        fs::write(tmp.join(name), zstd::decode_all(*compressed)?)?;
    }
    fs::rename(&tmp, &dir)?;

    for old in fs::read_dir(&root)?.flatten() {
        if old.path() != dir {
            let _ = fs::remove_dir_all(old.path());
        }
    }
    Ok(dir)
}

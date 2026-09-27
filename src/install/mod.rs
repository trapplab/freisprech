//! Makes the executable self-sufficient: it writes the desktop entry the XDG portals need
//! (Linux) and, when built with `--features bundle-native`, unpacks the native Foundry
//! engine embedded into it.

#[cfg(target_os = "linux")]
use anyhow::Context;
#[cfg(any(target_os = "linux", feature = "bundle-native"))]
use anyhow::Result;

/// Registers our app ID with the portals. Must precede any other portal call so
/// permissions are stored under it.
#[cfg(target_os = "linux")]
pub async fn register_app() -> Result<()> {
    const ATTEMPTS: u32 = 20;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

    if let Err(err) = write_desktop_entry() {
        tracing::warn!("Failed to write desktop entry: {err:#}");
    }
    // The portal picks up a newly written desktop entry only after a moment; until then
    // it rejects the app ID with "App info not found".
    let mut attempt = 1;
    loop {
        match ashpd::register_host_app(crate::controller::APP_ID.try_into()?).await {
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

/// The portals only accept an app ID backed by a desktop entry whose `Exec` exists, so
/// point it at this executable, wherever it currently lives. Launching the entry from the
/// app menu opens the settings (of the running instance, if there is one).
#[cfg(target_os = "linux")]
fn write_desktop_entry() -> Result<()> {
    use crate::controller::APP_ID;

    let exe = std::env::current_exe()?.display().to_string();
    let exec = if exe.contains(char::is_whitespace) {
        format!("Exec=\"{exe}\" --settings")
    } else {
        format!("Exec={exe} --settings")
    };
    let entry: String = include_str!("../assets/io.github.trapplab.Freisprech.desktop")
        .lines()
        .map(|line| if line.starts_with("Exec=") { exec.as_str() } else { line })
        .flat_map(|line| [line, "\n"])
        .collect();

    let path = dirs::data_dir()
        .context("No data directory")?
        .join("applications")
        .join(format!("{APP_ID}.desktop"));
    if std::fs::read_to_string(&path).is_ok_and(|old| old == entry) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, entry)?;
    tracing::info!(path = %path.display(), "Desktop entry written");
    Ok(())
}

#[cfg(feature = "bundle-native")]
mod bundled {
    include!(concat!(env!("OUT_DIR"), "/bundled_native.rs"));
}

/// Unpacks the embedded native engine into the data directory, once per build of it,
/// and removes older copies.
#[cfg(feature = "bundle-native")]
pub fn unpack_native() -> Result<std::path::PathBuf> {
    use std::fs;

    let root = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("No data directory"))?
        .join("freisprech")
        .join("native");
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

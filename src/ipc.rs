//! Single instance and remote control via a Unix socket in the runtime directory:
//! starting the app again opens the settings of the running one, and
//! `freisprech --toggle` starts/stops a dictation. The latter is for desktops
//! without the GlobalShortcuts portal (GNOME before 48), where a custom shortcut runs it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use anyhow::Result;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Toggle,
    Settings,
}

impl Command {
    fn as_str(self) -> &'static str {
        match self {
            Command::Toggle => "toggle",
            Command::Settings => "settings",
        }
    }

    fn parse(line: &str) -> Option<Self> {
        match line {
            "toggle" => Some(Command::Toggle),
            "settings" => Some(Command::Settings),
            _ => None,
        }
    }
}

fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("freisprech.sock")
}

/// Sends a command to the running instance; `false` if none is running.
pub fn send(command: Command) -> bool {
    UnixStream::connect(socket_path())
        .and_then(|mut stream| writeln!(stream, "{}", command.as_str()))
        .is_ok()
}

/// Receives commands from other instances. Call only after [`send`] found none running.
pub fn listen(
    toggle: mpsc::UnboundedSender<()>,
    settings: mpsc::UnboundedSender<()>,
) -> Result<()> {
    let path = socket_path();
    // Left behind by an instance that crashed; `send` just showed nobody listens on it.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::thread::Builder::new().name("ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_err() {
                continue;
            }
            match Command::parse(line.trim()) {
                Some(Command::Toggle) => drop(toggle.send(())),
                Some(Command::Settings) => drop(settings.send(())),
                None => tracing::warn!(line, "Unknown command on the socket"),
            }
        }
    })?;
    Ok(())
}

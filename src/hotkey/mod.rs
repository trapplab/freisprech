//! Global Ctrl+Alt+D shortcut that starts and stops a dictation.

#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
use portal::Shortcut;

#[cfg(windows)]
mod win32;
#[cfg(windows)]
use win32::Shortcut;

use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    /// All keys of the shortcut are up again; typing is safe from here on.
    Released,
}

/// What starts and stops dictations: the global shortcut if the desktop offers one,
/// plus `--toggle` commands from other instances.
pub struct Hotkey {
    shortcut: Option<Shortcut>,
    toggles: mpsc::UnboundedReceiver<()>,
}

impl Hotkey {
    /// Binds the global shortcut. Without one (e.g. GNOME before 48), dictations can
    /// still be toggled by a desktop shortcut running `freisprech --toggle`.
    pub async fn bind(toggles: mpsc::UnboundedReceiver<()>) -> Self {
        let shortcut = Shortcut::bind()
            .await
            .inspect_err(|err| tracing::warn!("No global shortcut: {err:#}"))
            .ok();
        Self { shortcut, toggles }
    }

    pub fn has_shortcut(&self) -> bool {
        self.shortcut.is_some()
    }

    /// `None` once the global shortcut is lost. A toggle has no key release to report:
    /// the typed text then waits for the hold timeout.
    pub async fn next(&mut self) -> Option<HotkeyEvent> {
        let shortcut = async {
            match &mut self.shortcut {
                Some(shortcut) => shortcut.next().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = shortcut => event,
            Some(()) = self.toggles.recv() => Some(HotkeyEvent::Pressed),
        }
    }
}

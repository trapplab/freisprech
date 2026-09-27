//! Global shortcut via the XDG GlobalShortcuts portal (Wayland).
//!
//! The portal owns the actual key binding: the user confirms it once, and can change
//! it later in the desktop's shortcut settings.

use std::pin::Pin;

use anyhow::{Context as _, Result};
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::desktop::{CreateSessionOptions, Session};
use tokio_stream::{Stream, StreamExt};

use super::HotkeyEvent;

const SHORTCUT_ID: &str = "toggle-dictation";
const PREFERRED_TRIGGER: &str = "CTRL+ALT+d";

pub struct Shortcut {
    _portal: GlobalShortcuts,
    _session: Session<GlobalShortcuts>,
    events: Pin<Box<dyn Stream<Item = HotkeyEvent> + Send>>,
}

impl Shortcut {
    /// Registers the dictation shortcut. The first time, the desktop asks the user to confirm.
    pub async fn bind() -> Result<Self> {
        let portal = GlobalShortcuts::new()
            .await
            .context("GlobalShortcuts portal not available")?;
        let session = portal.create_session(CreateSessionOptions::default()).await?;

        // Subscribe before binding so no activation is missed.
        let pressed = portal.receive_activated().await?;
        let released = portal.receive_deactivated().await?;

        let shortcut = NewShortcut::new(SHORTCUT_ID, "Start/stop dictation")
            .preferred_trigger(PREFERRED_TRIGGER);
        let bound = portal
            .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
            .await?
            .response()
            .context("Shortcut was not confirmed")?;
        for s in bound.shortcuts() {
            tracing::info!(id = s.id(), trigger = s.trigger_description(), "Shortcut active");
        }

        let pressed = pressed
            .filter(|a| a.shortcut_id() == SHORTCUT_ID)
            .map(|_| HotkeyEvent::Pressed);
        let released = released
            .filter(|d| d.shortcut_id() == SHORTCUT_ID)
            .map(|_| HotkeyEvent::Released);

        Ok(Self {
            _portal: portal,
            _session: session,
            events: Box::pin(pressed.merge(released)),
        })
    }

    pub async fn next(&mut self) -> Option<HotkeyEvent> {
        self.events.next().await
    }
}

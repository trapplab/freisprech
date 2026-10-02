//! Global shortcut on X11: a passive grab of Ctrl+Alt+D on the root window. It is fixed,
//! and fails if another application already grabbed the same keys.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use tokio::sync::mpsc;
use x11rb::connection::Connection as _;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, KeyButMask, Keycode, ModMask, Window};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use super::HotkeyEvent;
use crate::x11::{self, KeyboardMapping};

const KEYSYM_D: u32 = 0x64;
/// How long to wait for Ctrl and Alt to be let go after D before reporting the release anyway.
const MODIFIER_TIMEOUT: Duration = Duration::from_secs(10);
const MODIFIER_POLL: Duration = Duration::from_millis(20);

pub struct Shortcut {
    events: mpsc::UnboundedReceiver<HotkeyEvent>,
}

impl Shortcut {
    pub fn bind() -> Result<Self> {
        let (conn, root) = x11::connect()?;

        // Otherwise a held key repeats as release + press pairs.
        conn.xkb_use_extension(1, 0)?.reply()?;
        let repeat = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
        conn.xkb_per_client_flags(
            xkb::ID::USE_CORE_KBD.into(),
            repeat,
            repeat,
            0u32.into(),
            0u32.into(),
            0u32.into(),
        )?
        .reply()?;

        let key = KeyboardMapping::get(&conn)?
            .keycode_of(KEYSYM_D)
            .context("The keyboard layout has no D key")?;
        // A grab only matches its exact modifiers, so also grab with Caps Lock and Num Lock on.
        let modifiers = ModMask::CONTROL | ModMask::M1;
        for locks in [ModMask::from(0u16), ModMask::LOCK, ModMask::M2, ModMask::LOCK | ModMask::M2] {
            conn.grab_key(true, root, modifiers | locks, key, GrabMode::ASYNC, GrabMode::ASYNC)?
                .check()
                .context("Ctrl+Alt+D is already taken by another application")?;
        }
        tracing::info!("Shortcut active (X11 key grab)");

        let (tx, rx) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("x11-shortcut".into())
            .spawn(move || {
                if let Err(err) = listen(&conn, root, key, &tx) {
                    tracing::error!("X11 shortcut stopped: {err:#}");
                }
            })?;
        Ok(Self { events: rx })
    }

    pub async fn next(&mut self) -> Option<HotkeyEvent> {
        self.events.recv().await
    }
}

/// Forwards presses and releases of the grabbed key until the receiver is gone.
fn listen(
    conn: &RustConnection,
    root: Window,
    key: Keycode,
    tx: &mpsc::UnboundedSender<HotkeyEvent>,
) -> Result<()> {
    let mut down = false;
    loop {
        let event = match conn.wait_for_event()? {
            Event::KeyPress(e) if e.detail == key && !down => {
                down = true;
                HotkeyEvent::Pressed
            }
            Event::KeyRelease(e) if e.detail == key => {
                down = false;
                wait_for_modifiers_up(conn, root)?;
                HotkeyEvent::Released
            }
            _ => continue,
        };
        if tx.send(event).is_err() {
            return Ok(());
        }
    }
}

/// D usually comes up before Ctrl and Alt; typing while they are held makes shortcuts.
fn wait_for_modifiers_up(conn: &RustConnection, root: Window) -> Result<()> {
    let held = u16::from(KeyButMask::CONTROL | KeyButMask::MOD1);
    let deadline = Instant::now() + MODIFIER_TIMEOUT;
    while Instant::now() < deadline {
        let mask = u16::from(conn.query_pointer(root)?.reply()?.mask);
        if mask & held == 0 {
            break;
        }
        std::thread::sleep(MODIFIER_POLL);
    }
    Ok(())
}

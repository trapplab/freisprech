//! X11 sessions (e.g. Xfce): there are no portals for the shortcut and typing, so both
//! talk to the X server directly. Shared here: session detection and the keyboard mapping.

use anyhow::{Context as _, Result};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt as _, Keycode, Keysym, Window};
use x11rb::rust_connection::RustConnection;

/// An X server without a Wayland compositor. XWayland (both variables set) counts as Wayland.
pub fn is_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some()
}

/// Connects to the X server and returns the root window of the default screen.
pub fn connect() -> Result<(RustConnection, Window)> {
    let (conn, screen) = x11rb::connect(None).context("Cannot connect to the X server")?;
    let root = conn.setup().roots[screen].root;
    Ok((conn, root))
}

/// The core keyboard mapping: per keycode one keysym per column. With XKB the columns are
/// group 1 (plain, Shift), group 2 (plain, Shift), then group 1 with AltGr (plain, Shift).
pub struct KeyboardMapping {
    first: Keycode,
    per_keycode: usize,
    keysyms: Vec<Keysym>,
}

impl KeyboardMapping {
    pub fn get(conn: &RustConnection) -> Result<Self> {
        let setup = conn.setup();
        let (first, last) = (setup.min_keycode, setup.max_keycode);
        let reply = conn
            .get_keyboard_mapping(first, last - first + 1)?
            .reply()?;
        Ok(Self {
            first,
            per_keycode: usize::from(reply.keysyms_per_keycode).max(1),
            keysyms: reply.keysyms,
        })
    }

    #[cfg(test)]
    pub fn new(first: Keycode, per_keycode: usize, keysyms: Vec<Keysym>) -> Self {
        Self {
            first,
            per_keycode,
            keysyms,
        }
    }

    pub fn per_keycode(&self) -> usize {
        self.per_keycode
    }

    /// Every keycode with its keysyms, `0` (NoSymbol) where a column is empty.
    pub fn keys(&self) -> impl Iterator<Item = (Keycode, &[Keysym])> {
        (self.first..=Keycode::MAX).zip(self.keysyms.chunks(self.per_keycode))
    }

    /// The first keycode that has `keysym` in any column.
    pub fn keycode_of(&self, keysym: Keysym) -> Option<Keycode> {
        self.keys()
            .find(|(_, syms)| syms.contains(&keysym))
            .map(|(kc, _)| kc)
    }

    /// Updates our copy after we changed the mapping of `keycode` on the server.
    pub fn set(&mut self, keycode: Keycode, syms: &[Keysym]) {
        let start = usize::from(keycode - self.first) * self.per_keycode;
        self.keysyms[start..start + self.per_keycode].copy_from_slice(syms);
    }
}

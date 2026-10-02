//! X11: key presses are injected with the XTEST extension. Every character is mapped
//! through the current keyboard mapping to a key plus Shift/AltGr; a character the layout
//! lacks is bound to an unused keycode, which stays borrowed until the typer is dropped.

use std::time::Duration;

use anyhow::{Context as _, Result};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    ConnectionExt as _, Keycode, Keysym, Window, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use xkbcommon::xkb;

use crate::x11::{self, KeyboardMapping};

/// Pause between synthetic key strokes so slow applications keep up.
const KEY_DELAY: Duration = Duration::from_millis(3);
/// Applications pick up a changed keyboard mapping only after a moment.
const REMAP_DELAY: Duration = Duration::from_millis(50);

const NO_SYMBOL: Keysym = 0;
const RETURN: Keysym = 0xff0d;
const SHIFT_L: Keysym = 0xffe1;
const ISO_LEVEL3_SHIFT: Keysym = 0xfe03;

pub struct Typer {
    conn: RustConnection,
    root: Window,
    /// Unused keycodes we bound to characters the layout lacks, oldest first.
    borrowed: Vec<Keycode>,
}

struct Stroke {
    key: Keycode,
    modifiers: Vec<Keycode>,
}

impl Typer {
    pub fn connect() -> Result<Self> {
        let (conn, root) = x11::connect()?;
        conn.xtest_get_version(2, 2)?
            .reply()
            .context("The X server lacks the XTEST extension")?;
        tracing::info!("Typing via XTEST");
        Ok(Self {
            conn,
            root,
            borrowed: Vec::new(),
        })
    }

    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        // Fetched every time: the user may have switched the layout.
        let mut mapping = KeyboardMapping::get(&self.conn)?;
        let shift = mapping.keycode_of(SHIFT_L);
        let level3 = mapping.keycode_of(ISO_LEVEL3_SHIFT);
        for c in text.chars() {
            let stroke = match lookup(&mapping, c, shift, level3) {
                Some(stroke) => stroke,
                None => match self.borrow_keycode(&mut mapping, c).await? {
                    Some(key) => Stroke {
                        key,
                        modifiers: Vec::new(),
                    },
                    None => {
                        tracing::warn!(?c, "Character not reachable on the keyboard layout");
                        continue;
                    }
                },
            };
            for &m in &stroke.modifiers {
                self.key(m, true)?;
            }
            self.key(stroke.key, true)?;
            self.key(stroke.key, false)?;
            for &m in stroke.modifiers.iter().rev() {
                self.key(m, false)?;
            }
            self.conn.flush()?;
            tokio::time::sleep(KEY_DELAY).await;
        }
        Ok(())
    }

    fn key(&self, key: Keycode, press: bool) -> Result<()> {
        let kind = if press { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT };
        self.conn
            .xtest_fake_input(kind, key, x11rb::CURRENT_TIME, self.root, 0, 0, 0)?;
        Ok(())
    }

    /// Binds `c` to a keycode without keysyms, or else to the one borrowed longest ago.
    /// `None` if there is neither.
    async fn borrow_keycode(
        &mut self,
        mapping: &mut KeyboardMapping,
        c: char,
    ) -> Result<Option<Keycode>> {
        let keysym = xkb::utf32_to_keysym(u32::from(c)).raw();
        if keysym == NO_SYMBOL {
            return Ok(None);
        }
        let free = mapping
            .keys()
            .find(|(_, syms)| syms.iter().all(|&sym| sym == NO_SYMBOL))
            .map(|(key, _)| key);
        let key = match free {
            Some(key) => key,
            None if !self.borrowed.is_empty() => {
                // Its last character may still be on its way to the application.
                tokio::time::sleep(REMAP_DELAY).await;
                self.borrowed.remove(0)
            }
            None => return Ok(None),
        };
        // Also in the Shift column, so a held Shift doesn't matter.
        let mut syms = vec![NO_SYMBOL; mapping.per_keycode()];
        for sym in syms.iter_mut().take(2) {
            *sym = keysym;
        }
        self.change_mapping(key, &syms)?;
        mapping.set(key, &syms);
        self.borrowed.push(key);
        tokio::time::sleep(REMAP_DELAY).await;
        Ok(Some(key))
    }

    fn change_mapping(&self, key: Keycode, syms: &[Keysym]) -> Result<()> {
        let per_keycode = u8::try_from(syms.len())?;
        self.conn
            .change_keyboard_mapping(1, key, per_keycode, syms)?
            .check()?;
        Ok(())
    }
}

impl Drop for Typer {
    /// Gives the borrowed keycodes back.
    fn drop(&mut self) {
        if self.borrowed.is_empty() {
            return;
        }
        let result = KeyboardMapping::get(&self.conn).and_then(|mapping| {
            let empty = vec![NO_SYMBOL; mapping.per_keycode()];
            self.borrowed
                .iter()
                .try_for_each(|&key| self.change_mapping(key, &empty))
        });
        if let Err(err) = result {
            tracing::warn!("Failed to restore the keyboard mapping: {err:#}");
        }
    }
}

/// The key that produces `c` with the fewest modifiers, only from columns of the first
/// layout group: plain, Shift, AltGr, AltGr+Shift.
fn lookup(
    mapping: &KeyboardMapping,
    c: char,
    shift: Option<Keycode>,
    level3: Option<Keycode>,
) -> Option<Stroke> {
    let columns: [(usize, &[Option<Keycode>]); 4] = [
        (0, &[]),
        (1, &[shift]),
        (4, &[level3]),
        (5, &[level3, shift]),
    ];
    for (column, modifiers) in columns {
        let Some(modifiers) = modifiers.iter().copied().collect::<Option<Vec<_>>>() else {
            continue;
        };
        let key = mapping
            .keys()
            .find(|(_, syms)| syms.get(column).is_some_and(|&sym| produces(sym, c)));
        if let Some((key, _)) = key {
            return Some(Stroke { key, modifiers });
        }
    }
    None
}

/// Keypad keys are skipped: what they type depends on Num Lock.
fn produces(keysym: Keysym, c: char) -> bool {
    match c {
        '\n' => keysym == RETURN,
        _ => {
            keysym != NO_SYMBOL
                && !(0xff80..=0xffbd).contains(&keysym)
                && xkb::keysym_to_utf32(xkb::Keysym::new(keysym)) == u32::from(c)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A few keys of the German layout, as `xmodmap -pke` prints them.
    fn german() -> KeyboardMapping {
        #[rustfmt::skip]
        let keysyms = vec![
            0x65, 0x45, 0x65, 0x45, 0x20ac, 0x20ac, // 26: e E e E EuroSign EuroSign
            0xff0d, 0, 0xff0d, 0, 0, 0,             // 27: Return
            SHIFT_L, 0, SHIFT_L, 0, 0, 0,           // 28: Shift_L
            ISO_LEVEL3_SHIFT, 0, ISO_LEVEL3_SHIFT, 0, 0, 0, // 29: ISO_Level3_Shift
            0xffb1, 0xffb1, 0, 0, 0, 0,             // 30: KP_1
            0x76, 0x56, 0x76, 0x56, 0xafe, 0x1002019, // 31: v V v V doublelowquotemark U2019
        ];
        KeyboardMapping::new(26, 6, keysyms)
    }

    fn stroke(c: char) -> Option<(Keycode, Vec<Keycode>)> {
        let mapping = german();
        let shift = mapping.keycode_of(SHIFT_L);
        let level3 = mapping.keycode_of(ISO_LEVEL3_SHIFT);
        lookup(&mapping, c, shift, level3).map(|s| (s.key, s.modifiers))
    }

    #[test]
    fn finds_key_and_modifiers() {
        assert_eq!(stroke('e'), Some((26, vec![])));
        assert_eq!(stroke('E'), Some((26, vec![28])));
        assert_eq!(stroke('€'), Some((26, vec![29])));
        assert_eq!(stroke('„'), Some((31, vec![29])));
        assert_eq!(stroke('’'), Some((31, vec![29, 28])));
        assert_eq!(stroke('\n'), Some((27, vec![])));
    }

    #[test]
    fn skips_keypad_and_unknown_characters() {
        assert_eq!(stroke('1'), None);
        assert_eq!(stroke('é'), None);
    }
}

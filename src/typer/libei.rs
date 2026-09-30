//! Wayland: the RemoteDesktop portal hands out a libei connection. If the compositor
//! offers `ei_text`, UTF-8 is sent as is; otherwise every character is mapped through
//! the compositor's keymap to a key press plus the modifiers (Shift, AltGr) it needs.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use ashpd::desktop::remote_desktop::{
    ConnectToEISOptions, DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use reis::ei;
use reis::event::{Connection, Device, DeviceCapability, EiEvent};
use reis::tokio::EiConvertEventStream;
use tokio::time::{timeout, timeout_at, Instant};
use tokio_stream::StreamExt;
use xkbcommon::xkb;

/// How long the compositor gets to hand out the libei socket and complete the handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the compositor gets to announce its devices after the handshake.
const DEVICE_TIMEOUT: Duration = Duration::from_secs(5);
/// After a keyboard shows up, wait this long in case a text-capable device follows.
const TEXT_DEVICE_GRACE: Duration = Duration::from_millis(300);
/// Pause between synthetic key strokes so slow applications keep up.
const KEY_DELAY: Duration = Duration::from_millis(3);
/// Longest string `ei_text.utf8` accepts.
const MAX_TEXT_BYTES: usize = 254;

pub struct Typer {
    _portal: RemoteDesktop,
    _session: Session<RemoteDesktop>,
    connection: Connection,
    events: EiConvertEventStream,
    device: Device,
    /// `None` when the device accepts UTF-8 text directly.
    keys: Option<KeyMap>,
    sequence: u32,
}

impl Typer {
    /// Opens the portal session. Only the very first call shows a permission dialog:
    /// the portal's restore token is kept in the config directory.
    pub async fn connect() -> Result<Self> {
        let token_file = dirs::config_dir()
            .unwrap_or_default()
            .join("freisprech")
            .join("remote-desktop.token");
        let token = std::fs::read_to_string(&token_file).ok();
        let (typer, new_token) = Self::open(token.as_deref().map(str::trim)).await?;
        if let Some(new_token) = new_token {
            std::fs::create_dir_all(token_file.parent().unwrap())?;
            std::fs::write(&token_file, new_token)?;
        }
        Ok(typer)
    }

    async fn open(restore_token: Option<&str>) -> Result<(Self, Option<String>)> {
        let portal = RemoteDesktop::new()
            .await
            .context("RemoteDesktop portal not available")?;
        let session = portal.create_session(CreateSessionOptions::default()).await?;
        portal
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(BitFlags::from_flag(DeviceType::Keyboard))
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(restore_token),
            )
            .await?
            .response()?;
        let selected = portal
            .start(&session, None, StartOptions::default())
            .await?
            .response()
            .context("Keyboard access was not granted")?;
        let new_token = selected.restore_token().map(str::to_owned);

        // Neither step has a timeout of its own; after a crashed run KWin has been seen
        // to never answer here.
        let (connection, mut events) = timeout(CONNECT_TIMEOUT, async {
            let fd = portal
                .connect_to_eis(&session, ConnectToEISOptions::default())
                .await
                .context(
                    "The desktop does not offer keyboard input via libei \
                     (requires a recent GNOME, e.g. Ubuntu 26.04 / Debian 13, or KDE Plasma 6)",
                )?;
            tracing::debug!("Connected to EIS");
            let socket = UnixStream::from(fd);
            socket.set_nonblocking(true)?;
            let context = ei::Context::new(socket)?;
            let handshake = context
                .handshake_tokio("freisprech", ei::handshake::ContextType::Sender)
                .await?;
            anyhow::Ok(handshake)
        })
        .await
        .context("The desktop did not answer the keyboard connection – try starting again")??;

        let device = wait_for_device(&connection, &mut events).await?;
        let keys = if device.has_capability(DeviceCapability::Text) {
            tracing::info!("Typing via ei_text (UTF-8)");
            None
        } else {
            tracing::info!("Typing via keyboard keymap");
            Some(KeyMap::from_device(&device)?)
        };

        let typer = Self {
            _portal: portal,
            _session: session,
            connection,
            events,
            device,
            keys,
            sequence: 0,
        };
        Ok((typer, new_token))
    }

    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        self.handle_pending_events().await?;

        let device = self.device.device().clone();
        device.start_emulating(self.connection.serial(), self.sequence);
        self.sequence = self.sequence.wrapping_add(1);

        match &self.keys {
            None => {
                let text_iface = self
                    .device
                    .interface::<ei::Text>()
                    .context("Device has no ei_text")?;
                for (i, line) in text.split('\n').enumerate() {
                    // Applications take a line break only as the Enter key.
                    if i > 0 {
                        for state in [ei::keyboard::KeyState::Press, ei::keyboard::KeyState::Released] {
                            text_iface.keysym(xkb::Keysym::Return.raw(), state);
                            self.frame(&device);
                        }
                    }
                    for chunk in utf8_chunks(line, MAX_TEXT_BYTES) {
                        text_iface.utf8(chunk);
                        self.frame(&device);
                    }
                }
                self.flush()?;
            }
            Some(keys) => {
                let keyboard = self
                    .device
                    .interface::<ei::Keyboard>()
                    .context("Device has no keyboard")?;
                for c in text.chars() {
                    let Some(stroke) = keys.lookup(c) else {
                        tracing::warn!(?c, "Character not reachable on the keyboard layout");
                        continue;
                    };
                    // One key change per frame; several in one frame is a client bug in libei.
                    for &m in &stroke.modifiers {
                        keyboard.key(evdev(m), ei::keyboard::KeyState::Press);
                        self.frame(&device);
                    }
                    keyboard.key(evdev(stroke.key), ei::keyboard::KeyState::Press);
                    self.frame(&device);
                    keyboard.key(evdev(stroke.key), ei::keyboard::KeyState::Released);
                    self.frame(&device);
                    for &m in stroke.modifiers.iter().rev() {
                        keyboard.key(evdev(m), ei::keyboard::KeyState::Released);
                        self.frame(&device);
                    }
                    self.flush()?;
                    tokio::time::sleep(KEY_DELAY).await;
                }
            }
        }

        device.stop_emulating(self.connection.serial());
        self.flush()
    }

    /// Processes queued compositor events (pings, pause/disconnect) without blocking.
    async fn handle_pending_events(&mut self) -> Result<()> {
        while let Ok(Some(event)) = timeout(Duration::ZERO, self.events.next()).await {
            match event? {
                EiEvent::Disconnected(d) => bail!("Compositor closed the connection: {:?}", d.reason),
                EiEvent::DevicePaused(e) if e.device == self.device => {
                    bail!("Compositor paused the keyboard")
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn frame(&self, device: &ei::Device) {
        let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        let micros = now.tv_sec as u64 * 1_000_000 + now.tv_nsec as u64 / 1_000;
        device.frame(self.connection.serial(), micros);
    }

    fn flush(&self) -> Result<()> {
        self.connection
            .flush()
            .map_err(|e| anyhow!("libei connection: {e}"))
    }
}

/// Binds keyboard + text on every seat and returns the best resumed device.
async fn wait_for_device(connection: &Connection, events: &mut EiConvertEventStream) -> Result<Device> {
    let mut keyboard = None;
    let mut deadline = Instant::now() + DEVICE_TIMEOUT;
    loop {
        let event = match timeout_at(deadline, events.next()).await {
            Err(_) => break,
            Ok(None) => bail!("libei connection was closed"),
            Ok(Some(event)) => event?,
        };
        match event {
            EiEvent::SeatAdded(e) => {
                e.seat
                    .bind_capabilities(DeviceCapability::Keyboard | DeviceCapability::Text);
                connection.flush().map_err(|e| anyhow!("libei connection: {e}"))?;
            }
            EiEvent::DeviceResumed(e) => {
                if e.device.has_capability(DeviceCapability::Text) {
                    return Ok(e.device);
                }
                if keyboard.is_none() && e.device.has_capability(DeviceCapability::Keyboard) {
                    keyboard = Some(e.device);
                    deadline = Instant::now() + TEXT_DEVICE_GRACE;
                }
            }
            EiEvent::Disconnected(d) => bail!("Compositor closed the connection: {:?}", d.reason),
            _ => {}
        }
    }
    keyboard.context("Compositor provided no keyboard device")
}

/// Splits `text` into pieces of at most `max` bytes, on character boundaries.
fn utf8_chunks(mut text: &str, max: usize) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if text.is_empty() {
            return None;
        }
        let mut end = text.len().min(max);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let (chunk, rest) = text.split_at(end);
        text = rest;
        Some(chunk)
    })
}

/// libei uses evdev keycodes, xkb keycodes are offset by 8.
fn evdev(xkb_keycode: u32) -> u32 {
    xkb_keycode - 8
}

struct Stroke {
    key: u32,
    modifiers: Vec<u32>,
}

/// Reverse lookup character -> key + modifier keys in the compositor's keymap.
struct KeyMap {
    keymap: xkb::Keymap,
    /// Modifier mask bit and the key that sets it (e.g. Shift_L, ISO_Level3_Shift).
    modifier_keys: Vec<(xkb::ModMask, u32)>,
    lock_mask: xkb::ModMask,
}

impl KeyMap {
    fn from_device(device: &Device) -> Result<Self> {
        let km = device.keymap().context("Compositor sent no keymap")?;
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // SAFETY: the fd is a keymap memfd of `size` bytes sent by the compositor.
        let keymap = unsafe {
            xkb::Keymap::new_from_fd(
                &context,
                km.fd.try_clone()?,
                km.size as usize,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::COMPILE_NO_FLAGS,
            )
        }?
        .context("Failed to read keymap")?;

        let mut modifier_keys: Vec<(xkb::ModMask, u32)> = Vec::new();
        for kc in keycodes(&keymap) {
            let mut state = xkb::State::new(&keymap);
            state.update_key(xkb::Keycode::new(kc), xkb::KeyDirection::Down);
            let mask = state.serialize_mods(xkb::STATE_MODS_DEPRESSED);
            if mask.count_ones() == 1 && !modifier_keys.iter().any(|(m, _)| *m == mask) {
                modifier_keys.push((mask, kc));
            }
        }
        let lock_mask = 1 << keymap.mod_get_index("Lock");

        Ok(Self {
            keymap,
            modifier_keys,
            lock_mask,
        })
    }

    fn lookup(&self, c: char) -> Option<Stroke> {
        let keysym = match c {
            '\n' => xkb::Keysym::Return,
            '\t' => xkb::Keysym::Tab,
            _ => xkb::utf32_to_keysym(c as u32),
        };
        let layout = 0;

        // Prefer the key/level that needs the fewest modifiers; never rely on Caps Lock.
        let mut best: Option<(u32, xkb::ModMask)> = None;
        for kc in keycodes(&self.keymap) {
            let code = xkb::Keycode::new(kc);
            for level in 0..self.keymap.num_levels_for_key(code, layout) {
                if !self
                    .keymap
                    .key_get_syms_by_level(code, layout, level)
                    .contains(&keysym)
                {
                    continue;
                }
                let mut masks = [0; 8];
                let n = self
                    .keymap
                    .key_get_mods_for_level(code, layout, level, &mut masks);
                for &mask in &masks[..n] {
                    let better = best.is_none_or(|(_, m)| mask.count_ones() < m.count_ones());
                    if mask & self.lock_mask == 0 && better {
                        best = Some((kc, mask));
                    }
                }
            }
        }

        let (key, mask) = best?;
        let modifiers = (0..32)
            .map(|bit| 1 << bit)
            .filter(|bit| mask & bit != 0)
            .map(|bit| {
                self.modifier_keys
                    .iter()
                    .find(|(m, _)| *m == bit)
                    .map(|(_, kc)| *kc)
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Stroke { key, modifiers })
    }
}

fn keycodes(keymap: &xkb::Keymap) -> std::ops::RangeInclusive<u32> {
    keymap.min_keycode().raw()..=keymap.max_keycode().raw()
}

//! Global shortcut via `RegisterHotKey` on a dedicated message-loop thread.

use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, oneshot};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_CONTROL, VK_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

use super::HotkeyEvent;

const HOTKEY_ID: i32 = 1;
const KEY_D: u32 = b'D' as u32;

pub struct Shortcut {
    events: mpsc::UnboundedReceiver<HotkeyEvent>,
}

impl Shortcut {
    pub async fn bind() -> Result<Self> {
        let (tx, events) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        std::thread::Builder::new()
            .name("hotkey".into())
            .spawn(move || message_loop(tx, ready_tx))?;
        ready_rx
            .await?
            .map_err(|_| anyhow!("Ctrl+Alt+D is already taken by another program"))?;
        tracing::info!(trigger = "Ctrl+Alt+D", "Shortcut active");
        Ok(Self { events })
    }

    pub async fn next(&mut self) -> Option<HotkeyEvent> {
        self.events.recv().await
    }
}

/// WM_HOTKEY goes to the queue of the registering thread, so that thread must pump messages.
fn message_loop(tx: mpsc::UnboundedSender<HotkeyEvent>, ready: oneshot::Sender<Result<(), ()>>) {
    // SAFETY: plain Win32 calls; `msg` is a valid out-pointer for GetMessageW.
    unsafe {
        let modifiers = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
        if RegisterHotKey(std::ptr::null_mut(), HOTKEY_ID, modifiers, KEY_D) == 0 {
            let _ = ready.send(Err(()));
            return;
        }
        let _ = ready.send(Ok(()));

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message != WM_HOTKEY || msg.wParam != HOTKEY_ID as usize {
                continue;
            }
            if tx.send(HotkeyEvent::Pressed).is_err() {
                return;
            }
            // Hotkeys have no key-up message: poll until Ctrl and Alt are released.
            while key_down(VK_CONTROL) || key_down(VK_MENU) {
                std::thread::sleep(Duration::from_millis(10));
            }
            if tx.send(HotkeyEvent::Released).is_err() {
                return;
            }
        }
    }
}

fn key_down(vk: u16) -> bool {
    // SAFETY: GetAsyncKeyState has no preconditions.
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

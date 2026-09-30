//! Typing via `SendInput` with `KEYEVENTF_UNICODE`: no keyboard layout involved,
//! so umlauts, symbols and emoji arrive as they are. Line breaks are sent as Enter.

use anyhow::{bail, Result};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_RETURN,
};

pub struct Typer;

impl Typer {
    pub async fn connect() -> Result<Self> {
        Ok(Self)
    }

    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        let mut inputs: Vec<INPUT> = Vec::new();
        for c in text.chars() {
            if c == '\n' {
                inputs.extend([key(VK_RETURN, 0, 0), key(VK_RETURN, 0, KEYEVENTF_KEYUP)]);
            } else {
                for &unit in c.encode_utf16(&mut [0; 2]).iter() {
                    inputs.extend([
                        key(0, unit, KEYEVENTF_UNICODE),
                        key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                    ]);
                }
            }
        }
        // SAFETY: `inputs` is a valid array of INPUT structs of the given length.
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
        if sent as usize != inputs.len() {
            // UIPI: windows of elevated (admin) processes reject input from normal ones.
            bail!("SendInput was blocked (target window running with admin rights?)");
        }
        Ok(())
    }
}

fn key(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

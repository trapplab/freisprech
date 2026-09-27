//! Typing via `SendInput` with `KEYEVENTF_UNICODE`: no keyboard layout involved,
//! so umlauts, symbols and emoji arrive as they are.

use anyhow::{bail, Result};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE,
};

pub struct Typer;

impl Typer {
    pub async fn connect() -> Result<Self> {
        Ok(Self)
    }

    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        let inputs: Vec<INPUT> = text
            .encode_utf16()
            .flat_map(|unit| [key(unit, 0), key(unit, KEYEVENTF_KEYUP)])
            .collect();
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

fn key(unit: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

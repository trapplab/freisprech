//! Tray icon with menu, reflecting the controller status.

#[cfg(target_os = "linux")]
mod sni;
#[cfg(target_os = "linux")]
pub use sni::Tray;

#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use win32::Tray;

use crate::controller::Status;

const ICON_SIZE: u32 = 32;

#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    OpenSettings,
    Quit,
}

fn tooltip(status: &Status) -> String {
    format!("Freisprech – {}", status.describe())
}

/// RGBA pixels of a filled circle whose colour reflects the state; drawn in code so
/// no image files are needed.
fn icon_rgba(status: &Status) -> Vec<u8> {
    let [r, g, b] = match status {
        Status::Recording => [0xd9, 0x30, 0x25],
        Status::Ready { .. } => [0x3d, 0x7e, 0xc9],
        Status::Starting | Status::Downloading(_) | Status::LoadingModel => [0xe0, 0xa1, 0x00],
        Status::Error(_) | Status::Fatal(_) => [0x70, 0x70, 0x70],
    };
    let center = (ICON_SIZE as f32 - 1.0) / 2.0;
    let radius = ICON_SIZE as f32 / 2.0 - 1.0;
    let mut rgba = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let d = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            // One pixel of soft edge for anti-aliasing.
            let alpha = (radius - d + 0.5).clamp(0.0, 1.0);
            rgba.extend_from_slice(&[r, g, b, (alpha * 255.0) as u8]);
        }
    }
    rgba
}

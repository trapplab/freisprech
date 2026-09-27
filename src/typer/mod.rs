//! Types text into whatever window currently has keyboard focus.

#[cfg(target_os = "linux")]
mod libei;
#[cfg(target_os = "linux")]
pub use libei::Typer;

#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use win32::Typer;

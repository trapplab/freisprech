//! Types text into whatever window currently has keyboard focus.

#[cfg(target_os = "linux")]
mod libei;
#[cfg(target_os = "linux")]
mod xtest;

#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use win32::Typer;

/// Wayland: via the RemoteDesktop portal and libei. X11: via XTEST.
#[cfg(target_os = "linux")]
pub enum Typer {
    Libei(libei::Typer),
    Xtest(xtest::Typer),
}

#[cfg(target_os = "linux")]
impl Typer {
    pub async fn connect() -> anyhow::Result<Self> {
        if crate::x11::is_session() {
            return Ok(Self::Xtest(xtest::Typer::connect()?));
        }
        Ok(Self::Libei(libei::Typer::connect().await?))
    }

    pub async fn type_text(&mut self, text: &str) -> anyhow::Result<()> {
        match self {
            Self::Libei(typer) => typer.type_text(text).await,
            Self::Xtest(typer) => typer.type_text(text).await,
        }
    }
}

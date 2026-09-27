//! Linux tray via StatusNotifierItem (D-Bus), shown natively by KDE.
//!
//! Runs on its own thread with a tokio runtime: zbus is built with its tokio backend
//! (the portals need it), so every D-Bus call must happen inside a tokio context.

use anyhow::{anyhow, Result};
use ksni::menu::StandardItem;
use ksni::{MenuItem, ToolTip, TrayMethods};
use tokio::sync::mpsc;

use super::{icon_rgba, tooltip, TrayEvent, ICON_SIZE};
use crate::controller::{Status, APP_ID};

pub struct Tray {
    status: mpsc::UnboundedSender<Status>,
}

impl Tray {
    /// Fails if the desktop shows no tray (e.g. GNOME without the AppIndicator extension).
    pub fn new(events: mpsc::UnboundedSender<TrayEvent>) -> Result<Self> {
        let (status_tx, mut status_rx) = mpsc::unbounded_channel::<Status>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        std::thread::Builder::new().name("tray".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = ready_tx.send(Err(err.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let tray = SniTray {
                    status: Status::Starting,
                    events,
                };
                let handle = match tray.spawn().await {
                    Ok(handle) => handle,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err.to_string()));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                while let Some(status) = status_rx.recv().await {
                    handle.update(|tray| tray.status = status).await;
                }
            });
        })?;

        ready_rx.recv()?.map_err(|err| anyhow!("Tray: {err}"))?;
        Ok(Self { status: status_tx })
    }

    pub fn set_status(&self, status: &Status) {
        let _ = self.status.send(status.clone());
    }
}

struct SniTray {
    status: Status,
    events: mpsc::UnboundedSender<TrayEvent>,
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "Freisprech".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // SNI wants ARGB in network byte order.
        let data = icon_rgba(&self.status)
            .chunks_exact(4)
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect();
        vec![ksni::Icon {
            width: ICON_SIZE as i32,
            height: ICON_SIZE as i32,
            data,
        }]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: tooltip(&self.status),
            ..Default::default()
        }
    }

    /// Left click.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.events.send(TrayEvent::OpenSettings);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Settings …".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.events.send(TrayEvent::OpenSettings);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.events.send(TrayEvent::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

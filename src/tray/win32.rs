//! Windows notification area icon, driven by the UI thread's message loop.

use anyhow::Result;
use tokio::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{icon_rgba, tooltip, TrayEvent, ICON_SIZE};
use crate::controller::Status;

const MENU_SETTINGS: &str = "settings";
const MENU_QUIT: &str = "quit";

pub struct Tray {
    icon: TrayIcon,
}

impl Tray {
    /// Must be created on the UI thread (it needs that thread's message loop).
    pub fn new(events: mpsc::UnboundedSender<TrayEvent>) -> Result<Self> {
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::with_id(MENU_SETTINGS, "Settings …", true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(MENU_QUIT, "Quit", true, None),
        ])?;

        let menu_events = events.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let tray_event = if event.id == MENU_SETTINGS {
                TrayEvent::OpenSettings
            } else if event.id == MENU_QUIT {
                TrayEvent::Quit
            } else {
                return;
            };
            let _ = menu_events.send(tray_event);
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = events.send(TrayEvent::OpenSettings);
            }
        }));

        let status = Status::Starting;
        let icon = TrayIconBuilder::new()
            .with_tooltip(tooltip(&status))
            .with_icon(icon(&status))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;
        Ok(Self { icon })
    }

    pub fn set_status(&self, status: &Status) {
        if let Err(err) = self.icon.set_icon(Some(icon(status))) {
            tracing::warn!(%err, "Failed to update tray icon");
        }
        let _ = self.icon.set_tooltip(Some(tooltip(status)));
    }
}

fn icon(status: &Status) -> Icon {
    Icon::from_rgba(icon_rgba(status), ICON_SIZE, ICON_SIZE).expect("icon buffer matches its size")
}

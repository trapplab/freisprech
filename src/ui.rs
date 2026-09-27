//! Tray icon plus settings window. Runs as an iced daemon: it starts without a window,
//! and closing the window (X, Esc, "Close", on Windows also minimize) only hides it.
//! Quitting is only offered in the tray menu, like KeePass.

use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use iced::keyboard::{self, key};
use iced::time::Instant;
use iced::widget::{button, column, container, pick_list, progress_bar, row, rule, space, text};
use iced::{border, window, Alignment, Element, Length, Size, Subscription, Task, Theme};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::audio;
use crate::config::Config;
use crate::controller::Status;
use crate::logging;
use crate::tray::{Tray, TrayEvent};

#[cfg(target_os = "linux")]
const HOTKEY_HINT: &str = "Ctrl+Alt+D (can be changed in the desktop's keyboard settings)";
#[cfg(not(target_os = "linux"))]
const HOTKEY_HINT: &str = "Ctrl+Alt+D";

/// Thickness of the progress bars.
const BAR_GIRTH: f32 = 6.0;
/// One sweep of the busy bar, there and back.
const BUSY_PERIOD: Duration = Duration::from_millis(1600);

/// Blocks on the UI event loop until the user quits. `settings_rx` asks to show the
/// settings window, e.g. when the app is started a second time.
pub fn run(
    config: Config,
    config_tx: watch::Sender<Config>,
    status_rx: mpsc::UnboundedReceiver<Status>,
    settings_rx: mpsc::UnboundedReceiver<()>,
    show_settings: bool,
) -> iced::Result {
    let init = Mutex::new(Some((config, config_tx, status_rx, settings_rx)));
    iced::daemon(
        move || {
            let (config, config_tx, status_rx, settings_rx) = init
                .lock()
                .expect("boot lock")
                .take()
                .expect("boot runs once");
            App::new(config, config_tx, status_rx, settings_rx, show_settings)
        },
        App::update,
        App::view,
    )
    .title(|_: &App, _| "Freisprech".to_owned())
    .subscription(App::subscription)
    .run()
}

#[derive(Debug, Clone)]
enum Message {
    Status(Status),
    Tray(TrayEvent),
    CloseRequested(window::Id),
    WindowClosed(window::Id),
    Resized(window::Id, Size),
    Key(keyboard::Event),
    /// Animation frame of the busy bar.
    Tick(Instant),
    Language(Language),
    Microphone(Microphone),
    OpenLogDir,
    CloseWindow,
    Quit,
}

struct App {
    window: Option<window::Id>,
    status: Status,
    config: Config,
    config_tx: watch::Sender<Config>,
    microphones: Vec<Microphone>,
    tray: Option<Tray>,
    /// Start and current frame of the busy bar animation.
    started: Instant,
    now: Instant,
}

impl App {
    fn new(
        config: Config,
        config_tx: watch::Sender<Config>,
        status_rx: mpsc::UnboundedReceiver<Status>,
        settings_rx: mpsc::UnboundedReceiver<()>,
        show_settings: bool,
    ) -> (Self, Task<Message>) {
        let (tray_tx, tray_rx) = mpsc::unbounded_channel();
        let tray = Tray::new(tray_tx)
            .inspect_err(|err| tracing::warn!("No tray icon available: {err:#}"))
            .ok();

        let mut app = Self {
            window: None,
            status: Status::Starting,
            config,
            config_tx,
            microphones: Vec::new(),
            tray,
            started: Instant::now(),
            now: Instant::now(),
        };
        let mut tasks = vec![
            Task::run(UnboundedReceiverStream::new(status_rx), Message::Status),
            Task::run(UnboundedReceiverStream::new(tray_rx), Message::Tray),
            Task::run(UnboundedReceiverStream::new(settings_rx), |()| {
                Message::Tray(TrayEvent::OpenSettings)
            }),
        ];
        // Without a tray there would be no way to reach the settings at all.
        if show_settings || app.tray.is_none() {
            tasks.push(app.open_settings());
        }
        (app, Task::batch(tasks))
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        tracing::debug!(?message, "UI");
        match message {
            Message::Status(status) => {
                if let Some(tray) = &self.tray {
                    tray.set_status(&status);
                }
                self.status = status;
            }
            Message::Tray(TrayEvent::OpenSettings) => return self.open_settings(),
            Message::Tray(TrayEvent::Quit) | Message::Quit => return iced::exit(),
            Message::CloseRequested(id) => return window::close(id),
            Message::WindowClosed(id) => {
                if self.window == Some(id) {
                    self.window = None;
                }
            }
            // Windows reports a minimized window as zero-sized: hide it like KeePass does.
            Message::Resized(id, size) if size.width == 0.0 || size.height == 0.0 => {
                return window::close(id)
            }
            Message::Resized(..) => {}
            Message::Key(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::Escape),
                ..
            })
            | Message::CloseWindow => {
                if let Some(id) = self.window {
                    return window::close(id);
                }
            }
            Message::Key(_) => {}
            Message::Tick(now) => self.now = now,
            Message::Language(language) => {
                self.config.language = language.code.to_owned();
                self.apply_config();
            }
            Message::Microphone(microphone) => {
                self.config.microphone = microphone.0;
                self.apply_config();
            }
            Message::OpenLogDir => open_in_file_manager(&logging::log_dir()),
        }
        Task::none()
    }

    fn view(&self, _window: window::Id) -> Element<'_, Message> {
        let language = LANGUAGES
            .iter()
            .find(|l| l.code == self.config.language)
            .copied();
        let microphone = Microphone(self.config.microphone.clone());

        column![
            text("Freisprech").size(22),
            text(self.status.describe()),
            self.progress(),
            rule::horizontal(1),
            setting("Language", pick_list(LANGUAGES, language, Message::Language)),
            setting(
                "Microphone",
                pick_list(self.microphones.as_slice(), Some(microphone), Message::Microphone)
            ),
            setting("Shortcut", text(self.hotkey_hint())),
            space::vertical(),
            row![
                button("Open log folder").on_press(Message::OpenLogDir),
                space::horizontal(),
            ]
            .push(self.tray.is_none().then(|| button("Quit").on_press(Message::Quit)))
            .push(button("Close").on_press(Message::CloseWindow))
            .spacing(10),
            text(if self.tray.is_some() {
                "The app keeps running in the tray. To quit: right-click the tray icon → Quit."
            } else {
                "The app keeps running in the background. Starting it again (e.g. from the app menu) \
                 reopens this window."
            })
            .size(12),
        ]
        .spacing(14)
        .padding(20)
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            window::close_requests().map(Message::CloseRequested),
            window::close_events().map(Message::WindowClosed),
            window::resize_events().map(|(id, size)| Message::Resized(id, size)),
            keyboard::listen().map(Message::Key),
            if self.window.is_some() && self.busy() {
                iced::time::every(Duration::from_millis(30)).map(Message::Tick)
            } else {
                Subscription::none()
            },
        ])
    }

    /// Working on something that reports no progress of its own.
    fn busy(&self) -> bool {
        matches!(self.status, Status::Starting | Status::LoadingModel)
    }

    /// The model download reports percent; starting and loading only that they run.
    fn progress(&self) -> Option<Element<'_, Message>> {
        match self.status {
            Status::Downloading(percent) => {
                Some(progress_bar(0.0..=100.0, percent as f32).girth(BAR_GIRTH).into())
            }
            _ if self.busy() => Some(busy_bar(self.now.duration_since(self.started))),
            _ => None,
        }
    }

    fn open_settings(&mut self) -> Task<Message> {
        if let Some(id) = self.window {
            return window::gain_focus(id);
        }
        // Refresh on every open so newly plugged-in microphones show up.
        self.microphones = std::iter::once(Microphone(None))
            .chain(audio::input_devices().into_iter().map(|name| Microphone(Some(name))))
            .collect();

        let (id, open) = window::open(window::Settings {
            size: Size::new(480.0, 380.0),
            resizable: false,
            // Wayland does not tell apps about minimizing, so there is nothing to hide on.
            minimizable: cfg!(windows),
            exit_on_close_request: false,
            #[cfg(target_os = "linux")]
            platform_specific: window::settings::PlatformSpecific {
                application_id: crate::controller::APP_ID.to_owned(),
                ..Default::default()
            },
            ..Default::default()
        });
        self.window = Some(id);
        open.discard()
    }

    fn hotkey_hint(&self) -> String {
        match self.status {
            Status::Ready { shortcut: false } => format!(
                "Not available – create your own shortcut that runs \"{} --toggle\"",
                std::env::current_exe().unwrap_or_default().display()
            ),
            _ => HOTKEY_HINT.to_owned(),
        }
    }

    fn apply_config(&mut self) {
        self.config_tx.send_replace(self.config.clone());
        if let Err(err) = self.config.save() {
            tracing::error!("Failed to save settings: {err:#}");
        }
    }
}

fn setting<'a>(label: &'a str, control: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    row![text(label).width(140), control.into()]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

/// Progress bar without a known end: a block sliding back and forth, styled like
/// iced's progress bar.
fn busy_bar<'a>(elapsed: Duration) -> Element<'a, Message> {
    // Widths in thousandths of the track.
    const BLOCK: u16 = 250;
    let phase = elapsed.as_secs_f32() / BUSY_PERIOD.as_secs_f32() % 1.0;
    let position = 1.0 - (2.0 * phase - 1.0).abs();
    let before = ((1000 - BLOCK) as f32 * position) as u16;
    let after = 1000 - BLOCK - before;

    // A portion of 0 would not count as fill, so keep a sliver.
    let gap = |portion: u16| space().width(Length::FillPortion(portion.max(1)));
    let block = container(space())
        .width(Length::FillPortion(BLOCK))
        .height(Length::Fill)
        .style(|theme: &Theme| {
            container::background(theme.extended_palette().primary.base.color)
                .border(border::rounded(2))
        });
    container(row![gap(before), block, gap(after)])
        .height(BAR_GIRTH)
        .style(|theme: &Theme| {
            container::background(theme.extended_palette().background.strong.color)
                .border(border::rounded(2))
        })
        .into()
}

fn open_in_file_manager(dir: &std::path::Path) {
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    if let Err(err) = std::process::Command::new(program).arg(dir).spawn() {
        tracing::error!(%err, "Failed to open folder");
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Language {
    code: &'static str,
    name: &'static str,
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// A selection of the model's languages; codes as understood by Foundry Local.
const LANGUAGES: &[Language] = &[
    Language { code: "auto", name: "Detect automatically" },
    Language { code: "de", name: "German" },
    Language { code: "en", name: "English" },
    Language { code: "fr", name: "French" },
    Language { code: "es", name: "Spanish" },
    Language { code: "it", name: "Italian" },
    Language { code: "nl", name: "Dutch" },
    Language { code: "pl", name: "Polish" },
    Language { code: "pt", name: "Portuguese" },
    Language { code: "cs", name: "Czech" },
    Language { code: "sv", name: "Swedish" },
    Language { code: "da", name: "Danish" },
    Language { code: "tr", name: "Turkish" },
    Language { code: "uk", name: "Ukrainian" },
    Language { code: "ru", name: "Russian" },
];

/// Input device name; `None` = system default.
#[derive(Debug, Clone, PartialEq)]
struct Microphone(Option<String>);

impl fmt::Display for Microphone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_deref().unwrap_or("System default"))
    }
}

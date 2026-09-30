//! Tray icon plus settings window. Runs as an iced daemon: it starts without a window,
//! and closing the window (X, Esc, "Close", on Windows also minimize) only hides it.
//! Quitting is only offered in the tray menu, like KeePass.

use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use iced::keyboard::{self, key};
use iced::time::Instant;
use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, row, rule, scrollable, space,
    text, text_input,
};
use iced::{
    border, font, widget, window, Alignment, Element, Font, Length, Padding, Size, Subscription, Task,
    Theme,
};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::audio;
use crate::config::{Config, Replacement, VoiceCommand};
use crate::controller::Status;
use crate::install;
use crate::language;
use crate::logging;
use crate::tray::{Tray, TrayEvent};
use crate::update;

#[cfg(target_os = "linux")]
const HOTKEY_HINT: &str = "Ctrl+Alt+D (can be changed in the desktop's keyboard settings)";
#[cfg(not(target_os = "linux"))]
const HOTKEY_HINT: &str = "Ctrl+Alt+D";

const SIDEBAR_WIDTH: f32 = 190.0;
/// Column widths of the voice command table: what gets typed, what to say.
const COMMAND_WIDTH: f32 = 100.0;
const PHRASE_WIDTH: f32 = 180.0;
/// Width of a word in the vocabulary table.
const WORD_WIDTH: f32 = 150.0;
/// Width of the "×" buttons that delete a row or column.
const REMOVE_WIDTH: f32 = 26.0;
const TABLE_SPACING: f32 = 6.0;
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
    Page(Page),
    Language(Language),
    Microphone(Microphone),
    VoiceCommands(bool),
    CommandOutput(usize, String),
    /// Command, language code, phrases.
    CommandPhrase(usize, String, String),
    AddCommand,
    RemoveCommand(usize),
    AddCommandLanguage(Language),
    RemoveCommandLanguage(String),
    ResetCommands,
    /// Word, what to type.
    VocabularyWritten(usize, String),
    /// Word, variant, how the model writes it.
    VocabularyHeard(usize, usize, String),
    AddHeard(usize),
    AddWord,
    RemoveWord(usize),
    Autostart(bool),
    Install,
    /// Asks for confirmation first.
    Uninstall,
    UninstallConfirmed,
    UninstallCancelled,
    CheckUpdate,
    UpdateChecked(Result<Option<update::Release>, String>),
    ReleaseNotes,
    Update,
    /// The installed path of the new version.
    Updated(Result<std::path::PathBuf, String>),
    OpenLogDir,
    CloseWindow,
    Quit,
}

struct App {
    window: Option<window::Id>,
    page: Page,
    status: Status,
    config: Config,
    config_tx: watch::Sender<Config>,
    microphones: Vec<Microphone>,
    /// Text edits reached the dictation but not the settings file yet.
    unsaved: bool,
    install: install::State,
    confirm_uninstall: bool,
    update: UpdateState,
    /// Last failed install, uninstall, autostart change or update.
    install_error: Option<String>,
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
            page: Page::General,
            status: Status::Starting,
            config,
            unsaved: false,
            config_tx,
            microphones: Vec::new(),
            install: install::State::default(),
            confirm_uninstall: false,
            update: UpdateState::Unchecked,
            install_error: None,
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
            Message::Tray(TrayEvent::Quit) | Message::Quit => return self.exit(),
            Message::CloseRequested(id) => return window::close(id),
            Message::WindowClosed(id) => {
                if self.window == Some(id) {
                    self.window = None;
                    self.tidy_and_save_config();
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
            Message::Page(page) => self.page = page,
            Message::Language(language) => {
                self.config.language = language.code.to_owned();
                self.apply_config();
            }
            Message::Microphone(microphone) => {
                self.config.microphone = microphone.0;
                self.apply_config();
            }
            Message::VoiceCommands(enabled) => {
                self.config.voice_commands = enabled;
                self.apply_config();
            }
            Message::CommandOutput(index, output) => {
                if let Some(command) = self.config.commands.get_mut(index) {
                    command.output = output.replace("\\n", "\n");
                    self.edit_config();
                }
            }
            Message::CommandPhrase(index, language, phrase) => {
                if let Some(command) = self.config.commands.get_mut(index) {
                    if phrase.is_empty() {
                        command.phrases.remove(&language);
                    } else {
                        command.phrases.insert(language, phrase);
                    }
                    self.edit_config();
                }
            }
            Message::AddCommand => {
                self.config.commands.push(VoiceCommand::default());
                self.edit_config();
                return widget::operation::focus(command_id(self.config.commands.len() - 1));
            }
            Message::RemoveCommand(index) => {
                if index < self.config.commands.len() {
                    self.config.commands.remove(index);
                    self.apply_config();
                }
            }
            Message::AddCommandLanguage(language) => {
                self.config.command_languages.push(language.code.to_owned());
                self.apply_config();
            }
            Message::RemoveCommandLanguage(language) => {
                self.config.command_languages.retain(|l| *l != language);
                for command in &mut self.config.commands {
                    command.phrases.remove(&language);
                }
                self.apply_config();
            }
            Message::ResetCommands => {
                self.config.commands = VoiceCommand::defaults();
                self.config.command_languages = VoiceCommand::default_languages();
                self.apply_config();
            }
            Message::VocabularyWritten(index, written) => {
                if let Some(word) = self.config.vocabulary.get_mut(index) {
                    word.written = written;
                    self.edit_config();
                }
            }
            Message::VocabularyHeard(index, variant, heard) => {
                let word = self.config.vocabulary.get_mut(index);
                if let Some(slot) = word.and_then(|w| w.heard.get_mut(variant)) {
                    *slot = heard;
                    self.edit_config();
                }
            }
            Message::AddHeard(index) => {
                if let Some(word) = self.config.vocabulary.get_mut(index) {
                    word.heard.push(String::new());
                    let variant = word.heard.len() - 1;
                    self.edit_config();
                    return widget::operation::focus(heard_id(index, variant));
                }
            }
            Message::AddWord => {
                self.config.vocabulary.push(Replacement {
                    written: String::new(),
                    heard: vec![String::new()],
                });
                self.edit_config();
                return widget::operation::focus(word_id(self.config.vocabulary.len() - 1));
            }
            Message::RemoveWord(index) => {
                if index < self.config.vocabulary.len() {
                    self.config.vocabulary.remove(index);
                    self.apply_config();
                }
            }
            Message::Autostart(enabled) => {
                let result = install::set_autostart(enabled);
                self.install = install::State::read();
                self.report(result);
            }
            Message::Install => match install::install() {
                // Continue as the installed copy.
                Ok(path) => {
                    install::relaunch_after_exit(&path);
                    return self.exit();
                }
                Err(err) => self.report(Err(err)),
            },
            Message::Uninstall => self.confirm_uninstall = true,
            Message::UninstallCancelled => self.confirm_uninstall = false,
            Message::UninstallConfirmed => {
                self.confirm_uninstall = false;
                match install::uninstall() {
                    Ok(()) => return self.exit(),
                    Err(err) => self.report(Err(err)),
                }
            }
            Message::CheckUpdate => {
                self.update = UpdateState::Checking;
                self.install_error = None;
                return Task::perform(update::check(), |result| {
                    Message::UpdateChecked(result.map_err(|err| format!("{err:#}")))
                });
            }
            Message::UpdateChecked(Ok(Some(release))) => {
                self.update = UpdateState::Available(release)
            }
            Message::UpdateChecked(Ok(None)) => self.update = UpdateState::Latest,
            Message::ReleaseNotes => {
                if let UpdateState::Available(release) = &self.update {
                    open(&release.page);
                }
            }
            Message::Update => {
                if let UpdateState::Available(release) = &self.update {
                    let release = release.clone();
                    self.update = UpdateState::Downloading(release.version.clone());
                    self.install_error = None;
                    return Task::perform(update::install(release), |result| {
                        Message::Updated(result.map_err(|err| format!("{err:#}")))
                    });
                }
            }
            // Continue as the new version, like after installing.
            Message::Updated(Ok(path)) => {
                install::relaunch_after_exit(&path);
                return iced::exit();
            }
            Message::UpdateChecked(Err(err)) | Message::Updated(Err(err)) => {
                tracing::error!("{err}");
                self.update = UpdateState::Unchecked;
                self.install_error = Some(err);
            }
            Message::OpenLogDir => open(logging::log_dir()),
        }
        Task::none()
    }

    fn view(&self, _window: window::Id) -> Element<'_, Message> {
        let pages = Page::ALL.map(|page| {
            button(text(page.title()))
                .width(Length::Fill)
                .style(if page == self.page { button::primary } else { button::text })
                .on_press(Message::Page(page))
                .into()
        });
        let sidebar = container(
            column![text("Freisprech").size(22), space().height(10)]
                .extend(pages)
                .spacing(4),
        )
        .width(SIDEBAR_WIDTH)
        .height(Length::Fill)
        .padding(12)
        .style(|theme: &Theme| container::background(theme.extended_palette().background.weak.color));

        let content = column![
            text(self.status.describe()),
            self.progress(),
            rule::horizontal(1),
            // Room for the scrollbar next to the page.
            scrollable(container(self.page()).padding(Padding::ZERO.right(14))).height(Length::Fill),
            row![space::horizontal()]
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
        .padding(20);

        row![sidebar, content].into()
    }

    fn page(&self) -> Element<'_, Message> {
        match self.page {
            Page::General => {
                let language = LANGUAGES
                    .iter()
                    .find(|l| l.code == self.config.language)
                    .copied();
                let microphone = Microphone(self.config.microphone.clone());
                column![
                    setting("Language", pick_list(LANGUAGES, language, Message::Language)),
                    setting(
                        "Microphone",
                        pick_list(self.microphones.as_slice(), Some(microphone), Message::Microphone)
                    ),
                    setting("Shortcut", text(self.hotkey_hint())),
                ]
            }
            Page::VoiceCommands => column![
                checkbox(self.config.voice_commands)
                    .label("Type spoken commands as line breaks and punctuation")
                    .on_toggle(Message::VoiceCommands),
                text(
                    "Say a phrase to type what's in the first column: \\n breaks the line, \
                     punctuation attaches to the previous word, anything else is typed as a \
                     word. Alternatives are separated by commas. With a fixed language only its \
                     column works, when detecting from speech all of them. Commands also \
                     trigger when you mean the word itself."
                )
                .size(12),
                self.command_table(),
                self.command_buttons(),
            ],
            Page::Vocabulary => column![
                text(
                    "Replaces words the model keeps getting wrong, in every language. Add each \
                     way it writes a word. Only whole words match; case and punctuation are \
                     ignored."
                )
                .size(12),
                self.vocabulary_table(),
                row![button("Add word").on_press(Message::AddWord)],
            ],
            Page::Installation => column![
                setting("Installation", self.installation()),
                setting("Autostart", self.autostart()),
                setting("Version", self.version()),
            ]
            .push(self.install_note())
            .push(row![button("Open log folder").on_press(Message::OpenLogDir)]),
        }
        .spacing(14)
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

    /// The voice commands, editable: what they type and what to say in each language.
    fn command_table(&self) -> Element<'_, Message> {
        let languages = &self.config.command_languages;
        let header = row![space().width(REMOVE_WIDTH), bold("Types").width(COMMAND_WIDTH)]
            .extend(languages.iter().map(|language| {
                let name = LANGUAGES
                    .iter()
                    .find(|l| l.code == language)
                    .map_or(language.as_str(), |l| l.name);
                row![
                    bold(name),
                    space::horizontal(),
                    remove_button(Message::RemoveCommandLanguage(language.clone())),
                ]
                .width(PHRASE_WIDTH)
                .align_y(Alignment::Center)
                .into()
            }))
            .spacing(TABLE_SPACING)
            .align_y(Alignment::Center);
        let rows = self.config.commands.iter().enumerate().map(|(i, command)| {
            row![
                remove_button(Message::RemoveCommand(i)),
                text_input("\\n or .", &command.output.replace('\n', "\\n"))
                    .id(command_id(i))
                    .width(COMMAND_WIDTH)
                    .on_input(move |output| Message::CommandOutput(i, output)),
            ]
            .extend(languages.iter().map(|language| {
                let phrase = command.phrases.get(language).map_or("", String::as_str);
                text_input("", phrase)
                    .width(PHRASE_WIDTH)
                    .on_input(move |phrase| Message::CommandPhrase(i, language.clone(), phrase))
                    .into()
            }))
            .spacing(TABLE_SPACING)
            .align_y(Alignment::Center)
            .into()
        });
        horizontal_scroll(column![header].extend(rows).spacing(TABLE_SPACING))
    }

    /// The vocabulary, editable: each word with the ways the model gets it wrong.
    fn vocabulary_table(&self) -> Element<'_, Message> {
        let header = row![
            space().width(REMOVE_WIDTH),
            bold("Types").width(WORD_WIDTH),
            bold("Recognized as"),
        ]
        .spacing(TABLE_SPACING);
        let rows = self.config.vocabulary.iter().enumerate().map(|(i, word)| {
            row![
                remove_button(Message::RemoveWord(i)),
                text_input("Kubernetes", &word.written)
                    .id(word_id(i))
                    .width(WORD_WIDTH)
                    .on_input(move |written| Message::VocabularyWritten(i, written)),
            ]
            .extend(word.heard.iter().enumerate().map(|(v, heard)| {
                text_input("cube netties", heard)
                    .id(heard_id(i, v))
                    .width(WORD_WIDTH)
                    .on_input(move |heard| Message::VocabularyHeard(i, v, heard))
                    .into()
            }))
            .push(button(text("+")).style(button::text).on_press(Message::AddHeard(i)))
            .spacing(TABLE_SPACING)
            .align_y(Alignment::Center)
            .into()
        });
        horizontal_scroll(column![header].extend(rows).spacing(TABLE_SPACING))
    }

    fn command_buttons(&self) -> Element<'_, Message> {
        let addable: Vec<Language> = LANGUAGES
            .iter()
            .filter(|l| ![language::DETECT, language::SYSTEM].contains(&l.code))
            .filter(|l| !self.config.command_languages.iter().any(|c| c == l.code))
            .copied()
            .collect();
        row![
            button("Add command").on_press(Message::AddCommand),
            pick_list(addable, None::<Language>, Message::AddCommandLanguage)
                .placeholder("Add language …"),
            space::horizontal(),
            button("Reset to defaults")
                .style(button::secondary)
                .on_press(Message::ResetCommands),
        ]
        .spacing(10)
        .into()
    }

    /// Where the app runs from, with the button to install or uninstall it.
    fn installation(&self) -> Element<'_, Message> {
        let (info, action) = if self.confirm_uninstall {
            (
                "Uninstall Freisprech?".to_owned(),
                row![
                    button("Uninstall")
                        .style(button::danger)
                        .on_press(Message::UninstallConfirmed),
                    button("Cancel").on_press(Message::UninstallCancelled),
                ]
                .spacing(10),
            )
        } else if self.install.running_installed {
            (
                format!("Installed in {}", home_relative(&self.install.running_from)),
                row![button("Uninstall").on_press(Message::Uninstall)],
            )
        } else {
            (
                format!("Running from {}", home_relative(&self.install.running_from)),
                row![button("Install").on_press(Message::Install)],
            )
        };
        row![text(info).width(Length::Fill), action]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
    }

    /// Autostart always runs the installed copy, so it needs one.
    fn autostart(&self) -> Element<'_, Message> {
        let label = if self.install.can_autostart {
            "Start at login"
        } else {
            "Start at login (install first)"
        };
        checkbox(self.install.autostart)
            .label(label)
            .on_toggle_maybe(self.install.can_autostart.then_some(Message::Autostart))
            .into()
    }

    /// The running version, with the buttons to check for and install a newer one.
    fn version(&self) -> Element<'_, Message> {
        let current = env!("CARGO_PKG_VERSION");
        let (info, action): (String, Element<'_, Message>) = match &self.update {
            UpdateState::Unchecked => (
                current.to_owned(),
                button("Check for updates").on_press(Message::CheckUpdate).into(),
            ),
            UpdateState::Checking => (current.to_owned(), button("Checking …").into()),
            UpdateState::Latest => (
                format!("{current} is the latest"),
                button("Check again").on_press(Message::CheckUpdate).into(),
            ),
            UpdateState::Available(release) => (
                format!("{} available", release.version),
                row![
                    button("What's new").on_press(Message::ReleaseNotes),
                    button("Update").style(button::success).on_press(Message::Update),
                ]
                .spacing(10)
                .into(),
            ),
            UpdateState::Downloading(version) => {
                (format!("Downloading {version} …"), button("Update").into())
            }
        };
        row![text(info).width(Length::Fill), action]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
    }

    fn install_note(&self) -> Option<Element<'_, Message>> {
        let note = if let Some(err) = &self.install_error {
            err.clone()
        } else if self.confirm_uninstall {
            "Removes the app, its menu entry and autostart. The model, settings and logs stay \
             (see README to remove them too)."
                .to_owned()
        } else {
            return None;
        };
        Some(text(note).size(12).into())
    }

    fn report(&mut self, result: anyhow::Result<()>) {
        self.install_error = result.err().map(|err| {
            tracing::error!("{err:#}");
            format!("{err:#}")
        });
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
        self.install = install::State::read();
        self.confirm_uninstall = false;
        self.install_error = None;

        let (id, open) = window::open(window::Settings {
            size: Size::new(780.0, 500.0),
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

    /// Hands the settings to the dictation and saves them.
    fn apply_config(&mut self) {
        self.edit_config();
        self.save_config();
    }

    /// For typing into a field: the dictation gets every change, the file is written when
    /// the window closes.
    fn edit_config(&mut self) {
        self.config_tx.send_replace(self.config.clone());
        self.unsaved = true;
    }

    fn save_config(&mut self) {
        if !self.unsaved {
            return;
        }
        self.unsaved = false;
        if let Err(err) = self.config.save() {
            tracing::error!("Failed to save settings: {err:#}");
        }
    }

    /// When the window closes: drops empty table rows and fields, then saves.
    fn tidy_and_save_config(&mut self) {
        let before = self.config.clone();
        for word in &mut self.config.vocabulary {
            word.heard.retain(|heard| !heard.trim().is_empty());
        }
        self.config
            .vocabulary
            .retain(|word| !word.written.trim().is_empty() || !word.heard.is_empty());
        self.config
            .commands
            .retain(|command| !command.output.is_empty() || !command.phrases.is_empty());
        if self.config != before {
            self.edit_config();
        }
        self.save_config();
    }

    fn exit(&mut self) -> Task<Message> {
        self.tidy_and_save_config();
        iced::exit()
    }
}

/// Lets a table grow wider than the window. Nothing inside may fill the width: it is
/// unbounded here.
fn horizontal_scroll<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    scrollable(content).horizontal().spacing(6).into()
}

fn command_id(index: usize) -> widget::Id {
    widget::Id::from(format!("command-{index}"))
}

fn word_id(index: usize) -> widget::Id {
    widget::Id::from(format!("word-{index}"))
}

fn heard_id(index: usize, variant: usize) -> widget::Id {
    widget::Id::from(format!("heard-{index}-{variant}"))
}

fn bold<'a>(label: &'a str) -> text::Text<'a> {
    text(label).font(Font {
        weight: font::Weight::Bold,
        ..Font::DEFAULT
    })
}

/// Small "×" to delete a row or column.
fn remove_button<'a>(message: Message) -> Element<'a, Message> {
    button(text("×").center())
        .style(button::text)
        .width(REMOVE_WIDTH)
        .padding([2, 0])
        .on_press(message)
        .into()
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

/// `~/…` for paths in the home folder, as Linux users know them.
fn home_relative(path: &std::path::Path) -> String {
    let home = dirs::home_dir().filter(|_| cfg!(target_os = "linux"));
    match home.as_deref().and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => std::path::Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

/// Opens a folder in the file manager or a web page in the browser.
fn open(target: impl AsRef<std::ffi::OsStr>) {
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    if let Err(err) = std::process::Command::new(program).arg(target).spawn() {
        tracing::error!(%err, "Failed to open folder or web page");
    }
}

/// Where the update check in the settings stands. Failures show as `install_error`.
#[derive(Debug, Clone)]
enum UpdateState {
    Unchecked,
    Checking,
    Latest,
    Available(update::Release),
    /// Downloading and installing this version.
    Downloading(String),
}

/// Pages of the settings window, listed in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Page {
    General,
    VoiceCommands,
    Vocabulary,
    Installation,
}

impl Page {
    const ALL: [Page; 4] = [
        Page::General,
        Page::VoiceCommands,
        Page::Vocabulary,
        Page::Installation,
    ];

    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::VoiceCommands => "Voice commands",
            Page::Vocabulary => "Vocabulary",
            Page::Installation => "Installation",
        }
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
    Language { code: language::DETECT, name: "Detect from speech" },
    Language { code: language::SYSTEM, name: "System language" },
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

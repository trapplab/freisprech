# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] - 2026-09-28

### Added

- Update check in the settings: installs a newer release and restarts.

## [0.2.0] - 2026-09-28

### Added

- `--record <file>`: dictate and save the audio for `--file`.
- `--language <code>` for `--file` and `--record`.
- *System language* as an option in the language setting.
- Microphone levels in the log.

### Changed

- Quiet microphones are amplified.
- Silent channels are no longer mixed in.

### Fixed

- The end of the last word was cut off when stopping.

## [0.1.1] - 2026-09-27

### Added

- **Install** in the settings copies the app to a fixed location (Linux `~/.local/bin`,
  Windows `%LOCALAPPDATA%\Programs\Freisprech`), adds it to the app menu and restarts it
  from there. On Windows it also appears under *Installed apps*.
- **Start at login** in the settings, once the app is installed.
- **Uninstall** in the settings and the `--uninstall` option. The model, settings and logs
  stay.

### Changed

- On Linux the menu entry points to the installed copy if there is one.

## [0.1.0] - 2026-09-27

### Added

- First release: offline dictation into any app with **Ctrl+Alt+D**, using Foundry Local
  and NVIDIA Nemotron ASR, typed live while speaking.
- Tray icon and settings window with language and microphone.
- Linux (Wayland, via the XDG portals) and Windows single-file binaries.

[Unreleased]: https://github.com/trapplab/freisprech/compare/0.3.0...HEAD
[0.3.0]: https://github.com/trapplab/freisprech/compare/0.2.0...0.3.0
[0.2.0]: https://github.com/trapplab/freisprech/compare/0.1.1...0.2.0
[0.1.1]: https://github.com/trapplab/freisprech/compare/0.1.0...0.1.1
[0.1.0]: https://github.com/trapplab/freisprech/releases/tag/0.1.0

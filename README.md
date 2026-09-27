# Freisprech

**Dictate into any app with local AI.**

Local dictation assistant for Linux and Windows. **Ctrl+Alt+D** starts and stops dictation. Speech is recognized **offline** with [Foundry Local](https://github.com/microsoft/foundry-local) and NVIDIA Nemotron ASR, and typed into the focused text field while you are still speaking. The app runs in the background with a tray icon.

## Usage

1. Download `freisprech-linux-x86_64`, make it executable and run it. No installation needed.
2. On first start:
   - On Linux, confirm two desktop dialogs: the keyboard shortcut and permission to type.
   - The model (about 760 MB) is downloaded once. Progress is shown in the tray tooltip, the settings window and the console.
3. **Ctrl+Alt+D** starts dictation, pressing **Ctrl+Alt+D** again stops it.

Settings (language, microphone) are available from the tray icon, and on Linux also by starting the program a second time or from the app menu. Quit via the tray menu, or without a tray via the button in the settings window.

| Option | Purpose |
|---|---|
| `--settings` | Open the settings window on start |
| `--toggle` | Linux only: start/stop dictation in the running instance (fallback if the desktop offers no global shortcut) |
| `--file <pcm>` | Test: transcribe a file (16 kHz, mono, s16le) |
| `--type-text <text>` | Test: type text without speech recognition |

## System requirements

| System | Status |
|---|---|
| KDE Plasma 6 (Wayland) | works, tested on Fedora 44 |
| Ubuntu 26.04, Debian 13 (GNOME ≥ 48, Wayland) | should work, untested |
| Ubuntu 24.04, Debian 12 and older | not supported: they lack the GlobalShortcuts portal or keyboard input via libei |
| Windows | compiles, untested |

The Linux binary needs glibc ≥ 2.35. Debian's GNOME has no tray by default.

## Files

| What | Linux | Windows |
|---|---|---|
| Settings | `~/.config/freisprech/config.toml` | `%APPDATA%\freisprech\` |
| Logs | `~/.local/share/freisprech/logs/` | `%LOCALAPPDATA%\freisprech\logs\` |
| Native libraries (unpacked by the app) | `~/.local/share/freisprech/native/` | `%LOCALAPPDATA%\freisprech\native\` |
| Model cache | `~/.freisprech/` | `%USERPROFILE%\.freisprech\` |
| Desktop entry (created by the app) | `~/.local/share/applications/io.github.trapplab.Freisprech.desktop` | – |

## Development

```bash
scripts/fetch-native-nightly.sh     # native Foundry engine into native/ (Windows: .ps1)
cargo run --release                 # development, uses native/ in the project
scripts/build-linux.sh              # release: single file in dist/, built in a Debian 12 container
cargo build --release --features bundle-native   # release for Windows, run on Windows
```

On Fedora the build needs `alsa-lib-devel` and `libxkbcommon-devel`, and `build-linux.sh` additionally needs `podman`.

The `bundle-native` feature embeds the native libraries (86 MB) zstd-compressed into the binary (about 50 MB in total). They are unpacked on first start.

## Release

Bump the version in `Cargo.toml`, commit, and push a matching tag:

```bash
git tag 0.2.0 && git push origin 0.2.0
```

Or let an agent do all of it: the skill [`bump-version`](.agents/skills/bump-version/SKILL.md) takes the version, updates `Cargo.toml` and `Cargo.lock`, verifies the build, commits, and pushes `main` and the tag.

The workflow [`.github/workflows/release.yml`](.github/workflows/release.yml) then builds the Linux and Windows binaries and creates a GitHub release with `SHA256SUMS` and automatic release notes. It aborts if the tag does not match `Cargo.toml`. Tags with a suffix such as `0.2.0-rc1` are marked as pre-releases. The workflow can also be started manually under *Actions → Release → Run workflow*; it then only builds (results as artifacts) without creating a release.

## Notes

- **Nightly runtime:** Foundry Local 2.0.1 ignores the language setting when streaming ([Foundry-Local#1064](https://github.com/microsoft/Foundry-Local/issues/1064)). The project therefore uses a nightly build for now. Switch back once a release contains the fix.
- **Linux:** The shortcut and typing go through the XDG portals (GlobalShortcuts, RemoteDesktop + libei). Only one instance runs at a time: starting a second one opens the settings of the first.

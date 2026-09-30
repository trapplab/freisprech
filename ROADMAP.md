# Roadmap

## 1.0: must have

- [ ] Test Windows for real, or mark it experimental (Ctrl+Alt counts as AltGr there)
- [ ] Switch from the nightly Foundry runtime to a stable release ([Foundry-Local#1064](https://github.com/microsoft/Foundry-Local/issues/1064))
- [ ] Don't drop characters the keyboard layout can't reach (`„“ – … é`): fall back to a replacement character or the clipboard, or at least tell the user
- [ ] Recording feedback without a tray (GNOME): start/stop sound, notification or overlay
- [ ] Show errors visibly, not only in the tray tooltip
- [ ] Stop automatically after N seconds of silence or at a maximum duration
- [ ] Configurable shortcut on Windows; on Linux, document how to change it in the system settings

## 1.0: demo and testing

- [ ] Test text field in the settings window ("click here, press Ctrl+Alt+D")
- [ ] Microphone level meter in the settings
- [ ] "Copy diagnostics" button (version, OS, desktop, portals, microphone, recent log lines)
- [ ] GitHub issue template that asks for the diagnostics
- [ ] Manual test matrix `docs/testing.md` (desktops × apps × layouts × languages)
- [ ] German test recording next to `jfk.pcm` (umlauts, ß, punctuation)
- [ ] CI on push/PR: `cargo check` and `clippy` for both targets
- [ ] Unit tests: `lookup` in the keymap (de/us, special characters), `LiveOutput::flush`
- [ ] GIF or short video in the README

## After 1.0: polish text with a local LLM

- [ ] Prototype: extend `--file` to run the transcript through a small LLM and print the original and the result side by side
- [ ] Compare models on German recordings (quality, latency, RAM)
- [ ] Opt-in "Polished" mode: no live typing, correct the text after stopping, then type it all
- [ ] Guard rails: strict prompt, temperature 0, type the original if the output differs too much
- [ ] Remove filler words, resolve self-corrections, fix grammar and misheard words

## Later

- [x] Voice commands ("new paragraph", "full stop")
- [ ] Push-to-talk as an alternative to toggling
- [x] Custom vocabulary
- [ ] Choice of models
- [ ] Update notice (needs an HTTP client, ask first)

# AGENTS.md

Local dictation assistant in Rust (Linux Wayland + Windows). Setup and usage: `README.md`.

## Check that the project builds

After every code change; all commands must succeed:

```sh
cargo check --locked                                   # Linux
cargo check --locked --target x86_64-pc-windows-msvc   # Windows code, works without MSVC
cargo clippy --locked                                  # don't introduce new warnings
```

Platform code lives in `src/{hotkey,tray,typer}/`, split per platform. When changing one
side, check both targets.

Check speech recognition (downloads the model on first run, about 760 MB):

```sh
cargo run --locked -- --file tests/fixtures/jfk.pcm --language en
```

The output must match the expected text in `tests/fixtures/README.md` (punctuation may differ).

Only when asked:
- `--type-text` (types into whatever window has focus after 3 s)
- `scripts/build-linux.sh` (release build, slow, needs podman or docker)

## Conventions

- Everything in English: code, comments, UI, log and error messages, docs
- Don't run `cargo fmt` over the whole repo unasked; it isn't fully formatted yet
- No new dependencies without asking
- Keep versions in `scripts/fetch-native-nightly.sh` and `.ps1` in sync

## Documentation

- Update `README.md` in the same change when usage, CLI options, requirements or build steps
  change
- Add user-visible changes to `CHANGELOG.md` under `## [Unreleased]` in the same change
  ([Keep a Changelog](https://keepachangelog.com): Added, Changed, Fixed, Removed …).
  Releasing turns that section into the version (skill `bump-version`)
- Keep the changelog messages short and avoid beeing too verbose

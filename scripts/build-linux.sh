#!/usr/bin/env bash
# Builds the single-file Linux release (native engine embedded) inside a Debian 12
# container: linked against its glibc 2.36 instead of the host's, the binary also runs on
# Debian 12+ and Ubuntu 23.04+. Result: dist/freisprech-linux-x86_64
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE=docker.io/library/rust:1-bookworm
ENGINE=$(command -v podman || command -v docker) || { echo "podman or docker is missing" >&2; exit 1; }

[ -d "$ROOT/native" ] || "$ROOT/scripts/fetch-native-nightly.sh"

# Crates come from the host's cargo cache, so the container build needs no crates.io.
(cd "$ROOT" && cargo fetch --locked)

# Separate target dir, so host builds (newer glibc) and container builds don't mix.
"$ENGINE" run --rm \
  --security-opt label=disable \
  -v "$ROOT":/src -w /src \
  -v "${CARGO_HOME:-$HOME/.cargo}/registry":/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/src/target/bookworm \
  "$IMAGE" bash -euc '
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends pkg-config libasound2-dev libxkbcommon-dev >/dev/null
    cargo build --release --offline --locked --features bundle-native'

mkdir -p "$ROOT/dist"
OUT="$ROOT/dist/freisprech-linux-x86_64"
cp "$ROOT/target/bookworm/release/freisprech" "$OUT"
GLIBC=$(objdump -T "$OUT" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
echo "Done: $OUT ($(du -h "$OUT" | cut -f1), needs $GLIBC)"

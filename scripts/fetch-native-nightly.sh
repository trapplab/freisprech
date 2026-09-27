#!/usr/bin/env bash
# Downloads a Foundry Local nightly runtime plus matching ONNX Runtime / GenAI into native/.
# Needed until a release contains the fix for streaming language selection
# (microsoft/Foundry-Local#1064); v2.0.1 ignores `language` when streaming.
set -euo pipefail

FOUNDRY=2.0.0-dev.202609240625
ORT=1.30.0
GENAI=0.16.0
RID=${RID:-linux-x64}

DIR="$(cd "$(dirname "$0")/.." && pwd)/native"
NUGET=https://api.nuget.org/v3-flatcontainer
NIGHTLY=https://pkgs.dev.azure.com/aiinfra/PublicPackages/_packaging/ORT-Nightly/nuget/v3/flat2

fetch() { # <feed> <package id> <version>
  local tmp
  tmp=$(mktemp)
  echo "Downloading $2 $3 …"
  curl -fsSL -o "$tmp" "$1/$2/$3/$2.$3.nupkg"
  unzip -o -q -j "$tmp" "runtimes/$RID/native/*" -d "$DIR"
  rm -f "$tmp"
}

mkdir -p "$DIR"
fetch "$NIGHTLY" microsoft.ai.foundry.local.runtime "$FOUNDRY"
fetch "$NUGET" microsoft.ml.onnxruntime "$ORT"
fetch "$NUGET" microsoft.ml.onnxruntimegenai.foundry "$GENAI"
ls -l "$DIR"

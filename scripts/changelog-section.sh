#!/bin/sh
# Prints the entries of one version in CHANGELOG.md, without its heading, e.g. as release
# notes. Fails if there are none. Usage: scripts/changelog-section.sh 0.2.0 (or Unreleased)
set -eu

version=$1
notes=$(awk -v v="$version" '
    /^## \[/ { found = index($0, "## [" v "]") == 1; next }
    /^\[[^]]+\]: / { found = 0 }
    found
' "$(dirname "$0")/../CHANGELOG.md" | sed '/./,$!d')

if [ -z "$notes" ]; then
    echo "CHANGELOG.md has no entries for $version" >&2
    exit 1
fi
printf '%s\n' "$notes"

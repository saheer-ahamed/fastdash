#!/usr/bin/env bash
# Bump every place the app version lives, keeping them in sync. This is the one
# source of truth for a version bump - CI calls it, and you can run it locally.
#
#   scripts/bump-version.sh 0.1.1
#
# It does NOT commit or tag; the caller does that.
set -euo pipefail

VERSION="${1:?usage: bump-version.sh <x.y.z>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "Bumping to $VERSION"

# package.json + package-lock.json
npm version "$VERSION" --no-git-tag-version --allow-same-version >/dev/null

# tauri.conf.json (structured edit so we never touch the wrong "version")
tmp="$(mktemp)"
jq --arg v "$VERSION" '.version = $v' src-tauri/tauri.conf.json > "$tmp"
mv "$tmp" src-tauri/tauri.conf.json

# Cargo.toml: the first `version = ` at column 0 is the [package] version.
sed -i "0,/^version = \".*\"/s//version = \"$VERSION\"/" src-tauri/Cargo.toml

# Cargo.lock: only the fastdash package entry, not any dependency named similarly.
awk -v v="$VERSION" '
  /^name = "fastdash"$/ { print; getline; sub(/version = ".*"/, "version = \"" v "\""); print; next }
  { print }
' src-tauri/Cargo.lock > src-tauri/Cargo.lock.tmp
mv src-tauri/Cargo.lock.tmp src-tauri/Cargo.lock

echo "Done. package.json / tauri.conf.json / Cargo.toml / Cargo.lock now at $VERSION"

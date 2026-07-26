#!/usr/bin/env bash
#
# Single source of truth for a version bump.
#
#   scripts/bump-version.sh 1.2.3
#
# Updates every file that carries the app version so a release is internally
# consistent - the installer, the crate, and the updater config all agree.
# Called by .github/workflows/release.yml, but safe to run locally (it only
# edits files; it never commits, tags, or pushes).
#
# Files touched:
#   package.json           - "version"
#   package-lock.json      - root + packages[""] "version"  (via npm)
#   src-tauri/tauri.conf.json - top-level "version"
#   src-tauri/Cargo.toml   - [package] version
#   src-tauri/Cargo.lock   - the fastdash package entry
set -euo pipefail

VERSION="${1:?usage: bump-version.sh X.Y.Z}"

# Reject anything that is not a bare semver (a leading "v" is a common slip and
# would desync the tag from the manifests).
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: version must be X.Y.Z with no leading v (got '$VERSION')" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# package.json + package-lock.json: npm keeps the two in sync and touches only
# the root package, never dependency entries that happen to share the version.
npm version "$VERSION" --no-git-tag-version --allow-same-version >/dev/null

# tauri.conf.json: replace only the first "version" (the top-level one). Editing
# the string in place keeps the file's formatting untouched.
perl -0pi -e 's/("version"\s*:\s*")[0-9]+\.[0-9]+\.[0-9]+(")/${1}'"$VERSION"'${2}/' \
  src-tauri/tauri.conf.json

# Cargo.toml: the [package] version. Anchored to the [package] table so a
# version pin in [dependencies] can never be hit by accident.
perl -0pi -e 's/(\[package\][^\[]*?\nversion\s*=\s*")[0-9]+\.[0-9]+\.[0-9]+(")/${1}'"$VERSION"'${2}/s' \
  src-tauri/Cargo.toml

# Cargo.lock: only the fastdash package's own entry (matched by its name line),
# never a dependency that happens to be on the same version.
perl -0pi -e 's/(name = "fastdash"\nversion = ")[0-9]+\.[0-9]+\.[0-9]+(")/${1}'"$VERSION"'${2}/' \
  src-tauri/Cargo.lock

echo "bumped version to $VERSION"

#!/usr/bin/env bash
#
# Single source of truth for a version bump.
#
#   scripts/bump-version.sh 1.2.3
#
# Updates every file that carries the app version so a release is internally
# consistent - the installer, every crate, and the update manifest all agree.
# Called by .github/workflows/release.yml, but safe to run locally (it only
# edits files; it never commits, tags, or pushes).
#
# Files touched:
#   package.json           - "version"
#   package-lock.json      - root + packages[""] "version"  (via npm)
#   Cargo.toml             - [workspace.package] version (every crate inherits it)
#   Cargo.lock             - the workspace crates' own entries
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

# Cargo.toml: the [workspace.package] version, which every crate inherits.
# Anchored to that table so a version pin in a dependency can never be hit.
perl -0pi -e 's/(\[workspace\.package\][^\[]*?\nversion\s*=\s*")[0-9]+\.[0-9]+\.[0-9]+(")/${1}'"$VERSION"'${2}/s' \
  Cargo.toml

# Cargo.lock: only the workspace crates' own entries (matched by their name
# lines), never a dependency that happens to be on the same version.
perl -0pi -e 's/(name = "(?:fastdash(?:-[a-z]+)?|xtask)"\nversion = ")[0-9]+\.[0-9]+\.[0-9]+(")/${1}'"$VERSION"'${2}/g' \
  Cargo.lock

echo "bumped version to $VERSION"

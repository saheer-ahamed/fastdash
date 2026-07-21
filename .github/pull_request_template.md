<!--
Title MUST be a Conventional Commit, e.g.  feat(ui): add developer mode toggle
Branch MUST be  type/short-description,   e.g.  feat/dev-mode
CI (PR checks) will block the merge otherwise. See CLAUDE.md > Conventions.
-->

## What

<!-- One or two sentences on what this changes and why. -->

## Release impact

<!-- Tick the type that matches the PR title. This drives the auto version bump. -->

- [ ] `fix:` - patch release
- [ ] `feat:` - minor release
- [ ] `feat!:` / `BREAKING CHANGE:` - major release
- [ ] `chore/docs/refactor/style/test/build/ci` - no release

## Checklist

- [ ] Title follows Conventional Commits
- [ ] Branch is named `type/short-description`
- [ ] `npm run lint` and `npm run typecheck` pass; Rust is `fmt` + `clippy` clean

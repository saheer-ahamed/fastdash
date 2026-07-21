// Conventional Commits, enforced. These messages are not just style - the
// release workflow parses them to decide the next version:
//   fix:            -> patch   (0.1.0 -> 0.1.1)
//   feat:           -> minor   (0.1.0 -> 0.2.0)
//   feat!: / footer -> major   (0.1.0 -> 1.0.0)  (BREAKING CHANGE)
// Anything else (chore/docs/refactor/style/test/build/ci/perf) ships no release.
//
// A commit that fails this is rejected locally by .husky/commit-msg, and a PR
// whose title fails is rejected in CI by .github/workflows/pr-checks.yml.
export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    // The type must be one of these - keep in sync with CLAUDE.md.
    "type-enum": [
      2,
      "always",
      [
        "feat", // new user-facing capability          (minor)
        "fix", // bug fix                              (patch)
        "perf", // performance improvement              (patch)
        "refactor", // behavior-preserving code change
        "style", // formatting only (fmt, prettier, eslint)
        "docs", // documentation only
        "test", // tests only
        "build", // build system, deps, bundling
        "ci", // CI / workflows
        "chore", // maintenance, releases, tooling
        "revert", // reverts a previous commit
      ],
    ],
    // Lower-case first letter (matches the PR-title check), imperative mood.
    "subject-case": [
      2,
      "never",
      ["sentence-case", "start-case", "pascal-case", "upper-case"],
    ],
    "subject-empty": [2, "never"],
    "subject-full-stop": [2, "never", "."],
    "header-max-length": [2, "always", 100],
  },
};

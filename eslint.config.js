// ESLint flat config (ESLint 9). Lints the React + TypeScript frontend in src/.
// The Rust side is covered separately by cargo fmt + clippy (see .husky/pre-commit).
import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";

export default tseslint.config(
  // Build output, deps, and Rust artifacts are never linted.
  { ignores: ["dist", "node_modules", "src-tauri/target"] },
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      // The classic, stable hooks rules. We deliberately do not enable the
      // plugin's newer React-Compiler ruleset (refs/immutability/
      // set-state-in-effect): it flags intentional, documented patterns in this
      // codebase (e.g. the read-latest ref writes in App/Settings), and churning
      // that working logic is out of scope for the lint gate.
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // Fast-Refresh boundary: a module exporting a component should export only
      // components. `allowConstantExport` keeps constant exports (e.g. catalogs) legal.
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
    },
  },
  // Config files run in Node, not the browser.
  {
    files: ["*.config.{js,ts}"],
    languageOptions: { globals: globals.node },
  },
);

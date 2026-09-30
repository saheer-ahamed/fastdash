use std::fs;
use std::path::Path;

fn main() {
    // Re-embed the Windows app icon whenever the icon files change. cargo does
    // not track them as build inputs by default, so without this an icon swap
    // relinks the exe with the STALE icon resource.
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=icons/icon.ico");

    embed_build_env();

    // A custom Windows manifest, for its compatibility section: see the file.
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    let windows = tauri_build::WindowsAttributes::new()
        .app_manifest(include_str!("windows-app-manifest.xml"));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}

/// Bake env-file config into the binary at build time so it ships to end users.
/// cargo does not read `.env`, so we do it here: for each key, an already
/// exported process env var (e.g. from CI) wins; otherwise the value comes from
/// the repo-root `.env`. Absent both, `option_env!` sees nothing and the app
/// reports "not configured" - the expected state for contributor builds.
fn embed_build_env() {
    // build.rs runs in `src-tauri/`; the project `.env` sits one level up.
    let dotenv = Path::new("../.env");
    println!("cargo:rerun-if-changed=../.env");

    // Kept as a list so more build-env keys can be added here later; the single
    // element today is intentional, so silence clippy's single-element-loop lint.
    #[allow(clippy::single_element_loop)]
    for key in ["FASTDASH_GITHUB_CLIENT_ID"] {
        println!("cargo:rerun-if-env-changed={key}");
        let value = std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| read_dotenv_value(dotenv, key));
        if let Some(v) = value {
            println!("cargo:rustc-env={key}={}", v.trim());
        }
    }
}

/// Read `KEY=value` for `key` from a `.env` file, tolerating `export ` prefixes,
/// `#` comments, blank lines, and single/double quotes around the value.
fn read_dotenv_value(path: &Path, key: &str) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, val)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let val = val.trim();
        let val = val
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| val.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(val);
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

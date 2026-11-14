//! UI-only preferences: the theme, developer mode, and which guided tours have
//! run. They describe this machine's UI, not the user's settings, so they live
//! in their own `ui.toml` beside fastdash-core's `config.toml` rather than in
//! `AppConfig`. Loading never fails (defaults stand in); saving is atomic.

use std::collections::BTreeSet;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use directories::ProjectDirs;
use gpui::{App, Global};
use serde::{Deserialize, Serialize};

use crate::theme::ThemeChoice;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    pub theme: ThemeChoice,
    /// Unlocked by tapping the version in Settings five times. Shows technical
    /// error detail that everyday users never see.
    pub dev_mode: bool,
    /// Walkthroughs that have run to completion or been skipped.
    pub tours_seen: BTreeSet<String>,
}

impl Global for Prefs {}

#[cfg(not(target_arch = "wasm32"))]
fn path() -> Option<PathBuf> {
    ProjectDirs::from("co", "zro", "fastdash").map(|d| d.config_dir().join("ui.toml"))
}

pub fn load() -> Prefs {
    #[cfg(target_arch = "wasm32")]
    {
        return fastdash_core::engine::browser_store::get("fastdash.ui")
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }
}

fn save(prefs: &Prefs) -> std::io::Result<()> {
    #[cfg(target_arch = "wasm32")]
    {
        let text = toml::to_string_pretty(prefs).map_err(std::io::Error::other)?;
        return fastdash_core::engine::browser_store::set("fastdash.ui", &text)
            .map_err(std::io::Error::other);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = path() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(prefs).map_err(std::io::Error::other)?;
        // Write-then-rename, so a crash mid-write can never leave a torn file.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

pub fn get(cx: &App) -> &Prefs {
    cx.global::<Prefs>()
}

/// Change the preferences and persist them. A failed write is logged and the
/// in-memory change kept: losing a theme choice is not worth interrupting for.
pub fn update(cx: &mut App, f: impl FnOnce(&mut Prefs)) {
    let prefs = cx.global_mut::<Prefs>();
    f(prefs);
    if let Err(err) = save(prefs) {
        eprintln!("fastdash: could not save UI preferences: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_or_partial_file_loads_with_defaults() {
        let prefs: Prefs = toml::from_str("devMode = true").unwrap();
        assert!(prefs.dev_mode);
        assert_eq!(prefs.theme, ThemeChoice::System);
        assert!(prefs.tours_seen.is_empty());
    }

    #[test]
    fn prefs_round_trip() {
        let mut prefs = Prefs {
            theme: ThemeChoice::Amber,
            ..Default::default()
        };
        prefs.tours_seen.insert("github".into());
        let back: Prefs = toml::from_str(&toml::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(back.theme, ThemeChoice::Amber);
        assert!(back.tours_seen.contains("github"));
    }
}

//! Themes: one flat palette per theme, the only place colours are defined.
//!
//! Every view reads colours from the active [`Theme`] (a GPUI global), never
//! from literals, so adding a theme is one entry in [`ThemeChoice`] and one
//! palette in [`palette`]. "System" follows the OS light/dark setting.

use gpui::{rgb, App, Global, Hsla, WindowAppearance};
use serde::{Deserialize, Serialize};

/// A resolved palette.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Hsla,
    pub panel: Hsla,
    pub panel_2: Hsla,
    pub border: Hsla,
    pub border_bright: Hsla,
    pub text: Hsla,
    pub bright: Hsla,
    pub muted: Hsla,
    pub fill: Hsla,
    pub idle: Hsla,
    pub ok: Hsla,
    pub warn: Hsla,
    pub err: Hsla,
}

impl Global for Theme {}

impl Theme {
    /// `fill` mixed toward `panel` by `amount` (0 = panel, 1 = fill), the way
    /// the heatmap shades its levels.
    pub fn fill_mix(&self, amount: f32) -> Hsla {
        mix(self.panel, self.fill, amount)
    }
}

/// What the user picked in Settings. Persisted in the UI preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Dark,
    Light,
    Midnight,
    Amber,
    Green,
    Paper,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 7] = [
        ThemeChoice::System,
        ThemeChoice::Dark,
        ThemeChoice::Light,
        ThemeChoice::Midnight,
        ThemeChoice::Amber,
        ThemeChoice::Green,
        ThemeChoice::Paper,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
            ThemeChoice::Midnight => "Midnight",
            ThemeChoice::Amber => "Amber",
            ThemeChoice::Green => "Green",
            ThemeChoice::Paper => "Paper",
        }
    }

    /// The concrete palette for this choice under the OS `appearance`.
    pub fn resolve(self, appearance: WindowAppearance) -> Theme {
        let light = matches!(
            appearance,
            WindowAppearance::Light | WindowAppearance::VibrantLight
        );
        match self {
            ThemeChoice::System if light => palette(ThemeChoice::Light),
            ThemeChoice::System => palette(ThemeChoice::Dark),
            other => palette(other),
        }
    }
}

/// Install the theme for `choice` as the global every view reads.
pub fn apply(choice: ThemeChoice, appearance: WindowAppearance, cx: &mut App) {
    cx.set_global(choice.resolve(appearance));
    cx.refresh_windows();
}

/// The active theme. Panics only if called before [`apply`], which `main`
/// does before opening any window.
pub fn current(cx: &App) -> Theme {
    *cx.global::<Theme>()
}

#[allow(clippy::too_many_arguments)]
fn theme(
    bg: u32,
    panel: u32,
    panel_2: u32,
    border: u32,
    border_bright: u32,
    text: u32,
    bright: u32,
    muted: u32,
    fill: u32,
    idle: u32,
    ok: u32,
    warn: u32,
    err: u32,
) -> Theme {
    Theme {
        bg: rgb(bg).into(),
        panel: rgb(panel).into(),
        panel_2: rgb(panel_2).into(),
        border: rgb(border).into(),
        border_bright: rgb(border_bright).into(),
        text: rgb(text).into(),
        bright: rgb(bright).into(),
        muted: rgb(muted).into(),
        fill: rgb(fill).into(),
        idle: rgb(idle).into(),
        ok: rgb(ok).into(),
        warn: rgb(warn).into(),
        err: rgb(err).into(),
    }
}

fn palette(choice: ThemeChoice) -> Theme {
    match choice {
        ThemeChoice::System | ThemeChoice::Dark => theme(
            0x0a0b0c, 0x0e0f11, 0x17191c, 0x2a2d31, 0x3a3e44, 0xc8ccd1, 0xe8ebee, 0x6a7078,
            0x8a9198, 0x3a3e44, 0x9aa1a8, 0xb28f3a, 0xc0564a,
        ),
        ThemeChoice::Light => theme(
            0xf6f6f3, 0xffffff, 0xececea, 0xdcdcd6, 0xc4c4bd, 0x2b2d30, 0x0d0e10, 0x7c7f84,
            0x565b61, 0xc4c4bd, 0x4d5157, 0x8a6a1e, 0xa83a2b,
        ),
        ThemeChoice::Midnight => theme(
            0x0a0e17, 0x0d1220, 0x141c2e, 0x1e2940, 0x2d3c5a, 0xa9b7d4, 0xe3ebfa, 0x5c6d8c,
            0x5b7ec4, 0x2d3c5a, 0x6f8ac2, 0xc79a3e, 0xd05a5a,
        ),
        ThemeChoice::Amber => theme(
            0x0a0700, 0x100b02, 0x1b1405, 0x3a2c10, 0x574417, 0xd99a2b, 0xffbe4d, 0x7d6024,
            0xe0952a, 0x3a2c10, 0xe0952a, 0xffbe4d, 0xff6a4a,
        ),
        ThemeChoice::Green => theme(
            0x000700, 0x021102, 0x051b05, 0x0f3a12, 0x17571c, 0x33cc46, 0x5cff6f, 0x1f6a2a,
            0x33cc46, 0x0f3a12, 0x33cc46, 0xcbb23a, 0xff5a5a,
        ),
        ThemeChoice::Paper => theme(
            0xf4f1ea, 0xfbf9f3, 0xeae4d8, 0xd9d1c1, 0xc3b9a2, 0x34302a, 0x14120d, 0x867c68,
            0x6b5f45, 0xc3b9a2, 0x5a5140, 0x8a6a1e, 0xa8402b,
        ),
    }
}

/// Linear sRGB mix of two opaque colours, like CSS `color-mix(in srgb, ...)`.
fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let a = from.to_rgb();
    let b = to.to_rgb();
    let lerp = |x: f32, y: f32| x + (y - x) * amount;
    gpui::Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: 1.0,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_follows_the_os_appearance() {
        let light = ThemeChoice::System.resolve(WindowAppearance::Light);
        let dark = ThemeChoice::System.resolve(WindowAppearance::Dark);
        assert_eq!(light.bg, palette(ThemeChoice::Light).bg);
        assert_eq!(dark.bg, palette(ThemeChoice::Dark).bg);
    }

    #[test]
    fn a_choice_round_trips_through_its_stored_name() {
        for choice in ThemeChoice::ALL {
            let stored = toml::Value::try_from(choice).unwrap();
            assert_eq!(stored.clone().try_into::<ThemeChoice>().unwrap(), choice);
        }
    }

    #[test]
    fn mixing_reaches_both_ends() {
        let t = palette(ThemeChoice::Dark);
        assert_eq!(t.fill_mix(1.0).to_rgb().r, t.fill.to_rgb().r);
        assert_eq!(t.fill_mix(0.0).to_rgb().r, t.panel.to_rgb().r);
    }
}

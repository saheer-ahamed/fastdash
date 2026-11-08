//! Assets compiled into the binary, so the app is one self-contained exe.

use std::borrow::Cow;
use std::time::Duration;

use gpui::{
    percentage, prelude::*, px, svg, Animation, AnimationExt, AssetSource, Hsla, SharedString,
    Transformation,
};

/// Every asset, by the path views ask for it under.
const ASSETS: &[(&str, &[u8])] = &[(
    "icons/spinner.svg",
    include_bytes!("../resources/icons/spinner.svg"),
)];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(ASSETS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(ASSETS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

/// The small circular loading indicator that spins inside a button while a
/// background refresh runs (the data on screen stays put).
pub fn spinner(color: Hsla) -> impl IntoElement {
    svg()
        .path("icons/spinner.svg")
        .flex_none()
        .size(px(11.))
        .text_color(color.opacity(0.8))
        .with_animation(
            "spinner",
            Animation::new(Duration::from_millis(600)).repeat(),
            |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_asset_loads_by_its_path() {
        for (path, _) in ASSETS {
            assert!(Assets.load(path).unwrap().is_some(), "{path}");
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }
}

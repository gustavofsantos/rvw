//! The bundled assets: the Phosphor icons, embedded in the binary so the
//! window never reads a file from disk at runtime.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Embeds `assets/icons/<name>.svg` as `("icons/<name>.svg", bytes)`.
macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((
            concat!("icons/", $name, ".svg"),
            include_bytes!(concat!("../assets/icons/", $name, ".svg")) as &[u8],
        )),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "list-checks",
    "list-checks-fill",
    "chat-text",
    "chat-text-fill",
    "git-diff",
    "git-diff-fill",
    "clock-counter-clockwise",
    "clock-counter-clockwise-fill",
    "git-branch",
    "circle",
    "circle-half",
    "check",
    "x",
    "plus",
    "caret-down",
    "caret-right",
    "arrow-left",
    "arrow-up",
    "arrow-down",
    "check-square",
    "square",
    "minus",
    "copy",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_loads_and_is_svg() {
        for (name, _) in ICONS {
            let bytes = Assets.load(name).unwrap().unwrap();
            assert!(bytes.starts_with(b"<svg"), "{name} is not an svg");
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }

    #[test]
    fn every_named_icon_is_bundled() {
        for icon in crate::icon::Icon::ALL {
            let path = icon.path();
            assert!(Assets.load(path).unwrap().is_some(), "{path} missing");
        }
    }
}

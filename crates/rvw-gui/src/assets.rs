//! The bundled assets: the rail's line icons, embedded in the binary so the
//! window never reads a file from disk at runtime.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

const ICONS: &[(&str, &[u8])] = &[
    (
        "icons/queue.svg",
        include_bytes!("../assets/icons/queue.svg"),
    ),
    (
        "icons/review.svg",
        include_bytes!("../assets/icons/review.svg"),
    ),
    (
        "icons/history.svg",
        include_bytes!("../assets/icons/history.svg"),
    ),
    (
        "icons/branches.svg",
        include_bytes!("../assets/icons/branches.svg"),
    ),
    (
        "icons/window-minimize.svg",
        include_bytes!("../assets/icons/window-minimize.svg"),
    ),
    (
        "icons/window-maximize.svg",
        include_bytes!("../assets/icons/window-maximize.svg"),
    ),
    (
        "icons/window-restore.svg",
        include_bytes!("../assets/icons/window-restore.svg"),
    ),
    (
        "icons/window-close.svg",
        include_bytes!("../assets/icons/window-close.svg"),
    ),
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
    fn the_window_control_icons_are_bundled() {
        for name in ["minimize", "maximize", "restore", "close"] {
            let path = format!("icons/window-{name}.svg");
            assert!(Assets.load(&path).unwrap().is_some(), "{path} missing");
        }
    }
}

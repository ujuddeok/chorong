//! The viewer's assets: its own icons (`app/assets/icons`, under `app-icons/`) and, for every
//! other path, the shared set from `cho_layout_ui::assets`.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

const ICONS: &[(&str, &[u8])] = &[
    ("link", include_bytes!("../assets/icons/link.svg")),
    ("unlink", include_bytes!("../assets/icons/unlink.svg")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let own = path
            .strip_prefix("app-icons/")
            .and_then(|p| p.strip_suffix(".svg"));
        match own {
            Some(name) => Ok(ICONS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, bytes)| Cow::Borrowed(*bytes))),
            None => cho_layout_ui::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut all = cho_layout_ui::assets::Assets.list(path)?;
        all.extend(
            ICONS
                .iter()
                .map(|(n, _)| SharedString::from(format!("app-icons/{n}.svg")))
                .filter(|p| p.starts_with(path)),
        );
        Ok(all)
    }
}

/// The `svg().path(..)` of one of the viewer's own icons.
pub fn icon(name: &str) -> SharedString {
    debug_assert!(ICONS.iter().any(|(n, _)| *n == name), "unknown icon {name}");
    SharedString::from(format!("app-icons/{name}.svg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_icons_and_shared_ones_are_served() {
        for (name, _) in ICONS {
            assert!(Assets.load(&icon(name)).unwrap().is_some(), "{name}");
        }
        let shared = cho_layout_ui::assets::icon("x");
        assert!(Assets.load(&shared).unwrap().is_some());
        assert!(Assets.load("app-icons/nope.svg").unwrap().is_none());
    }
}

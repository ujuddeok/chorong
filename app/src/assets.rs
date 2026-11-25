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

// The icon-name scan shared with cho-layout-ui's own test of its icons.
#[cfg(test)]
#[path = "../../crates/cho-layout-ui/tests/support/icon_scan.rs"]
mod icon_scan;

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

    /// Every icon name the viewer draws is bundled: its own through `crate::assets::icon`,
    /// the shared ones (`small_icon`, `cho_layout_ui::assets::icon`) from cho-layout-ui. Names
    /// that reach a call through a variable may be either.
    #[test]
    fn every_icon_the_viewer_draws_is_bundled() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let uses = super::icon_scan::icon_uses(&super::icon_scan::rust_files(&src));
        for known in ["link", "triangle-alert", "chevron-right", "file-code"] {
            assert!(
                uses.iter().any(|u| u.name == known),
                "the scan missed {known:?}"
            );
        }
        let own = |n: &str| ICONS.iter().any(|(i, _)| *i == n);
        let shared = |n: &str| {
            cho_layout_ui::assets::Assets
                .load(&format!("icons/{n}.svg"))
                .unwrap()
                .is_some()
        };
        let missing: Vec<String> = uses
            .iter()
            .filter(|u| match u.via.as_str() {
                "crate::assets::icon" | "assets::icon" => !own(&u.name),
                "" => !own(&u.name) && !shared(&u.name),
                _ => !shared(&u.name),
            })
            .map(|u| format!("{}:{}: {:?}", u.file.display(), u.line, u.name))
            .collect();
        assert!(
            missing.is_empty(),
            "unbundled icons:\n{}",
            missing.join("\n")
        );
    }
}

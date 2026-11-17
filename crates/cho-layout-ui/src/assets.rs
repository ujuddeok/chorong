//! Bundled Lucide icons (ISC, see `assets/icons/README.md`), served to GPUI's `svg()`.

use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "box",
    "chevron-down",
    "chevron-left",
    "chevron-right",
    "ellipsis",
    "eye",
    "file-code",
    "file-text",
    "folder",
    "folder-tree",
    "git-compare",
    "grip-vertical",
    "highlighter",
    "history",
    "inbox",
    "layers",
    "list-tree",
    "lock",
    "map",
    "maximize-2",
    "minimize-2",
    "minus",
    "panel-bottom",
    "panel-left",
    "panel-right",
    "panel-top",
    "pin",
    "pin-filled",
    "rotate-ccw",
    "search",
    "square-dashed",
    "square-function",
    "workflow",
    "x",
    "feather",
    "package",
    "list",
    "zap",
    "hash",
    "component",
    "maximize",
    "external-link",
    "file",
    "file-json",
    "folder-open",
    "info",
    "loader",
    "triangle-alert",
    "type",
    "variable",
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let name = path
            .strip_prefix("icons/")
            .and_then(|p| p.strip_suffix(".svg"));
        Ok(name.and_then(|n| {
            ICONS
                .iter()
                .find(|(i, _)| *i == n)
                .map(|(_, bytes)| Cow::Borrowed(*bytes))
        }))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .map(|(n, _)| SharedString::from(format!("icons/{n}.svg")))
            .filter(|p| p.starts_with(path))
            .collect())
    }
}

/// The path `svg().path(..)` takes for an icon name.
pub fn icon(name: &str) -> SharedString {
    debug_assert!(ICONS.iter().any(|(n, _)| *n == name), "unknown icon {name}");
    SharedString::from(format!("icons/{name}.svg"))
}

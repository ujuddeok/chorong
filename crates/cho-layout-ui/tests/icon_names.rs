//! Every icon name this crate draws is bundled (`assets::ICONS`): `icon()` only
//! `debug_assert!`s, and "Move pane to…" shipped asking for an unbundled "move".

#[path = "support/icon_scan.rs"]
mod icon_scan;

use cho_layout_ui::assets::Assets;
use gpui::AssetSource;
use std::path::Path;

#[test]
fn every_icon_the_crate_draws_is_bundled() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let uses = icon_scan::icon_uses(&icon_scan::rust_files(&src));
    // The scan finds the names it should: a scan that finds nothing passes vacuously.
    for known in [
        "move",
        "grip-vertical",
        "panel-left",
        "folder-tree",
        "square-function",
    ] {
        assert!(
            uses.iter().any(|u| u.name == known),
            "the scan missed {known:?}; found {uses:#?}"
        );
    }
    let missing: Vec<String> = uses
        .iter()
        .filter(|u| {
            Assets
                .load(&format!("icons/{}.svg", u.name))
                .unwrap()
                .is_none()
        })
        .map(|u| format!("{}:{}: {:?} {}", u.file.display(), u.line, u.name, u.via))
        .collect();
    assert!(
        missing.is_empty(),
        "unbundled icons:\n{}",
        missing.join("\n")
    );
}

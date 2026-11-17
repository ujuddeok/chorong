//! The UI and code fonts: the bundled IBM Plex by default, or the families chosen in settings
//! when they are installed.
//!
//! GPUI's default UI family is `.SystemUIFont`. On Linux it maps to a desktop font, and when
//! neither that nor GPUI's fallbacks (Lilex, IBM Plex Sans, Helvetica, …) are installed, every
//! text run repeats the failed lookups. GPUI caches the failure but rebuilds the error on each
//! hit (`anyhow!("{err}")`), which captures a backtrace when `RUST_BACKTRACE` is set: profiled,
//! that was about 70% of the lab's frame time.
//!
//! Call [`install`] at startup (and again when the font settings change) and read [`Fonts`]
//! with `cx.global::<Fonts>()` or [`pick`]. Naming an installed family avoids the failed
//! lookups entirely.

use gpui::{App, Global, SharedString};
use std::borrow::Cow;

/// The bundled families (IBM Plex, SIL Open Font License, `fonts/OFL-IBM-Plex.txt`): the
/// default UI and code fonts, so text looks the same on every machine. Only the faces Chorong
/// draws: Sans Regular, Medium and SemiBold (section headers; Bold falls back to it); Mono
/// Regular, Italic (comments) and Medium (the matched symbol in previews).
pub const BUNDLED_UI: &str = "IBM Plex Sans";
pub const BUNDLED_MONO: &str = "IBM Plex Mono";

const BUNDLED: &[&[u8]] = &[
    include_bytes!("../fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../fonts/IBMPlexMono-Italic.ttf"),
    include_bytes!("../fonts/IBMPlexMono-Medium.ttf"),
];

/// Installed families tried, in order, when neither the chosen font nor the bundled one can
/// be used (the bundled fonts failed to load).
const UI: &[&str] = &[
    "Inter",
    "Cantarell",
    "Ubuntu",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
];
const MONO: &[&str] = &[
    "Menlo",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
];

#[derive(Clone)]
pub struct Fonts {
    pub ui: SharedString,
    pub mono: SharedString,
}

fn first_installed(names: &[String], wanted: &[&str], otherwise: &str) -> SharedString {
    wanted
        .iter()
        .find(|w| names.iter().any(|n| n == *w))
        .copied()
        .unwrap_or(otherwise)
        .to_string()
        .into()
}

impl Global for Fonts {}

/// Registers the bundled fonts, picks the UI and code fonts (`ui` and `mono` are the chosen
/// families, from settings; None or a family that is not installed gives the bundled one),
/// and stores them as a global.
pub fn install(cx: &mut App, ui: Option<&str>, mono: Option<&str>) -> Fonts {
    if cx.try_global::<Bundled>().is_none() {
        let fonts = BUNDLED.iter().map(|b| Cow::Borrowed(*b)).collect();
        // A failure leaves the installed fallbacks in charge; nothing else depends on it.
        cx.text_system().add_fonts(fonts).ok();
        cx.set_global(Bundled);
    }
    let names = cx.text_system().all_font_names();
    let f = Fonts {
        ui: choose(&names, ui, BUNDLED_UI, UI, ".SystemUIFont"),
        mono: choose(&names, mono, BUNDLED_MONO, MONO, "DejaVu Sans Mono"),
    };
    cx.set_global(f.clone());
    f
}

/// Marks that the bundled fonts are registered.
struct Bundled;
impl Global for Bundled {}

/// The chosen family if installed, else the bundled one, else the first installed fallback.
fn choose(
    names: &[String],
    chosen: Option<&str>,
    bundled: &str,
    fallbacks: &[&str],
    otherwise: &str,
) -> SharedString {
    let has = |n: &str| names.iter().any(|x| x == n);
    match chosen.map(str::trim).filter(|c| !c.is_empty() && has(c)) {
        Some(c) => c.to_string().into(),
        None if has(bundled) => bundled.to_string().into(),
        None => first_installed(names, fallbacks, otherwise),
    }
}

/// Every installed family (the bundled ones included), sorted and without duplicates: what the
/// settings page offers.
pub fn families(cx: &App) -> Vec<String> {
    let mut names: Vec<String> = cx
        .text_system()
        .all_font_names()
        .into_iter()
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// The fonts in use: the global set by [`install`], else a pick from what is installed.
pub fn pick(cx: &App) -> Fonts {
    if let Some(f) = cx.try_global::<Fonts>() {
        return f.clone();
    }
    let names = cx.text_system().all_font_names();
    Fonts {
        ui: choose(&names, None, BUNDLED_UI, UI, ".SystemUIFont"),
        mono: choose(&names, None, BUNDLED_MONO, MONO, "DejaVu Sans Mono"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chosen_then_bundled_then_fallbacks() {
        let names: Vec<String> = ["Inter", "IBM Plex Sans", "Menlo"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = |chosen| choose(&names, chosen, BUNDLED_UI, UI, "x").to_string();
        assert_eq!(c(Some("Menlo")), "Menlo");
        assert_eq!(c(Some("Not Installed")), "IBM Plex Sans");
        assert_eq!(c(None), "IBM Plex Sans");
        let bare = vec!["Inter".to_string()];
        assert_eq!(
            choose(&bare, None, BUNDLED_UI, UI, "x").to_string(),
            "Inter"
        );
    }
}

//! The resolved theme colours the viewer's panes draw with (`cho_layout_ui::colors::Colors`,
//! the same set the shell uses), made once per theme and kept in a GPUI global. Pane bodies
//! draw while the shell is being updated, so they cannot read the shell's own copy.

use cho_layout_ui::colors::Colors;
use cho_theme::{Rgba, Theme};
use gpui::{App, Global, SharedString};
use std::rc::Rc;

#[derive(Default)]
struct PaneColors(Option<(Key, Rc<Colors>)>);

/// What the cached colours depend on: the theme, and the fonts they carry.
type Key = (String, Rgba, Rgba, SharedString, SharedString);

impl Global for PaneColors {}

/// The colours for `theme`, built on first use and whenever the theme changes.
pub fn colors(theme: &Theme, cx: &mut App) -> Rc<Colors> {
    let fonts = cho_fonts::pick(cx);
    let key = (
        theme.name.clone(),
        theme.text,
        theme.editor_background,
        fonts.ui.clone(),
        fonts.mono.clone(),
    );
    if let Some((k, c)) = &cx.default_global::<PaneColors>().0
        && *k == key
    {
        return c.clone();
    }
    let c = Rc::new(Colors::new(theme, fonts));
    cx.default_global::<PaneColors>().0 = Some((key, c.clone()));
    c
}

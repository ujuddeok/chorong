//! Small shared pieces of the shell's look: a pane kind's header icon, the 14 px icon, and the
//! verification dot and chip (design/visual-design.md, "Verification labels").

use crate::assets::icon;
use crate::colors::Colors;
use cho_layout::PaneKind;
use gpui::{
    AnyElement, FontWeight, Hsla, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder,
    px, svg,
};

/// The Lucide icon name for a pane kind's header.
pub fn kind_icon(kind: PaneKind) -> &'static str {
    match kind {
        PaneKind::FileTree => "folder-tree",
        PaneKind::Tour => "map",
        PaneKind::Context => "eye",
        PaneKind::PinnedContext => "pin-filled",
        PaneKind::Outline => "list-tree",
        PaneKind::CallStack => "layers",
        PaneKind::Comparison => "list",
        PaneKind::Graph => "workflow",
        PaneKind::Editor => "file-code",
        PaneKind::Search => "search",
        PaneKind::Changes => "git-compare",
        PaneKind::Highlights => "highlighter",
        PaneKind::Placeholder => "square-dashed",
    }
}

pub fn small_icon(name: &str, color: Hsla) -> AnyElement {
    svg()
        .path(icon(name))
        .size(px(14.))
        .flex_none()
        .text_color(color)
        .into_any_element()
}

/// A verification label (design/visual-design.md, "Verification labels"). Each differs by
/// shape as well as colour.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    Verified,
    Claimed,
    Broken,
}

impl Verification {
    fn word(self) -> &'static str {
        match self {
            Verification::Verified => "verified",
            Verification::Claimed => "claimed",
            Verification::Broken => "broken",
        }
    }

    fn colour(self, c: &Colors) -> Hsla {
        match self {
            Verification::Verified => c.success,
            Verification::Claimed => c.warning,
            Verification::Broken => c.error,
        }
        .unwrap_or(c.text_muted)
    }
}

/// The status dot: solid (verified), dashed ring (claimed), solid ring (broken).
pub fn dot(c: &Colors, v: Verification) -> AnyElement {
    let col = v.colour(c);
    let d = div().size(px(8.)).flex_none().rounded(px(4.));
    match v {
        Verification::Verified => d.bg(col),
        Verification::Claimed => d.border_1().border_dashed().border_color(col),
        Verification::Broken => d.border_1().border_color(col),
    }
    .into_any_element()
}

/// The status chip: tinted fill (verified, broken) or a dashed border and no fill (claimed).
pub fn chip(c: &Colors, v: Verification) -> AnyElement {
    let col = v.colour(c);
    let fill = match v {
        Verification::Verified => c.success_bg,
        Verification::Broken => c.error_bg,
        Verification::Claimed => None,
    };
    div()
        .px(px(6.))
        .h(px(16.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(col)
        .when_some(fill, |d, f| d.bg(f))
        .when(fill.is_none(), |d| {
            d.border_1()
                .border_color(col)
                .when(v == Verification::Claimed, |d| d.border_dashed())
        })
        .child(v.word())
        .into_any_element()
}

/// A mode chip at the start of a picker's query (Files, in Add to page): the mode that is
/// engaged, outlined in the accent on a faint wash. The caller adds the click.
pub fn mode_chip(
    c: &Colors,
    id: &'static str,
    icon_name: &str,
    label: &'static str,
) -> gpui::Stateful<gpui::Div> {
    use gpui::InteractiveElement;
    gpui::div()
        .id(id)
        .flex_none()
        .px(gpui::px(6.))
        .h(gpui::px(20.))
        .flex()
        .items_center()
        .gap_1()
        .rounded(gpui::px(4.))
        .border_1()
        .border_color(c.text_accent)
        .bg(c.accent_faint)
        .text_color(c.text)
        .text_size(gpui::px(11.5))
        .cursor_pointer()
        .hover(|s| s.bg(c.hover))
        .child(small_icon(icon_name, c.icon_accent))
        .child(label)
}

/// A key chip (`Esc`, bindings in menus and footers): element background, muted mono text.
pub fn key_chip(c: &Colors, k: &str) -> AnyElement {
    div()
        .flex_none()
        .px(px(5.))
        .h(px(16.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .bg(c.element_bg)
        .font_family(c.mono_font.clone())
        .text_size(px(10.5))
        .text_color(c.text_muted)
        .child(gpui::SharedString::from(k.to_string()))
        .into_any_element()
}

/// A picker's footer of keys and what they do (`Enter open  Esc close`); a key with no
/// binding shows its words alone.
pub fn hint_footer(c: &Colors, items: Vec<(Option<String>, String)>) -> AnyElement {
    div()
        .flex_none()
        .h(px(24.))
        .px_3()
        .flex()
        .items_center()
        .gap_3()
        .border_t_1()
        .border_color(c.border)
        .text_size(px(11.5))
        .text_color(c.text_muted)
        .children(items.into_iter().map(|(key, what)| {
            div()
                .flex()
                .gap_1()
                .children(key.map(|k| key_chip(c, &k)))
                .child(what)
        }))
        .into_any_element()
}

//! The pieces a list pane's rows are made of, shared by the Changes and Comparison panes: the
//! row shell and its tint order, the collapse chevron, the mark cell, small tags and the filter
//! box. Only looks; the panes own their rows and what a click does.
//!
//! In a plain list (Changes) tints are ordered **selection > change** ([`row_tint`]). In a
//! list of steps (tour, call stack, comparison) the open step keeps the full fill and the
//! keyboard's cursor on another row gets the lighter hover fill and an accent bar
//! ([`step_row_tint`], [`paint_tint`]), so "where I am" and "where I'm pointing" differ.

use super::*;
use cho_layout_ui::colors::Colors;
use cho_layout_ui::widgets::small_icon;
use gpui::{ElementId, Stateful};

/// Which tint a row gets, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowTint {
    /// The keyboard's selection.
    Selected,
    /// The open step.
    Step,
    /// The keyboard's cursor on a step that isn't open: the hover colour.
    Cursor,
    /// A change's colour (added, modified, deleted).
    Change,
    None,
}

/// The tint for a row in a plain list that is `selected` and has a `change` colour.
pub(crate) fn row_tint(selected: bool, change: bool) -> RowTint {
    if selected {
        RowTint::Selected
    } else if change {
        RowTint::Change
    } else {
        RowTint::None
    }
}

/// The tint of a row in a list of steps: the open step wins, then the keyboard's `cursor`.
pub(crate) fn step_row_tint(cursor: bool, current: bool) -> RowTint {
    if current {
        RowTint::Step
    } else if cursor {
        RowTint::Cursor
    } else {
        RowTint::None
    }
}

/// Fills `d` for `tint` (`change` is the change colour, used faintly), and draws the accent
/// bar of the keyboard's cursor when `cursor`.
pub(crate) fn paint_tint<E: Styled>(
    d: E,
    tint: RowTint,
    change: Option<gpui::Hsla>,
    cursor: bool,
    c: &Colors,
) -> E {
    let d = match (tint, change) {
        (RowTint::Selected | RowTint::Step, _) => d.bg(c.selected),
        (RowTint::Cursor, _) => d.bg(c.hover),
        (RowTint::Change, Some(colour)) => d.bg(colour.opacity(0.10)),
        _ => d,
    };
    if cursor {
        d.border_l_2().border_color(c.text_accent)
    } else {
        d
    }
}

/// A list row: full width, `height` tall, one line of flex children, clipped. The tint is
/// [`row_tint`]'s (the selection and the open step fill with the selection colour; `change`
/// is the change colour, used faintly). A clickable row that is not selected lights on hover.
/// The pane adds its own cursor and any selection border.
pub(crate) fn row_shell(
    id: impl Into<ElementId>,
    height: f32,
    tint: RowTint,
    change: Option<gpui::Hsla>,
    clickable: bool,
    c: &Colors,
) -> gpui::Stateful<Div> {
    let d = div()
        .id(id)
        .w_full()
        .h(px(height))
        .flex()
        .items_center()
        .pr_2()
        .whitespace_nowrap()
        .overflow_hidden();
    paint_tint(d, tint, change, false, c).when(clickable && tint != RowTint::Selected, |d| {
        d.hover(|s| s.bg(c.hover))
    })
}

/// The collapse chevron of a group row (a file, a phase); the caller handles the click.
pub(crate) fn chevron(
    id: impl Into<ElementId>,
    open: bool,
    height: f32,
    c: &Colors,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .w(px(16.))
        .h(px(height))
        .flex()
        .items_center()
        .child(small_icon(
            if open {
                "chevron-down"
            } else {
                "chevron-right"
            },
            c.icon_muted,
        ))
}

/// A mark cell: the mark's shape (`+`, `~`, `−`), in its colour. The glyph says it alone;
/// the colour only repeats it.
pub(crate) fn mark_cell(glyph: &'static str, colour: gpui::Hsla) -> Div {
    div()
        .flex_none()
        .w(px(14.))
        .mr(px(4.))
        .text_color(colour)
        .font_weight(FontWeight::BOLD)
        .child(glyph)
}

/// A list's filter box, typed while the list has the keyboard: `query` is the text as typed
/// (drawn by the pane), with a caret when `focused` and `placeholder` while `empty`. The
/// pane wraps it and handles the click.
pub(crate) fn filter_box(
    id: impl Into<ElementId>,
    query: impl IntoElement,
    empty: bool,
    placeholder: &'static str,
    focused: bool,
    c: &Colors,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .h(px(24.))
        .flex()
        .items_center()
        .gap_1()
        .rounded_sm()
        .border_1()
        .border_color(if focused { c.pane_focus } else { c.border })
        .cursor_text()
        .child(small_icon("search", c.icon_muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.))
                .child(query)
                .when(focused, |d| d.child(div().w(px(1.5)).h(px(13.)).bg(c.text)))
                .when(empty, |d| {
                    d.child(div().text_color(c.text_muted).child(placeholder))
                }),
        )
}

/// A small muted tag after a row's name (`uncommitted`, `same code`).
pub(crate) fn tag(text: impl Into<SharedString>, c: &Colors) -> Div {
    div()
        .flex_none()
        .ml_2()
        .px_1()
        .rounded_sm()
        .text_size(px(10.5))
        .text_color(c.text_muted)
        .bg(c.hover)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_wins_over_a_change() {
        assert_eq!(row_tint(true, true), RowTint::Selected);
        assert_eq!(row_tint(false, true), RowTint::Change);
        assert_eq!(row_tint(false, false), RowTint::None);
    }

    #[test]
    fn in_a_step_list_the_open_step_wins_over_the_cursor() {
        assert_eq!(step_row_tint(true, true), RowTint::Step);
        assert_eq!(step_row_tint(true, false), RowTint::Cursor);
        assert_eq!(step_row_tint(false, true), RowTint::Step);
        assert_eq!(step_row_tint(false, false), RowTint::None);
    }
}

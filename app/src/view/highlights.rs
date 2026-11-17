//! The Highlights pane: the active code pane's highlight groups as a row of chips, one per
//! group, in the group's colour. Clicking a chip's name steps to the group's next occurrence,
//! the arrows step either way and the cross removes the group. Its default place is the
//! bottom dock, where the chips run across; in a side dock they wrap.

use super::shell_host::{pane_of, show_pane};
use super::*;
use crate::marks::{Mode, occurrence_at, step_occurrence};
use cho_layout::{DockId, Node, PaneContent, PaneId, PaneKind};
use cho_layout_ui::widgets::small_icon;
use gpui::AnyElement;

const CHIP_H: f32 = 24.;
/// The bottom dock's height when the pane opens there alone: its header and one row of chips.
const DOCK_H: f32 = 72.;
/// The pane opens by itself when a highlight takes the count past this.
const AUTO_OPEN_AFTER: usize = 2;

/// What a chip says after the name: the caret's place among the occurrences ("3/12"), or
/// the count when the caret is on none. A name match says so, since it is not a claim that
/// the occurrences are one symbol.
fn position_label(occ: &[std::ops::Range<usize>], caret: Option<usize>, mode: &Mode) -> String {
    let n = occ.len();
    let at = caret.and_then(|c| occurrence_at(occ, c));
    let pos = match at {
        Some(i) => format!("{}/{n}", i + 1),
        None => n.to_string(),
    };
    match mode {
        Mode::NameInScope { .. } => format!("{pos} \u{b7} name"),
        Mode::Symbol(_) | Mode::Step(_) | Mode::Find(_) => pos,
    }
}

impl Chorong {
    /// Shows the pane, making it in the bottom dock when there is none. Alone there, the dock
    /// is sized to one row of chips. With `keep_focus` the keyboard stays in the code.
    fn open_highlights(&mut self, keep_focus: bool, cx: &mut Context<Self>) {
        let editor = self.active_editor;
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| {
                let bottom_was_empty = ws.arrangement().dock(DockId::Bottom).root.is_none();
                show_pane(ws, PaneContent::Highlights)?;
                let pane = pane_of(ws, PaneKind::Highlights);
                let in_bottom = pane
                    .and_then(|p| ws.arrangement().locate(p))
                    .is_some_and(|(r, _)| matches!(r, cho_layout::Region::Dock(DockId::Bottom)));
                if bottom_was_empty && in_bottom && !ws.in_focus() {
                    let _ = ws.resize_dock(DockId::Bottom, DOCK_H);
                }
                if keep_focus {
                    ws.activate(editor)?;
                }
                Ok(())
            });
        });
    }

    /// Cmd-K Shift-H: shows the pane, or hides it: the whole dock when it is alone there,
    /// otherwise by folding its group.
    pub(super) fn toggle_highlights(&mut self, cx: &mut Context<Self>) {
        let ws = self.shell.read(cx).workspace();
        let shown = pane_of(ws, PaneKind::Highlights).filter(|p| {
            ws.group_of(*p).is_some()
                && match ws.arrangement().locate(*p) {
                    Some((cho_layout::Region::Dock(d), _)) if !ws.in_focus() => {
                        ws.arrangement().dock(d).visible
                    }
                    _ => true,
                }
        });
        let Some(pane) = shown else {
            self.open_highlights(false, cx);
            return;
        };
        let alone_in = match ws.arrangement().locate(pane) {
            Some((cho_layout::Region::Dock(d), _))
                if !ws.in_focus()
                    && matches!(ws.arrangement().dock(d).root, Some(Node::Group(_))) =>
            {
                Some(d)
            }
            _ => None,
        };
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| match alone_in {
                Some(d) => ws.toggle_dock(d).map(|_| ()),
                None => ws.toggle_minimize(pane).map(|_| ()),
            });
        });
    }

    /// After a highlight is added in `pane`: the third one opens the pane, keeping the
    /// keyboard in the code. Not in a focus layout, where it would join the page.
    pub(super) fn highlight_added(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let count = self
            .editors
            .get(&pane)
            .map_or(0, |e| e.marks.groups().len());
        let ws = self.shell.read(cx).workspace();
        if pane != self.active_editor || count != AUTO_OPEN_AFTER + 1 || ws.in_focus() {
            return;
        }
        let on_screen = pane_of(ws, PaneKind::Highlights).is_some_and(|p| {
            ws.group_of(p).is_some()
                && match ws.arrangement().locate(p) {
                    Some((cho_layout::Region::Dock(d), _)) => ws.arrangement().dock(d).visible,
                    _ => true,
                }
        });
        if !on_screen {
            self.open_highlights(true, cx);
        }
    }

    /// Moves the caret to group `index`'s next (or previous) occurrence.
    fn step_group(&mut self, index: usize, forward: bool, cx: &mut Context<Self>) {
        let Some(g) = self.ed().marks.groups().get(index) else {
            return;
        };
        let caret = self.ed().caret.unwrap_or(0);
        if let Some(i) = step_occurrence(&g.occurrences, caret, forward) {
            let byte = g.occurrences[i].start;
            self.touch_pane();
            self.set_caret(byte, true, cx);
        }
    }

    fn remove_group(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.ed_mut().marks.remove(index) {
            self.touch_pane();
            self.find_group_gone(cx);
            if self.ed().marks.is_empty() {
                self.highlights_emptied(cx);
            }
        }
    }

    /// The last highlight is gone: the pane hides. Alone in a dock, the dock hides (keeping its
    /// size and place for next time); elsewhere the pane closes. A focus page is left as it is.
    pub(super) fn highlights_emptied(&mut self, cx: &mut Context<Self>) {
        let ws = self.shell.read(cx).workspace();
        if ws.in_focus() {
            return;
        }
        let Some(pane) = pane_of(ws, PaneKind::Highlights).filter(|p| ws.group_of(*p).is_some())
        else {
            return;
        };
        let place = match ws.arrangement().locate(pane) {
            Some((cho_layout::Region::Dock(d), _)) => {
                let dock = ws.arrangement().dock(d);
                if !dock.visible {
                    return;
                }
                matches!(dock.root, Some(Node::Group(_))).then_some(d)
            }
            _ => None,
        };
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| match place {
                Some(d) => ws.toggle_dock(d).map(|_| ()),
                None => ws.close(pane).map(|_| ()),
            });
        });
    }

    /// The pane header's buttons: the gutter-marks switch (`highlight.gutter_marks`, saved like
    /// any setting) and, with highlights, Clear all.
    pub(super) fn highlights_header_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let on = self.features.settings().highlight_gutter_marks;
        let button = |id: &'static str| {
            div()
                .id(id)
                .px(px(6.))
                .h(px(20.))
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(c.text_muted)
                .hover(|s| s.bg(c.hover).text_color(c.text))
                .active(|s| s.bg(c.ghost_active))
        };
        let gutter = button("highlights-gutter")
            .when(on, |d| d.text_color(c.text))
            .tooltip(tip(format!(
                "Gutter marks are {}: click to turn them {} (setting highlight.gutter_marks)",
                if on { "on" } else { "off" },
                if on { "off" } else { "on" }
            )))
            .child(small_icon(
                "panel-left",
                if on { c.text_accent } else { c.icon_muted },
            ))
            .child(if on { "Gutter: on" } else { "Gutter: off" })
            .on_click(cx.listener(|this, _, _, cx| {
                let saved =
                    crate::settings_window::toggle_setting(this.root(), "highlight.gutter_marks");
                this.notify_text(
                    match saved {
                        Some(true) => "Gutter marks on",
                        Some(false) => "Gutter marks off",
                        None => "Could not save the setting",
                    },
                    cx,
                );
            }));
        let clear = self.ed().marks.has_own().then(|| {
            button("highlights-clear")
                .tooltip(tip(
                    "Remove your highlights; a tour step's stay (Clear Highlights)".into(),
                ))
                .child("Clear all")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ed_mut().marks.clear_own();
                    this.touch_pane();
                    this.find_group_gone(cx);
                    this.highlights_emptied(cx);
                    cx.notify();
                }))
        });
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(gutter)
            .children(clear)
            .into_any_element()
    }

    pub(super) fn render_highlights(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let groups = self.ed().marks.groups();
        if groups.is_empty() {
            return div()
                .p_3()
                .text_size(px(12.5))
                .text_color(c.text_muted)
                .child(
                    "No highlights. Double-click a name in the code, or press h on it, to \
                     highlight every occurrence.",
                )
                .into_any_element();
        }
        let caret = self.ed().caret;
        let palette = &self.marker_colours;
        let chips = groups.iter().enumerate().map(|(i, g)| {
            let colour = palette
                .get(g.slot % palette.len().max(1))
                .copied()
                .unwrap_or(self.theme.ui.text_accent);
            let style = markers::style(colour, self.theme.appearance, false);
            let on_one = caret.is_some_and(|c| occurrence_at(&g.occurrences, c).is_some());
            let button = |id: &'static str, icon: &'static str, hint: &str| {
                div()
                    .id((id, i))
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover))
                    .active(|s| s.bg(c.active))
                    .tooltip(tip(hint.to_string()))
                    .child(small_icon(icon, c.icon))
            };
            let mode = g.mode.label();
            div()
                .flex()
                .items_center()
                .h(px(CHIP_H))
                .pl(px(2.))
                .pr(px(2.))
                .gap(px(2.))
                .rounded(px(6.))
                .border_1()
                .border_color(hsla(style.outline))
                .bg(hsla(style.fill))
                .when(on_one, |d| d.border_2())
                .child(
                    div()
                        .id(("highlight-name", i))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h_full()
                        .px(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover))
                        .active(|s| s.bg(c.active))
                        .child(div().size(px(8.)).rounded_full().bg(hsla(colour)))
                        .child(
                            div()
                                .font_family(c.mono_font.clone())
                                .font_weight(FontWeight::MEDIUM)
                                .child(g.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(c.text_muted)
                                .child(position_label(&g.occurrences, caret, &g.mode)),
                        )
                        .tooltip(tip(format!(
                            "{}: {} in this file, {mode}. Click for the next one.",
                            g.name,
                            g.occurrences.len()
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.step_group(i, true, cx);
                            cx.notify();
                        })),
                )
                .child(
                    button("highlight-prev", "chevron-left", "Previous occurrence").on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.step_group(i, false, cx);
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    button("highlight-next", "chevron-right", "Next occurrence").on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.step_group(i, true, cx);
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    button("highlight-remove", "x", "Remove this highlight").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.remove_group(i, cx);
                            cx.notify();
                        },
                    )),
                )
        });
        self.scroll_areas
            .get("highlights")
            .vertical(
                "highlights",
                self.features.probe_if_on(Feature::Overscroll),
                div()
                    .p_2()
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap_2()
                    .text_size(px(12.5))
                    .children(chips),
            )
            .size_full()
            .into_any_element()
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(chorong, [ToggleHighlights]);

pub(super) const COMMANDS: &[Command] = &[c(
    "chorong::ToggleHighlights",
    "Toggle Highlights Panel",
    Layer::View,
    146,
)];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    on!(d, cx, ToggleHighlights, |this, _w, cx| this
        .toggle_highlights(cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chip_shows_the_caret_place_or_the_count_and_marks_name_matches() {
        let occ = [2..5, 9..12, 20..23];
        let symbol = Mode::Symbol("s#x".into());
        let name = Mode::NameInScope {
            scope: 0..30,
            scope_kind: "fn",
        };
        assert_eq!(position_label(&occ, Some(10), &symbol), "2/3");
        assert_eq!(position_label(&occ, Some(7), &symbol), "3");
        assert_eq!(position_label(&occ, None, &symbol), "3");
        assert_eq!(position_label(&occ, Some(2), &name), "1/3 \u{b7} name");
    }
}

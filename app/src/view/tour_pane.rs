//! The Tour pane (`PaneKind::Tour`, left dock above the file tree) and the tour's filter on the
//! file tree (design/layout.md, "Tours").
//!
//! Stops are listed in order with numbered badges (done, current, to do, broken); a broken stop
//! is shown, never skipped, with its reason in place of its location. While a tour is open the
//! tree shows only the tour's files, each with its stop numbers, and `N of M files · show all`;
//! the Tour files chip in the tree header turns the filter off.

use super::investigation_view::{
    BadgeLook, banner_box, empty_pane, evidence_tag, phase_heading, step_badge,
};
use super::row_list::{paint_tint, step_row_tint};
use super::shell_host::OpenAt;
use super::*;
use crate::investigation::{self as inv, Label};
use crate::tree::{RowKind, Tree, file_icon, reveal};
use cho_investigation::Kind;
use cho_layout_ui::widgets::small_icon;
use std::path::Path;

const ROW_H: f32 = 40.;
const TREE_ROW_H: f32 = 22.;

/// The filtered tree: rows, and the stop numbers per file row.
pub(crate) struct TourTree {
    rows: Vec<(crate::tree::Row, Vec<usize>)>,
    shown: usize,
    total: usize,
}

impl Chorong {
    /// Whether the open investigation is a tour.
    pub(super) fn tour_open(&self) -> bool {
        self.inv.as_ref().is_some_and(|s| *s.kind() == Kind::Tour)
    }

    pub(super) fn render_tour(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let Some(st) = self.inv.as_ref().filter(|_| self.tour_open()) else {
            return empty_pane("tour", &c);
        };
        let steps = st.steps.clone();
        let items = inv::list_items(&steps);
        let (current, selected) = (st.current, st.selected);
        let keyboard = self.key_focus == KeyFocus::Steps;
        let error = c.error.unwrap_or(c.text);
        let font = self.font.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .children(self.render_inv_notices(cx))
            .when_some(st.host.clone(), |d, host| {
                d.child(
                    div()
                        .px_3()
                        .pt_1()
                        .text_size(px(11.))
                        .text_color(c.text_muted)
                        .child(format!("from {host}")),
                )
            })
            .when(steps.is_empty() && st.at.is_some() && !st.checking, |d| {
                d.child(banner_box(&c).child("This tour has no steps."))
            })
            .children(self.render_branch_picker("tour-branch", cx))
            .children(self.render_inv_overview(cx))
            .child(
                crate::scroll::uniform_list(
                    "tour-stops",
                    items.len(),
                    cx.processor(move |_this, range: std::ops::Range<usize>, _w, cx| {
                        range
                            .filter_map(|row| {
                                let ix = match items.get(row)? {
                                    inv::ListItem::Phase(p) => {
                                        return Some(phase_heading(*p, ROW_H, &c));
                                    }
                                    inv::ListItem::Step(ix) => *ix,
                                };
                                let s = steps.get(ix)?;
                                let state = inv::stop_state(s, ix, current);
                                let external = s.is_external();
                                let action = s.is_action();
                                // Off the chosen path: muted, never hidden.
                                let not_reached = s.extras.not_reached.clone();
                                let off = not_reached.is_some();
                                let broken = s.label == Label::Broken && !s.no_code() && !off;
                                let loc = if let Some(why) = &not_reached {
                                    format!("not reached \u{b7} {why}")
                                } else if external {
                                    "not in the code".to_string()
                                } else if action {
                                    match s.extras.action.as_ref().map_or(0, |a| a.commands.len()) {
                                        0 => "to do by hand".to_string(),
                                        1 => "to do by hand \u{b7} 1 command".to_string(),
                                        n => format!("to do by hand \u{b7} {n} commands"),
                                    }
                                } else if broken {
                                    s.reason.clone().unwrap_or_default()
                                } else {
                                    s.location()
                                };
                                let is_current = ix == current;
                                let is_selected = keyboard && ix == selected;
                                let look =
                                    if off || (s.no_code() && state == inv::StopState::Broken) {
                                        BadgeLook::ToDo
                                    } else {
                                        BadgeLook::from(state)
                                    };
                                Some(
                                    div()
                                        .id(("stop", ix))
                                        .w_full()
                                        .h(px(ROW_H))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .cursor_pointer()
                                        .map(|d| {
                                            let tint = step_row_tint(is_selected, is_current);
                                            paint_tint(d, tint, None, is_selected, &c)
                                        })
                                        .when(off, |d| d.opacity(0.55))
                                        .hover(|s| s.bg(c.hover))
                                        .child(step_badge(s.number.clone(), look, &c))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .whitespace_nowrap()
                                                        .overflow_hidden()
                                                        .text_ellipsis()
                                                        .when(is_current, |d| {
                                                            d.font_weight(FontWeight::MEDIUM)
                                                        })
                                                        .when(
                                                            state == inv::StopState::Done || off,
                                                            |d| d.text_color(c.text_muted),
                                                        )
                                                        .child(s.list_name().to_string()),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .min_w_0()
                                                        .text_size(px(11.))
                                                        .when(external, |d| {
                                                            d.child(small_icon("box", c.icon))
                                                        })
                                                        .when(action, |d| {
                                                            d.child(
                                                                super::action_card::action_glyph(
                                                                    font.clone(),
                                                                    &c,
                                                                ),
                                                            )
                                                        })
                                                        .child(
                                                            div()
                                                                .min_w_0()
                                                                .whitespace_nowrap()
                                                                .overflow_hidden()
                                                                .text_ellipsis()
                                                                .text_color(if broken {
                                                                    error
                                                                } else {
                                                                    c.text_muted
                                                                })
                                                                .when(broken, |d| d.line_through())
                                                                .when(
                                                                    external || action || off,
                                                                    |d| d.italic(),
                                                                )
                                                                .child(loc),
                                                        )
                                                        .children(
                                                            s.extras
                                                                .evidence
                                                                .map(|e| evidence_tag(e, &c)),
                                                        ),
                                                ),
                                        )
                                        .when_some(
                                            s.reason.clone().filter(|_| !broken && !s.no_code()),
                                            |d, r| d.tooltip(tip(r)),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.step_clicked(ix, cx);
                                        }))
                                        .into_any_element(),
                                )
                            })
                            .collect()
                    }),
                )
                .wrap(
                    &st.over,
                    &st.scroll,
                    self.features.probe_if_on(Feature::Overscroll),
                )
                .flex_1()
                .min_h_0(),
            )
            .child(self.render_step_hint(cx))
            .into_any_element()
    }

    /// The Tour pane's header qualifier: `3/6 · the tour's title`.
    pub(super) fn tour_qualifier(&self) -> String {
        let Some(st) = self.inv.as_ref().filter(|_| self.tour_open()) else {
            return String::new();
        };
        match st.position() {
            Some(pos) => format!("{pos} \u{b7} {}", st.inv.title),
            None => st.inv.title.clone(),
        }
    }

    /// Added to the tree's qualifier while the tour filters it, so its header (and the Tour
    /// files chip) redraws when the filter changes.
    pub(super) fn tree_filter_qualifier(&self) -> Option<String> {
        let st = self
            .inv
            .as_ref()
            .filter(|s| s.tree_filter && self.tour_open())?;
        Some(format!("tour \u{b7} {}", inv::tour_files(&st.steps).len()))
    }

    /// The Tour files chip (tree header) and the `show all` link.
    pub(super) fn toggle_tour_files(&mut self, cx: &mut Context<Self>) {
        if !self.tour_open() {
            self.notify_text("No tour is open", cx);
            return;
        }
        if let Some(st) = &mut self.inv {
            st.tree_filter = !st.tree_filter;
        }
        self.redraw(None, cx);
        self.redraw_step_headers(cx);
    }

    /// The tree as the tour filters it, while a tour is open with the filter on.
    pub(super) fn tour_tree(&self) -> Option<TourTree> {
        let st = self
            .inv
            .as_ref()
            .filter(|s| s.tree_filter && self.tour_open())?;
        if st.steps.is_empty() {
            return None;
        }
        let files = inv::tour_files(&st.steps);
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(p)).collect();
        let tree = Tree::new(&paths);
        let mut expanded = HashSet::new();
        for p in &paths {
            reveal(p, &mut expanded);
        }
        let rows = tree
            .rows(&expanded)
            .into_iter()
            .map(|r| {
                let ns = files
                    .iter()
                    .find(|(p, _)| Path::new(p) == r.rel)
                    .map(|(_, ns)| ns.clone())
                    .unwrap_or_default();
                (r, ns)
            })
            .collect();
        Some(TourTree {
            rows,
            shown: files.len(),
            total: self.files.len(),
        })
    }

    pub(super) fn render_tour_tree(&self, t: TourTree, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let current_n = self
            .inv
            .as_ref()
            .and_then(|s| s.steps.get(s.current))
            .map(|s| s.n);
        let open = self.ed().doc.as_ref().and_then(|d| {
            let root = self.inv.as_ref()?.root.clone();
            d.path.strip_prefix(root).ok().map(Path::to_path_buf)
        });
        let rows = t.rows.into_iter().enumerate().map(|(ix, (row, ns))| {
            let file = row.kind == RowKind::File;
            let selected = file && open.as_deref() == Some(row.rel.as_path());
            let name = row.segments.join(" / ");
            let icon = match row.kind {
                RowKind::Folder { .. } => "folder-open",
                RowKind::File => file_icon(row.segments.last().map_or("", String::as_str)),
            };
            let rel = row.rel.clone();
            div()
                .id(("tour-tree", ix))
                .mx_1()
                .h(px(TREE_ROW_H))
                .rounded(px(4.))
                .flex()
                .items_center()
                .pl(px(4. + 12. * row.depth as f32 + 18.))
                .pr_2()
                .gap(px(6.))
                .whitespace_nowrap()
                .overflow_hidden()
                .when(selected, |d| {
                    d.bg(c.selected).font_weight(FontWeight::MEDIUM)
                })
                .when(file && !selected, |d| d.hover(|s| s.bg(c.hover)))
                .when(file, |d| d.cursor_pointer())
                .child(small_icon(icon, c.icon))
                .child(div().flex_1().min_w_0().overflow_hidden().child(name))
                .children(ns.into_iter().map(|n| {
                    let cur = Some(n) == current_n;
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded_full()
                        .text_size(px(10.5))
                        .when(cur, |d| d.bg(c.text_accent).text_color(c.editor))
                        .when(!cur, |d| d.bg(c.hover).text_color(c.text_muted))
                        .child(n.to_string())
                }))
                .when(file, |d| {
                    d.on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        if let Some(root) = this.inv.as_ref().map(|s| s.root.clone()) {
                            this.open_at(root.join(&rel), OpenAt::of_click(&e.modifiers()), cx);
                        }
                    }))
                })
        });
        div()
            .size_full()
            .pt_1()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .child(div().flex_1().min_h_0().overflow_hidden().children(rows))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .flex()
                    .gap_1()
                    .text_size(px(11.5))
                    .text_color(c.text_muted)
                    .child(format!("{} of {} files \u{b7}", t.shown, t.total))
                    .child(
                        div()
                            .id("tour-show-all")
                            .text_color(c.text_accent)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child("show all")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_tour_files(cx))),
                    ),
            )
            .into_any_element()
    }

    /// The Tour files chip in the file tree's header, while a tour is open.
    pub(super) fn tour_files_chip(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let st = self.inv.as_ref().filter(|_| self.tour_open())?;
        let on = st.tree_filter;
        let c = crate::pane_style::colors(&self.theme, cx);
        Some(
            div()
                .id("tour-files-chip")
                .px(px(6.))
                .h(px(20.))
                .flex()
                .items_center()
                .gap_1()
                .rounded(px(4.))
                .border_1()
                .border_color(if on { c.text_accent } else { c.border })
                .when(on, |d| d.bg(c.accent_faint).text_color(c.text))
                .when(!on, |d| d.text_color(c.text_muted))
                .cursor_pointer()
                .text_size(px(11.5))
                .hover(|s| s.bg(c.hover))
                .tooltip(tip(self.tip_text(
                    if on {
                        "Showing the tour's files: click to show every file"
                    } else {
                        "Show only the tour's files"
                    },
                    "chorong::ToggleTourFiles",
                )))
                .child(small_icon("map", if on { c.icon_accent } else { c.icon }))
                .child("Tour files")
                .on_click(cx.listener(|this, _, _, cx| this.toggle_tour_files(cx)))
                .into_any_element(),
        )
    }
}

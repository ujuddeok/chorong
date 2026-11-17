//! The Comparison pane (`PaneKind::Comparison`, a bottom-bar tab like Call stack) and what a
//! format version 2 comparison, its variants and a tour's branches look like in the viewer
//! (design/investigations-viewer.md, "Format version 2 in the viewer (decided)"; the owner's
//! option A, with option C as Open Comparison as Tour).
//!
//! Rows are steps, not files: one row per comparison row, a column per side, and a result
//! chip that says by shape as well as colour whether the row is the same, differs or is
//! missing, where that comes from (derived by Chorong or claimed by the author) and whether
//! every side runs the same code. Setup rows (S1…) are their own group; a callout ("Paths
//! split here") follows its row. Enter opens the row's focus page with one editor per side,
//! each at its anchor and tinted; the same pages with the rail are the comparison as a tour.
//! The rows are built from the shared `row_list` pieces, in its tint order.
//!
//! Variants (a comparison side's) and branches (a tour's) are chosen here too: the choice is
//! remembered per investigation, and the steps are drawn again from the last check without
//! checking again.

use super::investigation_view::{BadgeLook, banner_box, empty_pane, runtime_chip, step_badge};
use super::row_list::{RowTint, chevron, paint_tint, row_shell, step_row_tint, tag};
use super::shell_host::Redraw;
use super::*;
use crate::comparison::{Outcome, RowInfo, SideCell};
use crate::investigation::{self as inv, CodeAt, Step};
use crate::note_md::Text;
use crate::tour_pages::Phase;
use cho_investigation::Kind;
use cho_layout_ui::colors::Colors;
use cho_layout_ui::widgets::small_icon;
use gpui::{AnyElement, ScrollHandle, relative};

const ROW_H: f32 = 38.;
const HEAD_H: f32 = 24.;
/// The number and result columns; the step name and the sides share the rest.
const NUMBER_W: f32 = 40.;
const RESULT_W: f32 = 200.;
const NAME_SHARE: f32 = 0.28;

/// What the Comparison pane keeps between frames.
#[derive(Default)]
pub(crate) struct CmpView {
    pub setup_collapsed: bool,
    pub call_collapsed: bool,
    /// Only rows with this result (a click on a count in the header).
    pub filter: Option<Outcome>,
    pub scroll: ScrollHandle,
}

/// A line of the Comparison pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CmpItem {
    /// A phase's heading, with its row count.
    Phase(Phase, usize),
    Row(usize),
    /// The callout after row `ix`.
    Callout(usize),
}

/// The pane's lines: when any row has a phase, each phase's rows under its heading (Setup
/// first, as the rows are written); a collapsed phase shows only its heading. A filter keeps
/// the rows with that result.
pub(crate) fn cmp_items(steps: &[Step], view: &CmpView) -> Vec<CmpItem> {
    let phased = steps.iter().any(|s| s.extras.phase.is_some());
    let shown = |s: &Step| {
        view.filter
            .is_none_or(|f| s.extras.row.as_ref().is_some_and(|r| r.outcome == f))
    };
    let mut out = Vec::new();
    let mut last = None;
    for (i, s) in steps.iter().enumerate() {
        let p = s.extras.phase.unwrap_or(Phase::Call);
        if phased && last != Some(p) {
            let n = steps[i..]
                .iter()
                .take_while(|t| t.extras.phase.unwrap_or(Phase::Call) == p)
                .filter(|t| shown(t))
                .count();
            out.push(CmpItem::Phase(p, n));
            last = Some(p);
        }
        let collapsed = phased
            && match p {
                Phase::Setup => view.setup_collapsed,
                Phase::Call => view.call_collapsed,
            };
        if collapsed || !shown(s) {
            continue;
        }
        out.push(CmpItem::Row(i));
        if s.extras.row.as_ref().is_some_and(|r| r.callout.is_some()) {
            out.push(CmpItem::Callout(i));
        }
    }
    out
}

/// The rows `items` shows, in order.
pub(crate) fn shown_rows(items: &[CmpItem]) -> Vec<usize> {
    items
        .iter()
        .filter_map(|i| match i {
            CmpItem::Row(ix) => Some(*ix),
            _ => None,
        })
        .collect()
}

/// The row `delta` shown rows from `from` (clamped); `from` itself may be hidden.
pub(crate) fn step_shown(shown: &[usize], from: usize, delta: isize) -> Option<usize> {
    let at = match shown.iter().position(|r| *r == from) {
        Some(p) => p.saturating_add_signed(delta),
        // Hidden: the first shown row after it going down, before it going up.
        None if delta >= 0 => shown.iter().position(|r| *r > from).unwrap_or(0),
        None => shown.iter().rposition(|r| *r < from).unwrap_or(0),
    };
    shown.get(at.min(shown.len().checked_sub(1)?)).copied()
}

/// The result chip: its glyph and word, where it comes from, and `same code`. Each result has
/// its own shape (filled, tinted, dashed, outlined) as well as colour.
pub(crate) fn result_chip(info: &RowInfo, c: &Colors) -> Div {
    let warn = c.warning.unwrap_or(c.text_accent);
    let chip = div()
        .flex_none()
        .h(px(18.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded(px(4.))
        .text_size(px(11.))
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .child(info.outcome.glyph()),
        )
        .child(info.outcome.word());
    let chip = match info.outcome {
        Outcome::Same => chip.bg(c.hover).text_color(c.text_muted),
        Outcome::Differs => chip
            .bg(warn.opacity(0.18))
            .text_color(warn)
            .font_weight(FontWeight::MEDIUM),
        Outcome::Missing => chip
            .border_1()
            .border_dashed()
            .border_color(c.text_muted)
            .text_color(c.text_muted),
        Outcome::Unset => chip
            .border_1()
            .border_color(c.border)
            .text_color(c.text_muted)
            .italic(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .min_w_0()
        .child(chip)
        .when(info.source != crate::comparison::Source::None, |d| {
            d.child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(c.text_muted)
                    .child(info.source.word()),
            )
        })
        .when(info.same_code, |d| d.child(tag("same code", c).ml_0()))
}

/// The "paths split here" line under a row: the author's callout, called out.
fn callout_line(ix: usize, text: &str, c: &Colors) -> AnyElement {
    let error = c.error.unwrap_or(c.text_accent);
    div()
        .id(("cmp-callout", ix))
        .mx_2()
        .mb(px(2.))
        .px_2()
        .h(px(22.))
        .flex()
        .items_center()
        .gap_1()
        .rounded_sm()
        .bg(error.opacity(0.10))
        .text_color(error)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(11.5))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(small_icon("git-compare", error))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(super::note_text_in(&Text::of(text, " "), c)),
        )
        .tooltip(super::note_tip(Text::of(text, "\n")))
        .into_any_element()
}

/// One side's cell: the step's short name or declaration with its values as claims (dashed),
/// and where it is; or why the side has none.
fn cell_el(cell: &SideCell, share: f32, mono: &SharedString, c: &Colors) -> Div {
    let error = c.error.unwrap_or(c.text);
    let col = div()
        .w(relative(share))
        .min_w_0()
        .pr_2()
        .flex()
        .flex_col()
        .justify_center()
        .overflow_hidden();
    if let Some(reason) = &cell.missing {
        return col.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .italic()
                .text_size(px(11.5))
                .text_color(c.text_muted)
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .child("\u{2205}"),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(super::note_text_in(&Text::of(reason, " "), c)),
                ),
        );
    }
    let text = cell.text.clone().unwrap_or_default();
    let second = match (&cell.broken, &cell.external, &cell.location) {
        (Some(why), _, _) => Some(why.clone()),
        (None, Some(what), _) => Some(format!("not in the code \u{b7} {what}")),
        (None, None, loc) => loc.clone(),
    };
    col.child(
        div()
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .when(cell.external.is_some(), |d| {
                d.child(small_icon("box", c.icon))
            })
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(mono.clone())
                    .text_size(px(11.5))
                    .when(cell.broken.is_some(), |d| {
                        d.text_color(error).line_through()
                    })
                    .child(text),
            )
            .children(cell.values.iter().map(|v| runtime_chip(v, c))),
    )
    .children(second.map(|t| {
        div().flex().min_w_0().child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(10.5))
                .text_color(if cell.broken.is_some() {
                    error
                } else {
                    c.text_muted
                })
                .when(cell.external.is_some(), |d| d.italic())
                .child(t),
        )
    }))
}

impl Chorong {
    /// Whether the open investigation is a comparison.
    pub(super) fn comparison_open(&self) -> bool {
        self.inv
            .as_ref()
            .is_some_and(|s| *s.kind() == Kind::Comparison)
    }

    /// The Comparison pane's header qualifier: `2/5 · the title`.
    pub(super) fn comparison_qualifier(&self) -> String {
        let Some(st) = self.inv.as_ref().filter(|_| self.comparison_open()) else {
            return String::new();
        };
        match st.position() {
            Some(pos) => format!("{pos} \u{b7} {}", st.inv.title),
            None => st.inv.title.clone(),
        }
    }

    pub(super) fn render_comparison(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let Some(st) = self.inv.as_ref().filter(|_| self.comparison_open()) else {
            return empty_pane("comparison", &c);
        };
        let steps = st.steps.clone();
        let items = cmp_items(&steps, &st.cmp);
        let keyboard = self.key_focus == KeyFocus::Steps;
        let (current, selected) = (st.current, st.selected);
        let shown_in_code = steps
            .get(current)
            .and_then(|s| s.extras.row.as_ref())
            .is_some_and(|row| {
                self.editors.values().any(|e| {
                    e.step_mark
                        .as_ref()
                        .and_then(|m| m.cell.as_ref())
                        .is_some_and(|(r, _)| Arc::ptr_eq(r, row))
                })
            });
        let sides = st.inv.sides.len().max(1);
        let side_share = (1.0 - NAME_SHARE) / sides as f32;
        let mono: SharedString = c.mono_font.clone();
        // The column headings: a side's label with its chosen variant.
        let side_heads: Vec<String> = st
            .inv
            .sides
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let variant = st
                    .choice
                    .variants
                    .get(&s.id)
                    .and_then(|v| s.variants.iter().find(|x| &x.id == v))
                    .map(|v| format!(" ({})", v.label));
                format!(
                    "{} \u{b7} {}{}",
                    crate::comparison::letter(i),
                    s.label,
                    variant.unwrap_or_default()
                )
            })
            .collect();
        let heading_row = div()
            .flex_none()
            .h(px(HEAD_H))
            .pl_2()
            .pr_2()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(c.border_variant)
            .text_size(px(11.))
            .text_color(c.text_muted)
            .child(div().flex_none().w(px(NUMBER_W)).child("#"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .child(div().w(relative(NAME_SHARE)).child("Step"))
                    .children(side_heads.iter().map(|h| {
                        div()
                            .w(relative(side_share))
                            .min_w_0()
                            .pr_2()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(h.clone())
                    })),
            )
            .child(div().flex_none().w(px(RESULT_W)).child("Result"));
        let mut lines: Vec<AnyElement> = Vec::with_capacity(items.len());
        for item in &items {
            let line: AnyElement = match *item {
                CmpItem::Phase(p, n) => {
                    let open = match p {
                        Phase::Setup => !st.cmp.setup_collapsed,
                        Phase::Call => !st.cmp.call_collapsed,
                    };
                    let (id, word) = match p {
                        Phase::Setup => ("cmp-phase-setup", "Setup"),
                        Phase::Call => ("cmp-phase-call", "Call"),
                    };
                    div()
                        .id(id)
                        .h(px(22.))
                        .pl_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.text_muted)
                        .child(chevron((id, 0usize), open, 22., &c))
                        .child(word)
                        .child(div().font_weight(FontWeight::NORMAL).child(match p {
                            Phase::Setup => format!("\u{b7} {n} (S1\u{2026}), before any call"),
                            Phase::Call => format!("\u{b7} {n}"),
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(st) = &mut this.inv {
                                match p {
                                    Phase::Setup => {
                                        st.cmp.setup_collapsed = !st.cmp.setup_collapsed
                                    }
                                    Phase::Call => st.cmp.call_collapsed = !st.cmp.call_collapsed,
                                }
                            }
                            this.redraw(Some(Redraw::Steps), cx);
                        }))
                        .into_any_element()
                }
                CmpItem::Callout(ix) => {
                    let text = steps
                        .get(ix)
                        .and_then(|s| s.extras.row.as_ref()?.callout.clone())
                        .unwrap_or_default();
                    callout_line(ix, &text, &c)
                }
                CmpItem::Row(ix) => {
                    let Some(s) = steps.get(ix) else { continue };
                    let Some(info) = s.extras.row.clone() else {
                        continue;
                    };
                    // The row whose code is shown: on its page, or still in a code pane after
                    // the pages closed.
                    let is_current = ix == current && (st.pages.is_some() || shown_in_code);
                    let cursor = keyboard && ix == selected;
                    let tint = step_row_tint(cursor, is_current);
                    let look = match tint {
                        RowTint::Step => BadgeLook::Current,
                        _ if s.label == inv::Label::Broken => BadgeLook::Broken,
                        _ => BadgeLook::ToDo,
                    };
                    // Chorong's own lines as written; the author's words (the missing side's
                    // reason, the note) through the note parser.
                    let mut tipt = Text::plain(&info.chip());
                    let author = |t: &mut Text, md: &str| {
                        t.push("\n", Default::default());
                        t.append(Text::of(md, "\n"));
                    };
                    if let Some(m) = info.missing_reason() {
                        author(&mut tipt, &m);
                    }
                    for m in &info.messages {
                        tipt.push(&format!("\n{m}"), Default::default());
                    }
                    if let Some(n) = &s.note {
                        author(&mut tipt, n);
                    }
                    row_shell(("cmp-row", ix), ROW_H, tint, None, true, &c)
                        .cursor_pointer()
                        .map(|d| paint_tint(d, RowTint::None, None, cursor, &c))
                        .pl_2()
                        .text_size(px(12.5))
                        .child(div().flex_none().w(px(NUMBER_W)).child(step_badge(
                            s.number.clone(),
                            look,
                            &c,
                        )))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .w(relative(NAME_SHARE))
                                        .min_w_0()
                                        .pr_2()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .when(tint != RowTint::None, |d| {
                                            d.font_weight(FontWeight::MEDIUM)
                                        })
                                        .child(s.title.clone()),
                                )
                                .children(
                                    info.cells
                                        .iter()
                                        .map(|cell| cell_el(cell, side_share, &mono, &c)),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .w(px(RESULT_W))
                                .overflow_hidden()
                                .child(result_chip(&info, &c)),
                        )
                        .tooltip(super::note_tip(tipt))
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                            this.step_clicked(ix, cx);
                            if e.click_count() > 1 {
                                this.step_open(ix, cx);
                            }
                        }))
                        .into_any_element()
                }
            };
            // In the scrolling column a line keeps its height.
            lines.push(div().flex_none().w_full().child(line).into_any_element());
        }
        let filtered_out = !steps.is_empty() && shown_rows(&items).is_empty();
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .children(self.render_inv_notices(cx))
            .when(steps.is_empty() && st.at.is_some() && !st.checking, |d| {
                d.child(banner_box(&c).child("This comparison has no rows."))
            })
            .child(self.render_comparison_header(cx))
            .children(self.render_inv_overview(cx))
            .when(!steps.is_empty(), |d| d.child(heading_row))
            .when(filtered_out, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(11.5))
                        .text_color(c.text_muted)
                        .child("No row has that result."),
                )
            })
            .child(
                self.scroll_areas
                    .get("cmp-rows")
                    .tracking(st.cmp.scroll.clone())
                    .vertical(
                        "cmp-rows",
                        self.features.probe_if_on(Feature::Overscroll),
                        div().flex().flex_col().children(lines),
                    )
                    .flex_1()
                    .min_h_0(),
            )
            .child(self.render_comparison_hint(cx))
            .into_any_element()
    }

    /// The pane's header: the title, the pinned commit and which code is read, the counts
    /// (each a filter), and a variant picker for each side that has variants.
    fn render_comparison_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let Some(st) = &self.inv else {
            return div().into_any_element();
        };
        let rev = cho_git::short(&st.inv.rev).to_string();
        let at = match &st.at {
            Some(CodeAt::Pinned(_)) => format!("pinned to {rev} \u{b7} reading that commit"),
            Some(CodeAt::WorkingTree) => format!("pinned to {rev} \u{b7} reading the working tree"),
            None => format!("pinned to {rev}"),
        };
        let (d, m, s, u) = crate::comparison::counts(&st.steps);
        let filter = st.cmp.filter;
        let count = |o: Outcome, n: usize, word: String| {
            let on = filter == Some(o);
            div()
                .id(("cmp-count", o as usize))
                .flex_none()
                .px(px(5.))
                .h(px(18.))
                .flex()
                .items_center()
                .gap(px(3.))
                .rounded(px(4.))
                .border_1()
                .border_color(if on { c.text_accent } else { c.border })
                .when(on, |d| d.bg(c.accent_faint).text_color(c.text))
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .child(div().font_weight(FontWeight::BOLD).child(o.glyph()))
                .child(format!("{n} {word}"))
                .tooltip(tip(if on {
                    "Showing only these rows: click to show every row".to_string()
                } else {
                    format!("Show only the rows that are {}", o.word())
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(st) = &mut this.inv {
                        st.cmp.filter = if st.cmp.filter == Some(o) {
                            None
                        } else {
                            Some(o)
                        };
                    }
                    this.redraw(Some(Redraw::Steps), cx);
                }))
        };
        let mut counts = Vec::new();
        if d > 0 {
            counts.push(count(
                Outcome::Differs,
                d,
                if d == 1 { "differs" } else { "differ" }.into(),
            ));
        }
        if m > 0 {
            counts.push(count(Outcome::Missing, m, "missing".into()));
        }
        if s > 0 {
            counts.push(count(Outcome::Same, s, "same".into()));
        }
        if u > 0 {
            counts.push(count(Outcome::Unset, u, "without a result".into()));
        }
        let pickers = self.variant_pickers("cmp-variant", cx);
        div()
            .flex_none()
            .px_3()
            .py(px(4.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .border_b_1()
            .border_color(c.border_variant)
            .text_size(px(11.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(12.5))
                            .child(st.inv.title.clone()),
                    )
                    .child(div().flex_none().text_color(c.text_muted).child(at))
                    .child(div().flex_1())
                    .children(counts),
            )
            .children(pickers)
            .into_any_element()
    }

    /// A variant picker for each side that has variants.
    fn variant_pickers(&self, id: &'static str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = crate::pane_style::colors(&self.theme, cx);
        let Some(st) = &self.inv else {
            return Vec::new();
        };
        st.inv
            .sides
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.variants.is_empty())
            .map(|(i, side)| {
                let chosen = st.choice.variants.get(&side.id).cloned();
                let note = chosen
                    .as_ref()
                    .and_then(|v| side.variants.iter().find(|x| &x.id == v))
                    .and_then(|v| v.note.clone());
                let mut options = vec![(None, "Base path".to_string())];
                options.extend(
                    side.variants
                        .iter()
                        .map(|v| (Some(v.id.clone()), v.label.clone())),
                );
                let picker = choice_picker(
                    (id, i),
                    format!("{} \u{b7} {}", crate::comparison::letter(i), side.label),
                    options,
                    chosen,
                    {
                        let side = side.id.clone();
                        move |this: &mut Chorong, v: Option<String>, cx: &mut Context<Chorong>| {
                            this.set_variant(&side, v, cx)
                        }
                    },
                    &c,
                    cx,
                );
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(picker)
                    .children(note.map(|n| {
                        div()
                            .text_size(px(11.))
                            .text_color(c.text_muted)
                            .child(super::note_text_in(&Text::of(&n, "\n"), &c))
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// The choice in the tour rail: a tour's branch picker, or a comparison's variant
    /// pickers.
    pub(super) fn render_rail_choice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.comparison_open() {
            return self.render_branch_picker("rail-branch", cx);
        }
        let pickers = self.variant_pickers("rail-variant", cx);
        (!pickers.is_empty()).then(|| {
            div()
                .flex_none()
                .pb(px(4.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .text_size(px(11.5))
                .children(pickers)
                .into_any_element()
        })
    }

    /// The Comparison pane's one-line key hint, from the live keymap.
    fn render_comparison_hint(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let key = |name: &str, fallback: &str| {
            self.key_labels
                .get(name)
                .cloned()
                .unwrap_or_else(|| fallback.to_string())
        };
        let variants = self
            .inv
            .as_ref()
            .is_some_and(|s| s.inv.sides.iter().any(|s| !s.variants.is_empty()));
        let mut text = "j/k \u{b7} Enter open both sides".to_string();
        if variants {
            text.push_str(&format!(
                " \u{b7} {} variant",
                key("chorong::NextVariant", "v")
            ));
        }
        text.push_str(&format!(
            " \u{b7} {} as a tour \u{b7} Esc code \u{b7} ? keys",
            key("chorong::OpenComparisonAsTour", "Cmd-K Shift-C")
        ));
        hint_line(text, &c)
    }

    // ---- Choosing a variant or a branch ----

    /// Chooses `variant` of comparison side `side` (`None`: its base path): its rows are
    /// drawn again from the last check, and the choice is remembered.
    pub(super) fn set_variant(
        &mut self,
        side: &str,
        variant: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(st) = &mut self.inv else { return };
        let Some(s) = st.inv.sides.iter().find(|s| s.id == side) else {
            return;
        };
        if variant
            .as_ref()
            .is_some_and(|v| !s.variants.iter().any(|x| &x.id == v))
        {
            return;
        }
        match variant {
            Some(v) => {
                st.choice.variants.insert(side.to_string(), v);
            }
            None => {
                st.choice.variants.remove(side);
            }
        }
        self.choice_changed(cx);
    }

    /// Chooses a tour branch (`None`: the main path): steps it does not reach are muted and
    /// skipped, and the choice is remembered.
    pub(super) fn set_branch(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        let Some(st) = &mut self.inv else { return };
        if branch
            .as_ref()
            .is_some_and(|b| !st.inv.variants.iter().any(|v| &v.id == b))
        {
            return;
        }
        st.choice.branch = branch;
        self.choice_changed(cx);
    }

    /// `v` in the Comparison pane: the next variant of the first side that has any.
    pub(super) fn next_variant(&mut self, cx: &mut Context<Self>) {
        let Some(st) = self.inv.as_ref().filter(|s| *s.kind() == Kind::Comparison) else {
            self.notify_text("No comparison is open", cx);
            return;
        };
        let Some(side) = st.inv.sides.iter().find(|s| !s.variants.is_empty()) else {
            self.notify_text("This comparison has no variants", cx);
            return;
        };
        let ids: Vec<Option<String>> = std::iter::once(None)
            .chain(side.variants.iter().map(|v| Some(v.id.clone())))
            .collect();
        let now = st.choice.variants.get(&side.id).cloned();
        let at = ids.iter().position(|v| *v == now).unwrap_or(0);
        let next = ids[(at + 1) % ids.len()].clone();
        let side = side.id.clone();
        self.set_variant(&side, next, cx);
    }

    /// `b` in the Tour pane: the next branch (after the last, the main path).
    pub(super) fn next_branch(&mut self, cx: &mut Context<Self>) {
        let Some(st) = &self.inv else {
            self.notify_text("No tour is open", cx);
            return;
        };
        if st.inv.variants.is_empty() || *st.kind() == Kind::Comparison {
            self.notify_text("This investigation has no branches", cx);
            return;
        }
        let ids: Vec<Option<String>> = std::iter::once(None)
            .chain(st.inv.variants.iter().map(|v| Some(v.id.clone())))
            .collect();
        let at = ids.iter().position(|v| *v == st.choice.branch).unwrap_or(0);
        let next = ids[(at + 1) % ids.len()].clone();
        self.set_branch(next, cx);
    }

    /// After a branch or variant is chosen: remember it, draw the steps again from the last
    /// check, stay on the current step if it is still reached (else the nearest reached), and
    /// lay any open pages out again.
    fn choice_changed(&mut self, cx: &mut Context<Self>) {
        let Some(st) = &mut self.inv else { return };
        st.choice.at = inv::now_secs();
        let key = inv::source_key(&st.source);
        if let Err(e) = crate::comparison::save_choice(&key, &st.choice) {
            self.notify_text(format!("Could not remember the choice: {e}"), cx);
        }
        let Some(st) = &mut self.inv else { return };
        if crate::devlog::on() {
            eprintln!(
                "INV choice branch={} variants={}",
                st.choice.branch.as_deref().unwrap_or("main"),
                st.choice
                    .variants
                    .iter()
                    .map(|(s, v)| format!("{s}:{v}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        let (Some(report), Some(at)) = (st.report.clone(), st.at.clone()) else {
            self.redraw(Some(Redraw::Steps), cx);
            return;
        };
        let keep = st.steps.get(st.current).map(|s| s.id.clone());
        let (steps, _) = inv::derive_steps(&st.inv, &report, &at, &st.choice);
        let ix = keep
            .and_then(|id| steps.iter().position(|s| s.id == id))
            .unwrap_or(st.current);
        let ix = if inv::reached(&steps, ix) {
            ix
        } else {
            inv::step_to_reached(&steps, ix, 1)
        };
        st.steps = Arc::new(steps);
        st.current = ix.min(st.steps.len().saturating_sub(1));
        st.selected = st.current;
        let pages = st.pages.as_ref().map(|p| p.rail);
        let comparison = *st.kind() == Kind::Comparison;
        let ix = st.current;
        if let Some(rail) = pages {
            self.reopen_tour_pages(rail, cx);
        } else if !comparison {
            // The step on screen may be one the branch replaced or does not reach.
            self.step_open(ix, cx);
        } else {
            // A code pane still showing a side of a row from before the variant changed would
            // say what that side no longer is: its strip and tint go (Enter shows the row again).
            let stale: Vec<PaneId> = self
                .editors
                .iter()
                .filter(|(_, e)| e.step_mark.as_ref().is_some_and(|m| m.cell.is_some()))
                .map(|(p, _)| *p)
                .collect();
            for pane in stale {
                self.with_pane(pane, |this| {
                    this.ed_mut().step_mark = None;
                    this.sync_note_rows();
                });
                self.redraw(Some(Redraw::Editor(pane)), cx);
            }
        }
        self.redraw(Some(Redraw::Steps), cx);
        self.redraw_step_headers(cx);
        self.redraw_chrome(cx);
    }

    /// The pages were laid out for the old choice: restore the layout and open them again,
    /// at the current step.
    fn reopen_tour_pages(&mut self, rail: bool, cx: &mut Context<Self>) {
        let now = inv::now_secs();
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| ws.restore(now).map(|_| ()));
        });
        self.sync_panes(cx);
        self.tour_pages_follow(cx);
        self.open_tour_pages_with(rail, cx);
    }

    /// The branch picker, in the Tour pane's header and the tour rail: the main path and each
    /// branch, with the chosen branch's note. `None` when the tour has no branches.
    pub(super) fn render_branch_picker(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let st = self.inv.as_ref()?;
        if st.inv.variants.is_empty() || *st.kind() == Kind::Comparison {
            return None;
        }
        let c = crate::pane_style::colors(&self.theme, cx);
        let mut options = vec![(None, "Main path".to_string())];
        options.extend(
            st.inv
                .variants
                .iter()
                .map(|v| (Some(v.id.clone()), v.label.clone())),
        );
        let skipped = st
            .steps
            .iter()
            .filter(|s| s.extras.not_reached.is_some())
            .count();
        let note = st
            .choice
            .branch
            .as_ref()
            .and_then(|b| st.inv.variants.iter().find(|v| &v.id == b))
            .map(|v| {
                let mut t = v
                    .note
                    .as_deref()
                    .map(|n| Text::of(n, " "))
                    .unwrap_or_default();
                if skipped > 0 {
                    if !t.text.is_empty() {
                        t.push(" \u{b7} ", Default::default());
                    }
                    t.push(
                        &format!(
                            "{skipped} step{} not reached",
                            if skipped == 1 { "" } else { "s" }
                        ),
                        Default::default(),
                    );
                }
                t
            })
            .filter(|t| !t.text.is_empty());
        Some(
            div()
                .flex_none()
                .px_3()
                .py(px(4.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .border_b_1()
                .border_color(c.border_variant)
                .text_size(px(11.5))
                .child(choice_picker(
                    (id, 0usize),
                    "Path".into(),
                    options,
                    st.choice.branch.clone(),
                    |this: &mut Chorong, b: Option<String>, cx: &mut Context<Chorong>| {
                        this.set_branch(b, cx)
                    },
                    &c,
                    cx,
                ))
                .children(note.map(|n| {
                    div()
                        .text_size(px(11.))
                        .text_color(c.text_muted)
                        .child(super::note_text_in(&n, &c))
                }))
                .into_any_element(),
        )
    }

    /// In an editor's note strip on a comparison row's page: the row's result and callout.
    pub(super) fn render_row_result(&self, info: &RowInfo, c: &Colors) -> Div {
        let error = c.error.unwrap_or(c.text_accent);
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(result_chip(info, c))
            .children(info.callout.clone().map(|t| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(error)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(11.5))
                    .child(small_icon("git-compare", error))
                    .child(super::note_text_in(&Text::of(&t, " "), c))
            }))
            .children(info.missing_reason().map(|m| {
                div()
                    .italic()
                    .text_size(px(11.5))
                    .text_color(c.text_muted)
                    .child(super::note_text_in(&Text::of(&m, " "), c))
            }))
    }
}

/// A labelled row of segment buttons, one per option, the chosen one marked (by a border
/// and fill as well as colour). `on_pick` gets the option's id.
fn choice_picker(
    id: (&'static str, usize),
    label: String,
    options: Vec<(Option<String>, String)>,
    chosen: Option<String>,
    on_pick: impl Fn(&mut Chorong, Option<String>, &mut Context<Chorong>) + Clone + 'static,
    c: &Colors,
    cx: &mut Context<Chorong>,
) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(4.))
        .child(
            div()
                .flex_none()
                .text_color(c.text_muted)
                .child(format!("{label}:")),
        )
        .children(options.into_iter().enumerate().map(|(k, (value, text))| {
            let on = value == chosen;
            let pick = on_pick.clone();
            div()
                .id((id.0, id.1 * 100 + k))
                .flex_none()
                .px(px(6.))
                .h(px(20.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded(px(4.))
                .border_1()
                .border_color(if on { c.text_accent } else { c.border })
                .when(on, |d| {
                    d.bg(c.accent_faint)
                        .text_color(c.text)
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!on, |d| d.text_color(c.text_muted))
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .when(on, |d| d.child(small_icon("chevron-right", c.icon_accent)))
                .child(text)
                .on_click(cx.listener(move |this, _, _, cx| pick(this, value.clone(), cx)))
        }))
        .into_any_element()
}

/// A step pane's one-line key hint.
pub(super) fn hint_line(text: String, c: &Colors) -> AnyElement {
    div()
        .flex_none()
        .px_3()
        .h(px(22.))
        .flex()
        .items_center()
        .border_t_1()
        .border_color(c.border)
        .text_size(px(11.))
        .text_color(c.text_muted)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(text)
        .into_any_element()
}

actions!(chorong, [NextBranch, NextVariant, OpenComparisonAsTour]);

pub(super) const COMMANDS: &[Command] = &[
    c(
        "chorong::OpenComparisonAsTour",
        "Open Comparison as Tour",
        Layer::Comparison,
        360,
    ),
    c(
        "chorong::NextVariant",
        "Next Variant (comparison side)",
        Layer::Comparison,
        361,
    ),
    c(
        "chorong::NextBranch",
        "Next Branch (tour path)",
        Layer::Comparison,
        362,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, OpenComparisonAsTour, |this, _w, cx| this
        .open_comparison_tour(cx));
    let d = on!(d, cx, NextVariant, |this, _w, cx| this.next_variant(cx));
    on!(d, cx, NextBranch, |this, _w, cx| this.next_branch(cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comparison::Source;
    use std::sync::Arc;

    fn row(phase: Phase, outcome: Outcome, callout: bool) -> Step {
        Step {
            id: "r".into(),
            n: 1,
            title: "row".into(),
            symbol: None,
            path: String::new(),
            label: inv::Label::Verified,
            reason: None,
            range: None,
            region: None,
            name_start: None,
            line: None,
            note: None,
            number: "1".into(),
            extras: crate::tour_pages::StepExtras {
                phase: Some(phase),
                row: Some(Arc::new(RowInfo {
                    outcome,
                    source: Source::Claimed,
                    same_code: false,
                    callout: callout.then(|| "Paths split here".into()),
                    cells: vec![],
                    editor_cells: vec![],
                    messages: vec![],
                })),
                ..Default::default()
            },
        }
    }

    #[test]
    fn setup_rows_are_their_own_group_and_callouts_follow_their_row() {
        let steps = vec![
            row(Phase::Setup, Outcome::Missing, false),
            row(Phase::Call, Outcome::Differs, true),
            row(Phase::Call, Outcome::Same, false),
        ];
        let mut v = CmpView::default();
        assert_eq!(
            cmp_items(&steps, &v),
            [
                CmpItem::Phase(Phase::Setup, 1),
                CmpItem::Row(0),
                CmpItem::Phase(Phase::Call, 2),
                CmpItem::Row(1),
                CmpItem::Callout(1),
                CmpItem::Row(2),
            ]
        );
        v.setup_collapsed = true;
        v.filter = Some(Outcome::Same);
        let items = cmp_items(&steps, &v);
        assert_eq!(
            items,
            [
                CmpItem::Phase(Phase::Setup, 0),
                CmpItem::Phase(Phase::Call, 1),
                CmpItem::Row(2),
            ]
        );
        assert_eq!(shown_rows(&items), [2]);
    }

    #[test]
    fn keys_move_over_the_rows_shown() {
        let shown = [1, 3, 4];
        assert_eq!(step_shown(&shown, 1, 1), Some(3));
        assert_eq!(step_shown(&shown, 4, 1), Some(4));
        assert_eq!(step_shown(&shown, 1, -1), Some(1));
        // From a hidden row: the next shown one in the direction moved.
        assert_eq!(step_shown(&shown, 2, 1), Some(3));
        assert_eq!(step_shown(&shown, 2, -1), Some(1));
        assert_eq!(step_shown(&[], 2, 1), None);
    }
}

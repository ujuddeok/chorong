//! The Call stack pane (`PaneKind::CallStack`, a bottom-bar tab): one frame per step, outermost
//! call first (design/layout.md, "Call stacks and graphs").
//!
//! Each frame shows its index, a status dot, the function glyph, the qualified name, its
//! location and a verification chip. The three labels differ by shape as well as colour, so
//! they read without colour: verified is a solid dot and a filled chip; claimed a dashed dot
//! and a dashed chip, with the name underlined by a dashed line; broken a hollow dot, the name
//! struck through and the location replaced by the reason.

use super::investigation_view::{
    banner_box, empty_pane, evidence_tag, phase_heading, runtime_chip,
};
use super::row_list::{paint_tint, step_row_tint};
use super::*;
use crate::investigation::{self as inv, CodeAt, Label};
use cho_investigation::Kind;
use cho_layout_ui::colors::kind_glyph;
use cho_layout_ui::widgets::small_icon;

const ROW_H: f32 = 24.;

impl Chorong {
    fn callstack_open(&self) -> bool {
        // A tour shows here too while its pages are open: a page may ask for this
        // investigation's call stack.
        self.inv
            .as_ref()
            .is_some_and(|s| *s.kind() == Kind::Callstack || s.pages.is_some())
    }

    pub(super) fn render_callstack(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let Some(st) = self.inv.as_ref().filter(|_| self.callstack_open()) else {
            return empty_pane("call stack", &c);
        };
        let steps = st.steps.clone();
        let (current, selected) = (st.current, st.selected);
        let keyboard = self.key_focus == KeyFocus::Steps;
        let (verified, claimed, broken) = inv::counts(&steps);
        let (ok, warn, bad) = (
            c.success.unwrap_or(c.text_accent),
            c.warning.unwrap_or(c.text_accent),
            c.error.unwrap_or(c.text),
        );
        let at = match &st.at {
            Some(CodeAt::Pinned(r)) => format!("at {} (read from git)", cho_git::short(r)),
            Some(CodeAt::WorkingTree) => format!(
                "pinned to {} \u{b7} working tree",
                cho_git::short(&st.inv.rev)
            ),
            None => format!("pinned to {}", cho_git::short(&st.inv.rev)),
        };
        let count = |n: usize, label: Label, colour: gpui::Hsla| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(status_dot(label, colour))
                .child(format!("{n} {}", label.word()))
        };
        let header = div()
            .flex_none()
            .px_3()
            .h(px(26.))
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(c.border)
            .text_size(px(12.))
            .whitespace_nowrap()
            .overflow_hidden()
            .child(
                div()
                    .min_w_0()
                    .pr_1()
                    .font_weight(FontWeight::MEDIUM)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(st.inv.title.clone()),
            )
            .child(div().text_color(c.text_muted).child(at))
            .when_some(st.host.clone(), |d, host| {
                d.child(div().text_color(c.text_muted).child(format!("from {host}")))
            })
            .when(!steps.is_empty(), |d| {
                d.child(
                    div()
                        .ml_auto()
                        .flex()
                        .gap_3()
                        .text_color(c.text_muted)
                        .child(count(verified, Label::Verified, ok))
                        .child(count(claimed, Label::Claimed, warn))
                        .child(count(broken, Label::Broken, bad)),
                )
            });
        let font = self.font.clone();
        let items = inv::list_items(&steps);
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .child(header)
            .children(self.render_inv_overview(cx))
            .children(self.render_inv_notices(cx))
            .when(steps.is_empty() && st.at.is_some() && !st.checking, |d| {
                d.child(banner_box(&c).child("This call stack has no frames."))
            })
            .child(
                crate::scroll::uniform_list(
                    "callstack-frames",
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
                                let external = s.is_external();
                                let colour = match s.label {
                                    Label::Verified => ok,
                                    Label::Claimed => warn,
                                    Label::Broken => bad,
                                };
                                let is_current = ix == current;
                                let is_selected = keyboard && ix == selected;
                                let glyph = match &s.symbol {
                                    _ if external => "external",
                                    Some(sym) if sym.contains("::") || sym.contains('.') => {
                                        "method"
                                    }
                                    Some(_) => "function",
                                    None => "file",
                                };
                                let loc = match (s.label, &s.reason) {
                                    _ if external => "not in the code".to_string(),
                                    (Label::Broken, Some(r)) => r.clone(),
                                    _ => match s.line {
                                        Some(l) => format!("{}:{l}", s.path),
                                        None => s.path.clone(),
                                    },
                                };
                                let name = div()
                                    .flex_none()
                                    .max_w(gpui::relative(0.5))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .font_family(font.clone())
                                    .child(match (&s.extras.name, &s.symbol) {
                                        (Some(n), _) => n.clone(),
                                        (None, Some(sym)) if !external => sym.clone(),
                                        _ => s.title.clone(),
                                    });
                                let name = match s.label {
                                    _ if external => name.italic(),
                                    Label::Verified => name,
                                    // A dashed line under the name: claimed.
                                    Label::Claimed => {
                                        name.border_b_1().border_dashed().border_color(warn)
                                    }
                                    Label::Broken => name.line_through().text_color(c.text_muted),
                                };
                                let chip = div()
                                    .flex_none()
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .text_size(px(10.5))
                                    .text_color(colour)
                                    .child(s.label.word());
                                let chip = match s.label {
                                    Label::Verified => chip.bg(c.success_bg.unwrap_or(c.hover)),
                                    Label::Claimed => {
                                        chip.border_1().border_dashed().border_color(warn)
                                    }
                                    Label::Broken => chip.border_1().border_color(bad),
                                };
                                Some(
                                    div()
                                        .id(("frame", ix))
                                        .w_full()
                                        .h(px(ROW_H))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .whitespace_nowrap()
                                        .cursor_pointer()
                                        .map(|d| {
                                            let tint = step_row_tint(is_selected, is_current);
                                            paint_tint(d, tint, None, is_selected, &c)
                                        })
                                        .hover(|s| s.bg(c.hover))
                                        .child(
                                            div()
                                                .flex_none()
                                                .w(px(18.))
                                                .text_right()
                                                .text_color(c.text_muted)
                                                .child(s.number.clone()),
                                        )
                                        .child(if external {
                                            // No verification: a dotted square, not a dot.
                                            div()
                                                .flex_none()
                                                .size(px(9.))
                                                .rounded(px(2.))
                                                .border_1()
                                                .border_dashed()
                                                .border_color(c.icon)
                                        } else {
                                            status_dot(s.label, colour)
                                        })
                                        .child(if external {
                                            small_icon("box", c.icon)
                                        } else {
                                            small_icon(kind_glyph(glyph), c.kind(glyph))
                                        })
                                        .child(name)
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .text_size(px(11.5))
                                                .text_color(if s.label == Label::Broken {
                                                    bad
                                                } else {
                                                    c.text_muted
                                                })
                                                .when(external, |d| d.italic())
                                                .child(loc),
                                        )
                                        .children(
                                            s.extras.runtime.iter().map(|v| runtime_chip(v, &c)),
                                        )
                                        .children(s.extras.evidence.map(|e| evidence_tag(e, &c)))
                                        .when(!external, |d| d.child(chip))
                                        .when_some(
                                            s.reason.clone().filter(|_| !external),
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

    /// The Call stack tab's qualifier: `3/5`.
    pub(super) fn callstack_qualifier(&self) -> String {
        self.inv
            .as_ref()
            .filter(|_| self.callstack_open())
            .and_then(|s| s.position())
            .unwrap_or_default()
    }
}

/// The status dot: solid (verified), dashed (claimed) or hollow (broken).
fn status_dot(label: Label, colour: gpui::Hsla) -> Div {
    let d = div().flex_none().size(px(9.)).rounded_full();
    match label {
        Label::Verified => d.bg(colour),
        Label::Claimed => d.border(px(1.5)).border_dashed().border_color(colour),
        Label::Broken => d.border(px(1.5)).border_color(colour),
    }
}

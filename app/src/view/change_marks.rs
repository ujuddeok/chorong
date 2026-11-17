//! Change highlights in the code while comparing: faint line tints in the theme's diff colours,
//! a darker block on the changed words of paired lines, and a rule across the code where the
//! other side has lines this side lacks (a deletion on After, an addition on Before).
//!
//! Everything drawn here comes from the marks worked out once per file and comparison (the
//! `Gutter` and the diff's `Words`); a frame only draws the visible lines. The strength is the
//! `compare.line_highlight` setting (off, subtle, stronger) and the word blocks are
//! `compare.changed_words`; both are off in Lightweight mode. The order of strength is always
//! line tint < word block < rule.

use super::changes::ChangePlace;
use super::compare::{Diff, Gap, Words};
use super::*;

/// Line tint, word block and rule opacity for each strength (on the theme's colour).
const SUBTLE: Strength = Strength {
    tint: 0.05,
    word: 0.13,
    rule: 0.5,
};
const STRONGER: Strength = Strength {
    tint: 0.11,
    word: 0.2,
    rule: 0.6,
};
/// Bumped when a window-wide way of drawing the marks changes (Highlight Uncommitted): part of
/// each code pane's body key, so the cached bodies draw anew.
pub(super) static MARKS_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// How long a followed declaration stays highlighted (`compare.follow_highlight`).
const FLASH: std::time::Duration = std::time::Duration::from_millis(900);
/// The fade's frame interval.
const FLASH_STEP: std::time::Duration = std::time::Duration::from_millis(33);
/// The fade's starting opacity (on the accent colour).
const FLASH_ALPHA: f32 = 0.3;

/// Word blocks with the line tint off.
const WORDS_ONLY: Strength = Strength {
    tint: 0.,
    word: 0.13,
    rule: 0.,
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Strength {
    tint: f32,
    word: f32,
    rule: f32,
}

/// How one code pane draws its comparison marks this frame.
#[derive(Clone)]
pub(super) struct ChangeStyle {
    pub gutter: Arc<Gutter>,
    pub diff: Option<Arc<Diff>>,
    pub side: Side,
    /// The side's own colour: green on After, red on Before.
    pub colour: gpui::Hsla,
    /// The other side's colour, for deletion markers: red on After, green on Before.
    pub other: gpui::Hsla,
    /// The theme's "modified" colour: uncommitted work.
    pub uncommitted: gpui::Hsla,
    /// The neutral triangle colour with tints off.
    pub neutral: gpui::Hsla,
    /// Line tints and rules are drawn.
    pub lines: bool,
    pub words: bool,
    /// Highlight Uncommitted is on: uncommitted lines are tinted yellow, the rest dimmer.
    pub highlight: bool,
    strength: Strength,
}

impl ChangeStyle {
    /// Only uncommitted work changed `line` (on Before: removed or changed it).
    fn uncommitted_only(&self, line: usize) -> bool {
        !self.gutter.changed(line) && self.gutter.hollow(line)
    }

    /// The line's tint, when it changed (committed or not) and tints are on. Uncommitted lines
    /// keep the side's tint; Highlight Uncommitted tints them yellow and dims the rest.
    pub fn tint(&self, line: usize) -> Option<gpui::Hsla> {
        if !self.lines || !self.gutter.any_change(line) {
            return None;
        }
        let tint = self.strength.tint;
        Some(match (self.highlight, self.uncommitted_only(line)) {
            (false, _) => self.colour.opacity(tint),
            (true, true) => self.uncommitted.opacity(tint),
            (true, false) => self.colour.opacity(tint * 0.6),
        })
    }

    /// The line number's colour on a line only uncommitted work changed: the theme's
    /// `modified`, so the run reads at a glance beside its bracket.
    pub fn number_colour(&self, line: usize) -> Option<gpui::Hsla> {
        self.uncommitted_only(line).then_some(self.uncommitted)
    }

    /// The yellow bracket in the gutter: a thin stroke down a run of lines only uncommitted
    /// work changed, with a tick at its first and last line, and a tick on a boundary where
    /// uncommitted work removed lines (`line_count`: the file's lines, for the boundary after
    /// the last). Committed lines have bars instead.
    pub fn bracket(&self, line: usize, line_count: usize) -> Option<gpui::AnyElement> {
        let g = &self.gutter;
        let run = self.uncommitted_only(line);
        let top = run && (line == 0 || !self.uncommitted_only(line - 1));
        let bottom = run && !self.uncommitted_only(line + 1);
        let gap_tick =
            |at: usize| g.wedge_above(at) && g.gap(at).is_some_and(|gap| gap.uncommitted);
        let tick_above = gap_tick(line);
        let tick_below = line + 1 == line_count && gap_tick(line_count);
        if !run && !tick_above && !tick_below {
            return None;
        }
        let colour = self.uncommitted;
        Some(
            gpui::canvas(
                |_, _, _| (),
                move |b, _, window, _| {
                    let (x, y0, h) = (b.origin.x, b.origin.y, b.size.height);
                    let quad = |window: &mut Window,
                                x: gpui::Pixels,
                                y: gpui::Pixels,
                                w: gpui::Pixels,
                                h: gpui::Pixels| {
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(gpui::point(x, y), gpui::size(w, h)),
                            colour,
                        ));
                    };
                    let stroke = px(2.);
                    let tick = px(5.);
                    if run {
                        let from = if top { y0 + px(2.) } else { y0 };
                        let to = if bottom { y0 + h - px(2.) } else { y0 + h };
                        quad(window, x + px(1.), from, stroke, to - from);
                        if top {
                            quad(window, x + px(1.), from, tick, stroke);
                        }
                        if bottom {
                            quad(window, x + px(1.), to - stroke, tick, stroke);
                        }
                    }
                    // A deletion's tick reaches past the triangle beside it, so it reads.
                    let long = px(12.);
                    if tick_above {
                        quad(window, x + px(1.), y0 - px(1.), long, stroke);
                    }
                    if tick_below {
                        quad(window, x + px(1.), y0 + h - px(1.), long, stroke);
                    }
                },
            )
            .absolute()
            .left_0()
            .top_0()
            .w(px(14.))
            .h(px(LINE_HEIGHT))
            .into_any_element(),
        )
    }

    /// The gutter bar's colour for `line`, if committed work changed it (uncommitted lines
    /// have the bracket).
    pub fn bar(&self, line: usize) -> Option<gpui::Hsla> {
        self.gutter.changed(line).then_some(self.colour)
    }

    /// The triangle's colour for a gap, and whether it is filled.
    pub fn wedge(&self, uncommitted: bool) -> (gpui::Hsla, bool) {
        // An uncommitted gap's yellow is the bracket's tick over the triangle.
        match (self.lines, uncommitted) {
            (true, _) => (self.other, true),
            (false, hollow) => (self.neutral, !hollow),
        }
    }

    fn words_of(&self) -> Option<&Words> {
        self.words
            .then(|| self.diff.as_deref().map(|d| &d.words))
            .flatten()
    }

    /// The line differs from its pair only in whitespace.
    pub fn whitespace_only(&self, line: usize) -> bool {
        self.diff
            .as_ref()
            .is_some_and(|d| d.words.whitespace_only(self.side, line))
    }

    /// The changed-word blocks of one line, absolutely positioned behind the text like the
    /// highlight markers (x = display column times the cell width).
    pub fn word_boxes(
        &self,
        line_ix: usize,
        line: &crate::document::DisplayLine,
        advance: f32,
    ) -> Vec<gpui::Div> {
        let Some(words) = self.words_of() else {
            return Vec::new();
        };
        let fill = self.colour.opacity(self.strength.word);
        words
            .on(self.side, line_ix)
            .iter()
            .filter_map(|r| {
                let abs = line.bytes.start + r.start..line.bytes.start + r.end;
                let display = line.display_range(&abs)?;
                let (x, w) = markers::extent(&line.text, &display, advance);
                Some(
                    div()
                        .absolute()
                        .left(px(x + markers::PAD))
                        .top(px(1.))
                        .w(px(w - 2. * markers::PAD))
                        .h(px(LINE_HEIGHT - 2.))
                        .rounded(px(2.))
                        .bg(fill),
                )
            })
            .collect()
    }

    /// The deletion marker on a line boundary (`at_end`: below the last line, else above
    /// `line_ix`): a 1 px rule across the code from `left` (yellow for uncommitted work while
    /// Highlight Uncommitted is on), with a taller invisible strip that takes the pointer.
    pub fn rule(
        &self,
        pane: PaneId,
        line_ix: usize,
        gap: Gap,
        at_end: bool,
        left: f32,
        cx: &mut Context<Chorong>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        if !self.lines {
            return None;
        }
        // Green reads only at full strength on a dark background; red at half.
        let other = if self.side == Side::Before {
            self.other
        } else {
            self.other.opacity(self.strength.rule)
        };
        let colour = if gap.uncommitted && self.highlight {
            self.uncommitted
        } else {
            other
        };
        let y = if at_end { LINE_HEIGHT } else { 0. };
        let text = super::compare::gap_text(self.side, &gap);
        Some(
            div()
                .id(("change-rule", line_ix * 2 + usize::from(at_end)))
                .absolute()
                .left(px(left))
                .right_0()
                .top(px(y - 4.))
                .h(px(8.))
                .cursor_pointer()
                .child(
                    gpui::canvas(
                        |_, _, _| (),
                        move |b, _, window, _| {
                            let y = b.origin.y + px(4.);
                            window.paint_quad(gpui::fill(
                                gpui::Bounds::new(
                                    gpui::point(b.origin.x, y),
                                    gpui::size(b.size.width, px(1.)),
                                ),
                                colour,
                            ));
                        },
                    )
                    .size_full(),
                )
                .tooltip(tip(text))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.jump_to_gap(pane, gap, cx);
                    cx.notify();
                })),
        )
    }
}

impl Chorong {
    /// `u` / Shift-U: the caret goes to the next (previous) stretch of uncommitted work in the
    /// active pane, wrapping round.
    pub(super) fn step_uncommitted(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.compare.is_none() {
            self.notify_text("Not comparing: use Compare Against\u{2026} first", cx);
            return;
        }
        let ed = self.ed();
        let (Some(g), Some(doc)) = (ed.gutter.clone(), ed.doc.clone()) else {
            return;
        };
        let runs = g.uncommitted_runs();
        let here = ed.caret.map(|c| doc.buffer.line_of(c));
        let Some(line) = super::compare::next_run(&runs, here, forward) else {
            self.notify_text("No uncommitted changes in this file", cx);
            return;
        };
        let byte = doc.buffer.line_range(line).map_or(0, |r| r.start);
        self.set_caret(byte, true, cx);
        let at = runs.iter().position(|r| *r == line).map_or(0, |i| i + 1);
        self.notify_text(format!("Uncommitted change {at} of {}", runs.len()), cx);
        self.dirty_pane = Some(self.active_editor);
    }

    /// Cmd-K U: uncommitted lines get a yellow tint (and the other changes dim) until it is
    /// pressed again.
    pub(super) fn toggle_highlight_uncommitted(&mut self, cx: &mut Context<Self>) {
        self.highlight_uncommitted = !self.highlight_uncommitted;
        self.notify_text(
            if self.highlight_uncommitted {
                "Highlighting uncommitted changes"
            } else {
                "Uncommitted changes shown as usual"
            },
            cx,
        );
        // Code panes are cached views: a new body key makes them draw anew.
        MARKS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.dirty_all = true;
    }

    /// The header chip of a code pane whose file has uncommitted work while comparing ("3
    /// uncommitted"); a click goes to the next one.
    pub(super) fn uncommitted_chip(
        &self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.compare.as_ref()?;
        let n = self.ed_of(pane).gutter.as_ref()?.uncommitted_runs().len();
        if n == 0 {
            return None;
        }
        let c = crate::pane_style::colors(&self.theme, cx);
        let yellow = self.theme.ui.modified.map_or(c.text_accent, hsla);
        Some(
            div()
                .id(("uncommitted-chip", pane.0 as usize))
                .flex_none()
                .mr_1()
                .px(px(5.))
                .h(px(16.))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(10.5))
                .text_color(yellow)
                .bg(yellow.opacity(if self.highlight_uncommitted {
                    0.28
                } else {
                    0.14
                }))
                .hover(|s| s.bg(yellow.opacity(0.24)))
                .tooltip(tip(self.tip_text(
                    "Go to the next uncommitted change (Shift-U: previous)",
                    "chorong::NextUncommitted",
                )))
                .child(format!("{n} uncommitted"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_editor(pane, cx);
                    this.step_uncommitted(true, cx);
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// The chain on a column of a linked split, beside its side chip: linked (accent) or
    /// unlinked (muted, broken chain). A click toggles the link.
    pub(super) fn split_link_button(
        &self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let link = self.links.iter().find(|l| l.has(pane))?;
        let c = crate::pane_style::colors(&self.theme, cx);
        let (icon, colour, text) = if link.linked {
            (
                "link",
                c.text_accent,
                "Linked: the two columns follow each other's declaration. Click to unlink",
            )
        } else {
            (
                "unlink",
                c.icon_muted,
                "Unlinked: the columns move on their own. Click to link",
            )
        };
        Some(
            div()
                .id(("split-link", pane.0 as usize))
                .w(px(20.))
                .h(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .active(|s| s.bg(c.ghost_active))
                .tooltip(tip(self.tip_text(text, "chorong::ToggleSplitLink")))
                .child(
                    gpui::svg()
                        .path(crate::assets::icon(icon))
                        .size(px(14.))
                        .flex_none()
                        .text_color(colour),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_editor(pane, cx);
                    this.toggle_split_link(cx);
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// Highlights `line` of `pane` briefly: a linked column followed its partner there. A fade
    /// redraws the pane every frame for [`FLASH`]; an outline once at its end.
    pub(super) fn start_flash(&mut self, pane: PaneId, line: usize, cx: &mut Context<Self>) {
        if !self.features.on(Feature::FollowHighlight) {
            return;
        }
        let fade = self.features.settings().compare_follow_highlight == "fade";
        let started = std::time::Instant::now();
        let Some(ed) = self.editors.get_mut(&pane) else {
            return;
        };
        ed.flash = Some((line, started));
        let probe = self.features.probe(Feature::FollowHighlight);
        cx.spawn(async move |this, cx| {
            loop {
                let step = if fade { FLASH_STEP } else { FLASH };
                cx.background_executor().timer(step).await;
                let done = this
                    .update(cx, |this, cx| {
                        let _t = probe.start();
                        let Some(ed) = this.editors.get_mut(&pane) else {
                            return true;
                        };
                        // A newer follow took over, or the caret moved on.
                        if ed.flash.map(|(_, at)| at) != Some(started) {
                            return true;
                        }
                        let over = started.elapsed() >= FLASH;
                        if over {
                            ed.flash = None;
                        }
                        this.shell
                            .update(cx, |shell, cx| shell.refresh_pane(pane, cx));
                        over
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// The follow highlight on `line_ix`, if it is the flashed line: a fading tint, or a still
    /// outline, across the code from `left`.
    pub(super) fn flash_overlay(
        &self,
        ed: &EditorState,
        line_ix: usize,
        left: f32,
        cx: &mut App,
    ) -> Option<gpui::Div> {
        let (line, at) = ed.flash?;
        if line != line_ix {
            return None;
        }
        let accent = crate::pane_style::colors(&self.theme, cx).text_accent;
        let base = div()
            .absolute()
            .top_0()
            .left(px(left))
            .right_0()
            .h(px(LINE_HEIGHT));
        if self.features.settings().compare_follow_highlight == "fade" {
            let t = (at.elapsed().as_secs_f32() / FLASH.as_secs_f32()).min(1.);
            Some(base.bg(accent.opacity(FLASH_ALPHA * (1. - t))))
        } else {
            Some(base.border_1().border_color(accent.opacity(0.7)))
        }
    }

    /// How `ed` draws its comparison marks, when it has any.
    pub(super) fn change_style(&self, ed: &EditorState, cx: &mut App) -> Option<ChangeStyle> {
        let gutter = ed.gutter.clone()?;
        let c = crate::pane_style::colors(&self.theme, cx);
        // The theme's diff colours (`created`, `deleted`, `modified`), else its status colours.
        let ui = &self.theme.ui;
        let text = hsla(self.theme.text);
        let (added, removed) = (ui.created.map_or(text, hsla), ui.deleted.map_or(text, hsla));
        let uncommitted = ui.modified.map_or(c.text_accent, hsla);
        let (colour, other) = match ed.side {
            Side::After => (added, removed),
            Side::Before => (removed, added),
        };
        let lines = self.features.on(Feature::LineHighlight);
        let words = self.features.on(Feature::ChangedWords);
        let strength = match (
            self.features.settings().compare_line_highlight.as_str(),
            lines,
        ) {
            ("stronger", true) => STRONGER,
            (_, true) => SUBTLE,
            (_, false) => WORDS_ONLY,
        };
        Some(ChangeStyle {
            gutter,
            diff: ed.diff.clone(),
            side: ed.side,
            colour,
            other,
            uncommitted,
            neutral: c.icon,
            lines,
            words,
            highlight: self.highlight_uncommitted,
            strength,
        })
    }

    /// A deletion marker was clicked: shows the other side at that spot. In a linked pair the
    /// other column scrolls there; otherwise the pane switches sides and lands there.
    pub(super) fn jump_to_gap(&mut self, pane: PaneId, gap: Gap, cx: &mut Context<Self>) {
        if self.compare.is_none() {
            return;
        }
        let side = self.ed_of(pane).side;
        if let Some(link) = self.links.iter().copied().find(|l| l.has(pane)) {
            let other = link.other(pane);
            let Some(doc) = self.ed_of(other).doc.clone() else {
                return;
            };
            let line = gap.other.min(doc.lines.len().saturating_sub(1));
            let byte = doc.buffer.line_range(line).map_or(0, |r| r.start);
            self.following = true;
            self.with_pane(other, |this| {
                this.place_caret(byte, super::navigation::Reveal::IfHidden, cx)
            });
            self.following = false;
            self.shell
                .update(cx, |shell, cx| shell.refresh_pane(other, cx));
            return;
        }
        self.activate_editor(pane, cx);
        let place = ChangePlace {
            path: String::new(),
            deleted: false,
            side: side.flip(),
            key: None,
            nth: 0,
            line: Some(gap.other),
        };
        self.apply_place(&place, cx);
        self.dirty_pane = Some(pane);
    }
}

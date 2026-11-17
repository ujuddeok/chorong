//! Find in File (Cmd-F): a find bar at the top of the active code pane. Its matches are the
//! pane's Find highlight group (`marks::Mode::Find`), so `n` / `N`, the gutter, the overview
//! lane and the minimap show them; Enter and Shift-Enter step through them, wrapping. The
//! matching itself is `crate::find`.
//!
//! While the bar's query has the keyboard the root key context is `Chorong Find`, never
//! `CodeView`: GPUI matches a binding at any depth of the context stack, so the code view's
//! single-letter keys would otherwise eat the letters typed.

use super::*;
use crate::find::FindBar;
use crate::marks::step_occurrence;
use gpui::AnyElement;

/// The bar's keys as bound in its own context, for its hints.
const SHOWN: [&str; 5] = [
    "chorong::FindNext",
    "chorong::FindPrev",
    "chorong::FindClose",
    "chorong::FindToggleCase",
    "chorong::FindToggleRegex",
];

/// The key context while the bar's query has the keyboard.
pub(super) const CONTEXT: &str = "Chorong Find";

/// What the window keeps for the find bars: the last bar closed (its query and toggles seed
/// the next one) and the bar's key labels from the live keymap.
#[derive(Default)]
pub(super) struct FindShared {
    last: Option<FindBar>,
    keys: HashMap<&'static str, String>,
    keys_for: Option<gpui::KeymapVersion>,
}

impl Chorong {
    /// Keystrokes go to the find bar's query.
    pub(super) fn find_focused(&self) -> bool {
        self.key_focus == KeyFocus::Find && self.ed().find.is_some()
    }

    /// Called from `Chorong::render`, before the shell draws the panes (which must not read the
    /// shell): the bar's key labels, read again when the keymap changes, and the keyboard
    /// back to the code when the active pane has no bar (another pane was activated).
    pub(super) fn prepare_find(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.key_focus == KeyFocus::Find && self.ed().find.is_none() {
            self.key_focus = KeyFocus::Code;
        }
        let version = cx.key_bindings().borrow().version();
        if self.find.keys_for == Some(version) {
            return;
        }
        self.find.keys_for = Some(version);
        self.find.keys = SHOWN
            .iter()
            .filter_map(|name| {
                let action = cx.build_action(name, None).ok()?;
                let label = cho_layout_ui::keys::label_in(action.as_ref(), window, CONTEXT)?;
                Some((*name, label))
            })
            .collect();
    }

    /// Cmd-F: opens the bar on the active code pane with the last query selected, or, when it
    /// is open, gives it the keyboard and selects its query.
    pub(super) fn open_find(&mut self, cx: &mut Context<Self>) {
        if self.ed().doc.is_none() {
            self.notify_text("Open a file first", cx);
            return;
        }
        let caret = self.ed().caret.unwrap_or(0);
        if let Some(bar) = &mut self.ed_mut().find {
            bar.query_all = !bar.query.is_empty();
            bar.origin = caret;
        } else {
            let mut bar = FindBar::new(String::new(), caret);
            if let Some(last) = &self.find.last {
                bar = FindBar {
                    case_sensitive: last.case_sensitive,
                    regex: last.regex,
                    ..FindBar::new(last.query.clone(), caret)
                };
            }
            self.ed_mut().find = Some(bar);
            self.refind(false, cx);
        }
        self.focus_to(KeyFocus::Find);
        self.touch_pane();
    }

    /// Esc in the bar: closes it and removes its highlight group.
    pub(super) fn close_find(&mut self, cx: &mut Context<Self>) {
        let Some(bar) = self.ed_mut().find.take() else {
            return;
        };
        self.find.last = Some(bar);
        self.key_focus.leave(KeyFocus::Find);
        let had = !self.ed().marks.is_empty();
        self.ed_mut().marks.set_find("", Vec::new(), 0);
        self.touch_pane();
        if had && self.ed().marks.is_empty() {
            self.highlights_emptied(cx);
        }
    }

    /// After a highlight group was removed some other way (Esc in the code, a chip's cross,
    /// Clear all): if it was the find group, the bar closes too, so it never counts matches
    /// that are no longer shown.
    pub(super) fn find_group_gone(&mut self, cx: &mut Context<Self>) {
        let ed = self.ed();
        if ed
            .find
            .as_ref()
            .is_some_and(|b| !b.matches.is_empty() && !ed.marks.has_find())
        {
            self.close_find(cx);
        }
    }

    /// Finds the query again in the active pane's document and remakes the highlight group.
    /// With `jump`, the caret goes to the first match from where typing started.
    pub(super) fn refind(&mut self, jump: bool, cx: &mut Context<Self>) {
        let first = self.refind_marks();
        self.touch_pane();
        if let Some(byte) = first.filter(|_| jump) {
            self.place_caret(byte, super::navigation::Reveal::IfHidden, cx);
        }
    }

    /// The matching half of [`Chorong::refind`]: the bar's matches and the highlight group, and
    /// where the first match from the origin starts.
    fn refind_marks(&mut self) -> Option<usize> {
        let palette = self.marker_colours.len();
        let ed = self.ed_mut();
        let (Some(bar), Some(doc)) = (&mut ed.find, &ed.doc) else {
            return None;
        };
        bar.refresh(doc.buffer.text());
        let first = bar.first_from_origin().map(|i| bar.matches[i].start);
        let (query, matches) = (bar.query.clone(), bar.matches.clone());
        ed.marks.set_find(&query, matches, palette);
        first
    }

    /// The active pane's document changed under an open bar: find again, without moving.
    pub(super) fn refind_after_load(&mut self) {
        if self.ed().find.is_some() {
            self.refind_marks();
        }
    }

    /// Enter / Shift-Enter: the next or previous match from the caret, wrapping.
    pub(super) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(bar) = &self.ed().find else { return };
        let caret = self.ed().caret.unwrap_or(bar.origin);
        let Some(i) = step_occurrence(&bar.matches, caret, forward) else {
            return;
        };
        let byte = bar.matches[i].start;
        if let Some(bar) = &mut self.ed_mut().find {
            bar.origin = byte;
            bar.query_all = false;
        }
        self.touch_pane();
        self.place_caret(byte, super::navigation::Reveal::IfHidden, cx);
    }

    /// Alt-C and Alt-R, or a click on **Aa** / **.\***.
    pub(super) fn find_toggle(&mut self, regex: bool, cx: &mut Context<Self>) {
        let Some(bar) = &mut self.ed_mut().find else {
            return;
        };
        if regex {
            bar.regex = !bar.regex;
        } else {
            bar.case_sensitive = !bar.case_sensitive;
        }
        self.refind(true, cx);
    }

    /// A key typed while the bar has the keyboard. Returns whether the bar took it.
    pub(super) fn find_key(
        &mut self,
        k: &Keystroke,
        paste: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        let caret = self.ed().caret;
        let Some(bar) = &mut self.ed_mut().find else {
            return false;
        };
        let was_empty = bar.query.is_empty();
        match edit_field(k, &mut bar.query, &mut bar.query_all, paste) {
            FieldEdit::None => return false,
            FieldEdit::Text => {
                if was_empty && let Some(c) = caret {
                    bar.origin = c;
                }
                self.refind(true, cx);
            }
            FieldEdit::Selection => self.touch_pane(),
            FieldEdit::Copy(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
        }
        true
    }

    /// The bar, at the top of code pane `pane`, while it has one. Drawn inside the pane body:
    /// it reads only `self` (never the shell).
    pub(super) fn render_find_bar(
        &self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ed = self.ed_of(pane);
        let bar = ed.find.as_ref()?;
        let t = &self.theme;
        let (muted, border, selected_bg) =
            (hsla(t.text_muted), hsla(t.border), hsla(t.element_selected));
        let focused = pane == self.active_editor && self.key_focus == KeyFocus::Find;
        let key = |name: &str| self.find.keys.get(name).cloned();
        let toggle = |id: &'static str, label: &'static str, on: bool, regex: bool, title: &str| {
            let tip_text = match key(if regex {
                "chorong::FindToggleRegex"
            } else {
                "chorong::FindToggleCase"
            }) {
                Some(k) => format!("{title} ({k})"),
                None => title.to_string(),
            };
            div()
                .id(id)
                .flex_none()
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .when(on, |d| d.bg(selected_bg).text_color(hsla(t.text)))
                .when(!on, |d| d.text_color(muted))
                .hover(|s| s.bg(selected_bg))
                .tooltip(tip(tip_text))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_editor(pane, cx);
                    this.focus_to(KeyFocus::Find);
                    this.find_toggle(regex, cx);
                    cx.notify();
                }))
        };
        let hints: Vec<String> = [
            ("chorong::FindNext", "next"),
            ("chorong::FindPrev", "previous"),
            ("chorong::FindClose", "close"),
        ]
        .iter()
        .filter_map(|(name, what)| Some(format!("{} {what}", key(name)?)))
        .collect();
        let summary = bar.summary(ed.caret);
        let error = bar.error.is_some();
        let accent = t.players.first().map_or(t.border, |p| p.cursor);
        Some(
            div()
                .id(("find-bar", pane.0 as usize))
                .flex_none()
                .h(px(32.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(border)
                .bg(hsla(t.ui.toolbar_background))
                .text_size(px(12.5))
                .overflow_hidden()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_editor(pane, cx);
                    this.focus_to(KeyFocus::Find);
                    this.touch_pane();
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(80.))
                        .max_w(px(420.))
                        .h(px(24.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .border_1()
                        .border_color(if focused { hsla(accent) } else { border })
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(bar.query.is_empty(), |d| {
                            d.child(div().text_color(muted).child("Find in file\u{2026}"))
                        })
                        .child(self.query_text(&bar.query, bar.query_all))
                        .when(focused, |d| {
                            d.child(div().w(px(1.5)).h(px(14.)).bg(hsla(t.text)))
                        }),
                )
                .child(toggle(
                    "find-case",
                    "Aa",
                    bar.case_sensitive,
                    false,
                    "Match case",
                ))
                .child(toggle(
                    "find-regex",
                    ".*",
                    bar.regex,
                    true,
                    "Regular expression",
                ))
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_size(px(11.5))
                        .when(error, |d| d.text_color(hsla(t.ui.error.unwrap_or(t.text))))
                        .when(!error, |d| d.text_color(muted))
                        .child(summary),
                )
                .when(focused && !hints.is_empty(), |d| {
                    d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(hints.join("  \u{b7}  ")),
                    )
                })
                .into_any_element(),
        )
    }
}

// Actions and registration (see `actions.rs`). `FindInFile` itself is in `navigation.rs`.

actions!(
    chorong,
    [
        FindNext,
        FindPrev,
        FindClose,
        FindToggleCase,
        FindToggleRegex,
    ]
);

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, FindNext, |this, _w, cx| this.find_step(true, cx));
    let d = on!(d, cx, FindPrev, |this, _w, cx| this.find_step(false, cx));
    let d = on!(d, cx, FindClose, |this, _w, cx| this.close_find(cx));
    let d = on!(d, cx, FindToggleCase, |this, _w, cx| this
        .find_toggle(false, cx));
    on!(d, cx, FindToggleRegex, |this, _w, cx| this
        .find_toggle(true, cx))
}

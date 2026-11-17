//! The pane pickers (Go to pane…, Move pane to…, Bring from tray…) and the Ctrl-Tab switcher:
//! one overlay in the palette's style. Every answer comes from `cho-layout`
//! (`pane_entries`, `destinations`, `tray_entries`, `Switcher`); this file lists, filters and
//! draws them.
//!
//! Keys: the overlay is focused, with key context `Chorong PanePicker` (pickers, which type a
//! query) or `Chorong PaneSwitcher`, so the keymap binds Up/Down/Enter/Esc and every other
//! binding in `Chorong` still works. The viewer's single-letter keys must not apply: a binding
//! matches at any depth of the context stack, so the host drops its `CodeView` context while
//! an overlay is open (`Shell::overlay_open`), and letters type into the query.
//!
//! The switcher commits when the modifier it was opened with is released. GPUI reports
//! modifier changes (`on_modifiers_changed`) to the focused element on every platform; a
//! release that happens before the overlay has focus is caught on the next frame, which
//! compares `Window::modifiers` with the ones held at open. A binding with no modifier
//! (a remapped `f6`) switches at once. Enter, a click and Esc work as well.

use super::actions::listener;
use super::*;
use crate::field::{FieldEdit, edit_field};
use crate::fuzzy::Ranker;
use cho_layout::{PaneEntry, Switcher};
use gpui::{KeyDownEvent, Modifiers, ModifiersChangedEvent};
use std::ops::Range;

const WIDTH: f32 = 560.;
const ROW: f32 = 26.;
const VISIBLE_ROWS: usize = 12;
/// The most rows a query can show.
const MAX_ROWS: usize = 200;
/// What Tab switches Bring from tray… to. Typing the start of it shows the rest as a hint.
const FILES_MODE: &str = "files";

/// The rest of [`FILES_MODE`] when `query` is its start (any case), for the hint after the
/// query; `Some("")` when the query is empty. `None` when Tab would do nothing.
fn files_completion(query: &str) -> Option<&'static str> {
    let q = query.trim().to_lowercase();
    FILES_MODE.strip_prefix(q.as_str())
}

/// What the open overlay is.
pub(super) enum Overlay {
    List(List),
    Switcher {
        state: Switcher,
        /// Modifiers held when it opened; releasing one of them commits.
        held: Modifiers,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ListKind {
    Focus,
    Move,
    Bring,
}

impl ListKind {
    fn title(self) -> &'static str {
        match self {
            ListKind::Focus => "Go to pane\u{2026}",
            ListKind::Move => "Move pane to\u{2026}",
            ListKind::Bring => "Add to page from the tray\u{2026}",
        }
    }
}

/// What confirming a row does.
#[derive(Clone, Copy)]
enum Op {
    Focus(PaneId),
    Move(PaneId, Target),
    Bring(PaneId),
}

/// What one drawn row shows.
struct RowView {
    icon: &'static str,
    label: SharedString,
    highlights: Vec<(Range<usize>, gpui::HighlightStyle)>,
    place: String,
    /// The focused pane: marked as such.
    current: bool,
}

struct Row {
    label: String,
    place: String,
    icon: &'static str,
    op: Op,
}

/// A row that matches the query, with the byte ranges of its label to highlight.
struct Hit {
    row: usize,
    ranges: Vec<Range<usize>>,
}

pub(super) struct List {
    kind: ListKind,
    query: String,
    /// The whole query is selected (Cmd-A): typing replaces it.
    query_all: bool,
    rows: Vec<Row>,
    hits: Vec<Hit>,
    selected: usize,
}

impl List {
    fn requery(&mut self) {
        self.hits = rank(&self.rows, &self.query);
        self.selected = 0;
    }
}

/// The rows matching `query`, best first (all rows in order when it is empty). Matches against
/// the label and the place, so `tray` or `right dock` find panes by where they are; only label
/// matches are highlighted.
fn rank(rows: &[Row], query: &str) -> Vec<Hit> {
    let texts: Vec<String> = rows
        .iter()
        .map(|r| format!("{}  {}", r.label, r.place))
        .collect();
    // Ties keep the list's own order (recency for panes).
    Ranker::labels()
        .rank(&texts, query, MAX_ROWS)
        .into_iter()
        .map(|m| {
            let label = rows[m.index].label.len();
            let ranges = m
                .ranges
                .into_iter()
                .filter(|r| r.start < label)
                .map(|r| r.start..r.end.min(label))
                .collect();
            Hit {
                row: m.index,
                ranges,
            }
        })
        .collect()
}

fn pane_row(e: &PaneEntry) -> Row {
    Row {
        label: e.title.clone(),
        place: e.place_label(),
        icon: kind_icon(e.kind),
        op: Op::Focus(e.pane),
    }
}

/// Whether any modifier in `held` is still down in `now`.
fn still_held(held: Modifiers, now: Modifiers) -> bool {
    (held.control && now.control) || (held.alt && now.alt) || (held.platform && now.platform)
}

/// Whether a chord's modifiers include one a switcher can wait on releasing.
fn holds_modifier(m: Modifiers) -> bool {
    m.control || m.alt || m.platform
}

impl Shell {
    /// Whether a picker or the switcher is open.
    pub fn overlay_open(&self) -> bool {
        self.overlay().is_some()
    }

    fn open_overlay(&mut self, overlay: Overlay, window: &mut Window, cx: &mut Context<Self>) {
        self.open_popup(Popup::Overlay(overlay), window, cx);
    }

    /// Closes the overlay without doing anything and gives the focus back to the host.
    pub(super) fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.take_overlay().is_some() {
            window.focus(&self.return_focus);
            cx.notify();
        }
    }

    /// Opens Bring from tray… (for the host's file picker to step back to). False, with
    /// nothing opened, when the tray is empty or there is no focus layout.
    pub fn open_tray_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.ws.in_focus() || self.ws.tray().is_empty() {
            return false;
        }
        self.open_list(ListKind::Bring, window, cx);
        true
    }

    /// Tab in Bring from tray…: when the query is empty or the start of "files", closes the
    /// list and asks the host for its file picker (`AddFileToPage`).
    pub(super) fn list_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to_files = matches!(self.overlay(), Some(Overlay::List(l))
            if l.kind == ListKind::Bring && files_completion(&l.query).is_some());
        if to_files {
            self.close_overlay(window, cx);
            ask_for_files(window, cx);
        }
    }

    pub(super) fn open_list(
        &mut self,
        kind: ListKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows: Vec<Row> = match kind {
            ListKind::Focus => self.ws.pane_entries().iter().map(pane_row).collect(),
            ListKind::Bring => self
                .ws
                .tray_entries()
                .iter()
                .map(|e| Row {
                    op: Op::Bring(e.pane),
                    ..pane_row(e)
                })
                .collect(),
            ListKind::Move => match self.target_pane() {
                Some(pane) => self
                    .ws
                    .destinations(pane)
                    .into_iter()
                    .map(|d| Row {
                        label: d.label,
                        place: d.place,
                        icon: "move",
                        op: Op::Move(pane, d.target),
                    })
                    .collect(),
                None => Vec::new(),
            },
        };
        // Building a page with an empty tray: straight to the host's file picker.
        if kind == ListKind::Bring && rows.is_empty() && self.ws.in_focus() {
            self.notice = None;
            ask_for_files(window, cx);
            return;
        }
        let notice = match (kind, rows.is_empty()) {
            (_, false) => None,
            // In a focus layout an empty tray went to the file picker above.
            (ListKind::Bring, true) => Some("The tray is only used in a focus layout"),
            (ListKind::Move, true) => Some("There is nowhere to move this pane"),
            (ListKind::Focus, true) => Some("There are no panes"),
        };
        if let Some(text) = notice {
            self.notice = Some(text.into());
            cx.notify();
            return;
        }
        self.notice = None;
        let mut list = List {
            kind,
            query: String::new(),
            query_all: false,
            hits: Vec::new(),
            rows,
            selected: 0,
        };
        list.requery();
        if kind == ListKind::Focus {
            // The focused pane heads the list; start on the pane before it.
            list.selected = list
                .hits
                .iter()
                .position(|h| !matches!(list.rows[h.row].op, Op::Focus(p) if self.ws.focused() == Some(p)))
                .unwrap_or(0);
        }
        self.open_overlay(Overlay::List(list), window, cx);
    }

    /// Ctrl-Tab: opens the switcher on the previous pane; with it open, steps. `backwards` is
    /// Ctrl-Shift-Tab.
    pub(super) fn switcher_key(
        &mut self,
        backwards: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.overlay_mut() {
            Some(Overlay::Switcher { state, .. }) => {
                state.step(backwards);
                cx.notify();
                return;
            }
            // A picker is open: Ctrl-Tab does nothing until it closes.
            Some(Overlay::List(_)) => return,
            None => {}
        }
        let Some(state) = Switcher::open(&self.ws, backwards) else {
            self.notice = Some("There is no other pane to switch to".into());
            cx.notify();
            return;
        };
        let held = window.modifiers();
        if !holds_modifier(held) {
            // Bound to a key with no modifier to release: switch at once.
            let pane = state.selected();
            self.apply(cx, |ws| ws.reveal(pane));
            return;
        }
        self.notice = None;
        self.open_overlay(Overlay::Switcher { state, held }, window, cx);
    }

    /// Switches to the switcher's selected pane and closes it.
    fn commit_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Switcher { state, .. }) = self.take_overlay() else {
            return;
        };
        window.focus(&self.return_focus);
        let pane = state.selected();
        self.apply(cx, |ws| ws.reveal(pane));
    }

    pub(super) fn list_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        match self.overlay_mut() {
            Some(Overlay::List(l)) if !l.hits.is_empty() => {
                let last = l.hits.len() - 1;
                l.selected = l.selected.saturating_add_signed(delta).min(last);
            }
            Some(Overlay::Switcher { state, .. }) => state.step(delta < 0),
            _ => return,
        }
        cx.notify();
    }

    /// Enter (`alt` false) or Shift-Enter (`alt` true).
    pub(super) fn list_confirm(&mut self, alt: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.overlay() {
            Some(Overlay::Switcher { .. }) => self.commit_switcher(window, cx),
            Some(Overlay::List(l)) => {
                let selected = l.selected;
                self.confirm_row(selected, alt, window, cx);
            }
            None => {}
        }
    }

    fn confirm_row(&mut self, hit: usize, alt: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::List(l)) = self.take_overlay() else {
            return;
        };
        window.focus(&self.return_focus);
        let Some(row) = l.hits.get(hit).map(|h| &l.rows[h.row]) else {
            cx.notify();
            return;
        };
        match row.op {
            Op::Focus(pane) => self.apply(cx, |ws| ws.reveal(pane)),
            Op::Move(pane, target) => self.apply(cx, |ws| ws.move_pane(pane, target)),
            Op::Bring(pane) => {
                let edge = if alt { Edge::Bottom } else { Edge::Right };
                self.apply(cx, |ws| ws.bring_from_tray(pane, edge));
                // Building a page: the list comes back for the next pane until the tray is
                // empty or Escape ends it.
                if !self.ws.tray().is_empty() {
                    self.open_list(ListKind::Bring, window, cx);
                }
            }
        }
    }

    // ----- drawing ---------------------------------------------------------------------------

    /// The overlay, when one is open.
    pub(super) fn render_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let overlay = self.overlay()?;
        let c = &self.c;
        let (context, title, body, footer) = match overlay {
            Overlay::List(l) => (
                "Chorong PanePicker",
                l.kind.title(),
                self.list_body(l, cx),
                self.list_footer(l, window),
            ),
            Overlay::Switcher { state, .. } => (
                "Chorong PaneSwitcher",
                "Recent panes",
                self.switcher_body(state, cx),
                self.switcher_footer(window),
            ),
        };
        let query = match overlay {
            Overlay::List(l) => Some((l.query.clone(), l.query_all)),
            Overlay::Switcher { .. } => None,
        };
        // Bring from tray…: typing the start of "files" shows the rest, and Tab takes it.
        let ghost = match overlay {
            Overlay::List(l) if l.kind == ListKind::Bring && !l.query.trim().is_empty() => {
                files_completion(&l.query).map(|rest| format!("{rest}  \u{21e5} files"))
            }
            _ => None,
        };
        Some(
            deferred(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(70.))
                    .child(
                        div()
                            .id("pane-overlay")
                            .key_context(context)
                            .track_focus(&self.overlay_focus)
                            .occlude()
                            .w(px(WIDTH))
                            .flex()
                            .flex_col()
                            .bg(c.panel)
                            .border_1()
                            .border_color(c.border)
                            .rounded_md()
                            .shadow_lg()
                            .text_size(px(13.))
                            .text_color(c.text)
                            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                                let k = &e.keystroke;
                                let paste = (k.key == "v" && k.modifiers.secondary())
                                    .then(|| cx.read_from_clipboard().and_then(|c| c.text()))
                                    .flatten();
                                let Some(Overlay::List(l)) = this.overlay_mut() else {
                                    return;
                                };
                                match edit_field(
                                    k,
                                    &mut l.query,
                                    &mut l.query_all,
                                    paste.as_deref(),
                                ) {
                                    FieldEdit::None => return,
                                    FieldEdit::Text => l.requery(),
                                    FieldEdit::Selection => {}
                                    FieldEdit::Copy(text) => {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
                                    }
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }))
                            .on_modifiers_changed(cx.listener(
                                |this, e: &ModifiersChangedEvent, window, cx| {
                                    if let Some(Overlay::Switcher { held, .. }) = this.overlay()
                                        && !still_held(*held, e.modifiers)
                                    {
                                        this.commit_switcher(window, cx);
                                    }
                                },
                            ))
                            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                                this.close_overlay(window, cx);
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .h(px(36.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .border_b_1()
                                    .border_color(c.border)
                                    .when_some(query, |d, (q, all)| {
                                        d.when(q.is_empty(), |d| {
                                            d.child(div().text_color(c.text_muted).child(title))
                                        })
                                        // The caret sits against the query, as in the file picker
                                        // (the row's gap is for the title and the completion).
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                // Selected by Cmd-A: drawn on the selection
                                                // colour.
                                                .child(
                                                    div().when(all, |d| d.bg(c.selected)).child(q),
                                                )
                                                .child(div().w(px(1.5)).h(px(16.)).bg(c.text)),
                                        )
                                    })
                                    .when_some(ghost, |d, g| {
                                        d.child(div().ml(px(-8.)).text_color(c.text_muted).child(g))
                                    })
                                    .when(matches!(overlay, Overlay::Switcher { .. }), |d| {
                                        d.child(div().text_color(c.text_muted).child(title))
                                    }),
                            )
                            .child(body)
                            .child(footer),
                    ),
            )
            .into_any_element(),
        )
    }

    fn row_element(
        &self,
        ix: usize,
        selected: bool,
        view: RowView,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &self.c;
        let RowView {
            icon,
            label,
            highlights,
            place,
            current,
        } = view;
        div()
            .id(("pane-row", ix))
            .w_full()
            .h(px(ROW))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |d| d.bg(c.selected))
            .hover(|d| d.bg(c.selected))
            .child(small_icon(icon, c.icon_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(gpui::StyledText::new(label).with_highlights(highlights)),
            )
            .when(current, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(c.text_muted)
                        .child("focused"),
                )
            })
            .when(!place.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .pl_3()
                        .text_size(px(12.))
                        .text_color(c.text_muted)
                        .child(place),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .into_any_element()
    }

    /// The visible window of `len` rows around `selected`.
    fn window_of(selected: usize, len: usize) -> Range<usize> {
        let start = (selected + 1).saturating_sub(VISIBLE_ROWS);
        start..len.min(start + VISIBLE_ROWS)
    }

    fn list_body(&self, l: &List, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        if l.hits.is_empty() {
            return div()
                .p_3()
                .text_color(c.text_muted)
                .child("No matches")
                .into_any_element();
        }
        let accent = gpui::HighlightStyle {
            color: Some(c.text_accent),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let range = Self::window_of(l.selected, l.hits.len());
        let rows: Vec<AnyElement> = range
            .map(|ix| {
                let hit = &l.hits[ix];
                let row = &l.rows[hit.row];
                let highlights = hit.ranges.iter().map(|r| (r.clone(), accent)).collect();
                let current = matches!(row.op, Op::Focus(p) if self.ws.focused() == Some(p));
                self.row_element(
                    ix,
                    ix == l.selected,
                    RowView {
                        icon: row.icon,
                        label: SharedString::from(row.label.clone()),
                        highlights,
                        place: row.place.clone(),
                        current,
                    },
                    move |this, window, cx| this.confirm_row(ix, false, window, cx),
                    cx,
                )
            })
            .collect();
        div().flex().flex_col().children(rows).into_any_element()
    }

    fn switcher_body(&self, s: &Switcher, cx: &mut Context<Self>) -> AnyElement {
        let range = Self::window_of(s.index(), s.order().len());
        let rows: Vec<AnyElement> = range
            .filter_map(|ix| {
                let entry = self.ws.pane_entry(s.order()[ix])?;
                Some(self.row_element(
                    ix,
                    ix == s.index(),
                    RowView {
                        icon: kind_icon(entry.kind),
                        label: SharedString::from(entry.title.clone()),
                        highlights: Vec::new(),
                        place: entry.place_label(),
                        current: entry.focused,
                    },
                    move |this, window, cx| {
                        if let Some(Overlay::Switcher { state, .. }) = this.overlay_mut() {
                            state.select(ix);
                        }
                        this.commit_switcher(window, cx);
                    },
                    cx,
                ))
            })
            .collect();
        div().flex().flex_col().children(rows).into_any_element()
    }

    fn footer(&self, items: Vec<(Option<String>, &str)>) -> AnyElement {
        crate::widgets::hint_footer(
            &self.c,
            items.into_iter().map(|(k, w)| (k, w.to_string())).collect(),
        )
    }

    fn list_footer(&self, l: &List, window: &Window) -> AnyElement {
        let key = |a: &dyn Action| keys::label_in(a, window, "Chorong PanePicker");
        let mut items = vec![(
            key(&PaneListConfirm),
            match l.kind {
                ListKind::Focus => "go to",
                ListKind::Move => "move",
                ListKind::Bring => "bring in to the right",
            },
        )];
        if l.kind == ListKind::Bring {
            items.push((key(&PaneListConfirmAlt), "below"));
            items.push((key(&PaneListTab), "files"));
        }
        let close = if l.kind == ListKind::Bring {
            "done"
        } else {
            "close"
        };
        items.push((key(&PaneListClose), close));
        self.footer(items)
    }

    fn switcher_footer(&self, window: &Window) -> AnyElement {
        let key = |a: &dyn Action| keys::label_in(a, window, "Chorong PaneSwitcher");
        self.footer(vec![
            (key(&OpenSwitcher), "next"),
            (key(&OpenSwitcherReverse), "previous"),
            (None, "release to switch"),
            (key(&PaneListClose), "cancel"),
        ])
    }

    /// Run on each frame: a switcher whose modifier was released before it had the focus (a
    /// quick tap), or without any reported release, commits now.
    pub(super) fn settle_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Switcher { held, .. }) = self.overlay()
            && !still_held(*held, window.modifiers())
        {
            cx.defer_in(window, |this, window, cx| this.commit_switcher(window, cx));
        }
    }
}

/// The overlay's actions, attached next to the shell's others.
pub(super) fn register<E: InteractiveElement>(d: E, shell: &Entity<Shell>) -> E {
    d.on_action(listener(shell, |this, _: &FocusPane, window, cx| {
        this.open_list(ListKind::Focus, window, cx)
    }))
    .on_action(listener(shell, |this, _: &MovePaneTo, window, cx| {
        this.open_list(ListKind::Move, window, cx)
    }))
    .on_action(listener(shell, |this, _: &BringFromTray, window, cx| {
        this.open_list(ListKind::Bring, window, cx)
    }))
    .on_action(listener(shell, |this, _: &OpenSwitcher, window, cx| {
        this.switcher_key(false, window, cx)
    }))
    .on_action(listener(
        shell,
        |this, _: &OpenSwitcherReverse, window, cx| this.switcher_key(true, window, cx),
    ))
    .on_action(listener(shell, |this, _: &PaneListPrev, _, cx| {
        this.list_move(-1, cx)
    }))
    .on_action(listener(shell, |this, _: &PaneListNext, _, cx| {
        this.list_move(1, cx)
    }))
    .on_action(listener(shell, |this, _: &PaneListConfirm, window, cx| {
        this.list_confirm(false, window, cx)
    }))
    .on_action(listener(
        shell,
        |this, _: &PaneListConfirmAlt, window, cx| this.list_confirm(true, window, cx),
    ))
    .on_action(listener(shell, |this, _: &PaneListTab, window, cx| {
        this.list_tab(window, cx)
    }))
    .on_action(listener(shell, |this, _: &PaneListClose, window, cx| {
        this.close_overlay(window, cx)
    }))
}

/// Asks the host for its add-to-page file picker, once focus is back in the host's view.
pub(super) fn ask_for_files(window: &mut Window, cx: &mut Context<Shell>) {
    cx.defer_in(window, |_, window, cx| {
        window.dispatch_action(Box::new(AddFileToPage), cx);
    });
}

#[cfg(test)]
mod files_mode_tests {
    use super::files_completion;

    #[test]
    fn the_start_of_files_completes_and_anything_else_does_not() {
        assert_eq!(files_completion(""), Some("files"));
        assert_eq!(files_completion("f"), Some("iles"));
        assert_eq!(files_completion("FIL"), Some("es"));
        assert_eq!(files_completion("files"), Some(""));
        assert_eq!(files_completion("fo"), None);
        assert_eq!(files_completion("outline"), None);
    }
}

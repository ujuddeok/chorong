//! Pinned context panes (design/layout.md, "Pinning context").
//!
//! A pin freezes what the Context pane shows into a pane of its own: the symbol, where it is
//! declared and its references, as the index answers for the pin's file and byte offset. Each
//! pin pane has a [`PinView`], gathered off the UI thread when the pane first draws and again
//! when the index changes. The pin records a fingerprint of its file (`Pin::content_hash`), so
//! a pin whose file changed underneath it says so rather than silently showing other code.
//! Clicking a definition or reference opens it in the active editor and records history.
//!
//! Cmd-K P (`TogglePin`) pins here, under the live Context pane, or unpins when a pin pane has
//! focus; Cmd-K Shift-P pins to the bottom bar. The layout shell owns the panes; this module
//! only draws their bodies.

use super::*;
use cho_layout::{PaneContent, PaneKind, Pin, PinPlace, Region, Workspace};
use std::path::Path;

/// Whether a pin's file still has the text the pin was taken on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Staleness {
    Unchanged,
    /// The file's text differs from when the pin was taken.
    Changed,
    /// The file is gone from the checkout.
    Removed,
    /// No fingerprint and no copy of the file at the pin's commit to compare with.
    Unknown,
}

/// Compares the pin's file now (`now`, `None` when it cannot be read) with what the pin
/// recorded: its fingerprint, or for a pin without one, the file at its commit (`at_commit`,
/// called only then).
pub(super) fn staleness(
    pin: &Pin,
    now: Option<&str>,
    at_commit: impl FnOnce() -> Option<String>,
) -> Staleness {
    let Some(now) = now else {
        return Staleness::Removed;
    };
    let same = match &pin.content_hash {
        Some(hash) => *hash == cho_layout::content_hash(now.as_bytes()),
        None => match at_commit() {
            Some(then) => then == now,
            None => return Staleness::Unknown,
        },
    };
    if same {
        Staleness::Unchanged
    } else {
        Staleness::Changed
    }
}

/// A file's text as the viewer reads it (the same decoding as an open document), so a
/// fingerprint taken from a document matches one taken from the file.
fn read_text(root: &Path, path: &Path) -> Option<String> {
    let bytes = cho_core::fs::read_source_in(root, path).ok()?;
    cho_core::SourceBuffer::from_bytes(bytes)
        .ok()
        .map(|b| b.text().to_string())
}

/// The one-based line holding `byte` and that line's text, trimmed.
pub(super) fn line_at(text: &str, byte: usize) -> Option<(usize, String)> {
    if byte > text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    let start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |i| byte + i);
    let line = text[..byte].matches('\n').count() + 1;
    Some((line, text[start..end].trim().to_string()))
}

/// What a pin pane shows, gathered off the UI thread.
pub(super) struct PinView {
    /// The pin this was gathered for.
    pin: Pin,
    /// The index load it was gathered with (`Chorong::index_id`), so a new index regathers.
    index: Option<Generation>,
    /// Which gather this view waits for or shows (`Chorong::pin_gathers`).
    generation: Generation,
    lookup: Lookup,
    /// The line at the pin's offset, in the file as it is now.
    line: Option<(usize, String)>,
    stale: Staleness,
    /// Each column's scroll and rubber-band overscroll (the list, or head and references
    /// side by side).
    scroll: [(gpui::ScrollHandle, crate::overscroll::Overscroll); 2],
}

struct Gathered {
    answer: Option<Answer>,
    line: Option<(usize, String)>,
    stale: Staleness,
}

impl PinView {
    /// A view of `pin` waiting for gather `generation`.
    fn new(pin: Pin, index: Option<Generation>, generation: Generation) -> Self {
        PinView {
            pin,
            index,
            generation,
            lookup: Lookup::Loading,
            line: None,
            stale: Staleness::Unknown,
            scroll: Default::default(),
        }
    }

    /// Whether this view is (being) gathered for `pin` with the index load `index`.
    fn shows(&self, pin: &Pin, index: Option<Generation>) -> bool {
        self.pin == *pin && self.index == index
    }

    /// Applies gather `generation` if it is the one this view waits for; a slower gather for
    /// an earlier pin or index is dropped. True when applied.
    fn land(&mut self, generation: Generation, got: Gathered) -> bool {
        if !self.generation.is(generation) {
            return false;
        }
        self.lookup = got.answer.map_or(Lookup::Idle, Lookup::Done);
        self.line = got.line;
        self.stale = got.stale;
        true
    }
}

fn gather(
    sem: Option<Arc<Semantic>>,
    root: &Path,
    pin: &Pin,
    references: Option<&crate::features::Probe>,
) -> Gathered {
    let now = read_text(root, Path::new(&pin.path));
    let stale = staleness(pin, now.as_deref(), || {
        cho_git::read_file_at(root, &pin.commit, &pin.path)
            .ok()
            .and_then(|b| cho_core::SourceBuffer::from_bytes(b).ok())
            .map(|b| b.text().to_string())
    });
    let line = now.as_deref().and_then(|t| line_at(t, pin.byte_offset));
    let answer = sem
        .filter(|_| stale != Staleness::Removed)
        .map(|sem| context::gather(&sem, root, &pin.path, pin.byte_offset, references));
    Gathered {
        answer,
        line,
        stale,
    }
}

/// Whether `pane` shows wide: in the bottom bar, or on a focus page (the design's "wide
/// layout": declaration and definition on the left, references in columns by file).
fn is_wide(ws: &Workspace, pane: PaneId) -> bool {
    if ws.focus().is_some_and(|f| f.on_screen().contains(&pane)) {
        return true;
    }
    matches!(
        ws.arrangement().locate(pane),
        Some((Region::Dock(cho_layout::DockId::Bottom), _))
    )
}

impl Chorong {
    /// The pin under the caret or pointer, made into a pane: `Here` under the live Context
    /// pane, or as a tab in the bottom bar.
    pub(super) fn pin_context(&mut self, place: PinPlace, cx: &mut Context<Self>) {
        let Some(pin) = self.layout_pin() else {
            self.notify_text("Nothing to pin: click or hover a symbol first", cx);
            return;
        };
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| ws.pin_context(pin, place));
        });
    }

    /// Cmd-K P: unpins the focused pin pane, else pins the context here.
    pub(super) fn toggle_pin(&mut self, cx: &mut Context<Self>) {
        let focused_pin = {
            let ws = self.shell.read(cx).workspace();
            ws.focused()
                .filter(|p| ws.kind(*p) == Some(PaneKind::PinnedContext))
        };
        match focused_pin {
            Some(pane) => self.shell.update(cx, |shell, cx| {
                let _ = shell.update_workspace(cx, |ws| ws.close(pane));
            }),
            None => self.pin_context(PinPlace::Here, cx),
        }
    }

    /// Drops the views of pin panes that closed. Runs on every layout change.
    pub(super) fn sync_pins(&mut self, ws: &Workspace) {
        self.pins
            .retain(|pane, _| matches!(ws.content(*pane), Some(PaneContent::PinnedContext(_))));
    }

    /// Starts gathering `pane`'s view if it has none, its pin changed, or the index changed.
    fn ensure_pin(&mut self, pane: PaneId, pin: &Pin, cx: &mut Context<Self>) {
        let sem = match &self.index {
            IndexState::Ready(sem, _) => Some(sem.clone()),
            _ => None,
        };
        let index = sem.as_ref().map(|_| self.index_id);
        if self.pins.get(&pane).is_some_and(|v| v.shows(pin, index)) {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let generation = self.pin_gathers.next();
        self.pins
            .insert(pane, PinView::new(pin.clone(), index, generation));
        let job_pin = pin.clone();
        // References follow the same feature setting as the Context pane's.
        let references = self
            .features
            .on(crate::features::Feature::References)
            .then(|| self.features.probe(crate::features::Feature::References));
        cx.spawn(async move |this, cx| {
            let got = cx
                .background_spawn(async move { gather(sem, &root, &job_pin, references.as_ref()) })
                .await;
            this.update(cx, |this, cx| {
                if !this
                    .pins
                    .get_mut(&pane)
                    .is_some_and(|v| v.land(generation, got))
                {
                    return;
                }
                // The body, and the header (its qualifier says whether the file changed).
                this.shell.update(cx, |shell, cx| {
                    shell.refresh_pane(pane, cx);
                    cx.notify();
                });
            })
            .ok();
        })
        .detach();
    }

    /// The header's qualifier for a pin pane: its kind, and a warning when the file changed.
    pub(super) fn pin_qualifier(&self, pane: PaneId, pin: &Pin) -> String {
        match self.pins.get(&pane).map(|v| v.stale) {
            Some(Staleness::Changed) => {
                format!("{} \u{b7} changed since {}", pin.symbol_kind, pin.commit)
            }
            Some(Staleness::Removed) => format!("{} \u{b7} file removed", pin.symbol_kind),
            _ => pin.symbol_kind.clone(),
        }
    }

    /// Whether the pin's file changed or went away since it was taken.
    pub(super) fn pin_is_stale(&self, pane: PaneId) -> bool {
        self.pins
            .get(&pane)
            .is_some_and(|v| matches!(v.stale, Staleness::Changed | Staleness::Removed))
    }

    /// A pin pane's body.
    pub(super) fn render_pin(
        &mut self,
        pane: PaneId,
        pin: &Pin,
        ws: &Workspace,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.ensure_pin(pane, pin, cx);
        let t = &self.theme;
        let (muted, border) = (hsla(t.text_muted), hsla(t.border));
        let warning = hsla(t.ui.warning.unwrap_or(t.ui.text_accent));
        let heading = |label: String| {
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_size(px(11.))
                .text_color(muted)
                .child(label)
                .into_any_element()
        };
        let note = |text: String| {
            div()
                .px_3()
                .py_1()
                .text_color(muted)
                .child(text)
                .into_any_element()
        };
        let Some(view) = self.pins.get(&pane) else {
            return note("Open a folder to show pins.".into());
        };

        // Left: what was pinned, where, and the definitions.
        let mut head: Vec<gpui::AnyElement> = Vec::new();
        head.push(
            div()
                .px_3()
                .pt_2()
                .flex()
                .gap_2()
                .whitespace_nowrap()
                .child(
                    div()
                        .flex_none()
                        .text_color(muted)
                        .child(pin.symbol_kind.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .font_weight(FontWeight::MEDIUM)
                        .child(pin.symbol.clone()),
                )
                .into_any_element(),
        );
        let place = match &view.line {
            Some((line, _)) => format!("{}:{} \u{b7} pinned at {}", pin.path, line, pin.commit),
            None => format!("{} \u{b7} pinned at {}", pin.path, pin.commit),
        };
        head.push(
            div()
                .px_3()
                .text_size(px(11.5))
                .text_color(muted)
                .whitespace_nowrap()
                .overflow_hidden()
                .child(place)
                .into_any_element(),
        );
        match view.stale {
            Staleness::Changed => head.push(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.5))
                    .text_color(warning)
                    .child(format!(
                        "The file changed since this pin was taken at {}: lines may have moved.",
                        pin.commit
                    ))
                    .into_any_element(),
            ),
            Staleness::Removed => head.push(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.5))
                    .text_color(warning)
                    .child(format!(
                        "{} is no longer in the checkout. The pin was taken at {}.",
                        pin.path, pin.commit
                    ))
                    .into_any_element(),
            ),
            _ => {}
        }
        if let Some((_, text)) = &view.line {
            head.push(
                div()
                    .mx_3()
                    .mt_1()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(hsla(t.editor_background))
                    .font_family(self.font.clone())
                    .text_size(px(11.5))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(text.clone())
                    .into_any_element(),
            );
        }

        let mut refs: Vec<gpui::AnyElement> = Vec::new();
        let wide = is_wide(ws, pane);
        let c = match &view.lookup {
            Lookup::Loading => {
                head.push(note("Looking up\u{2026}".into()));
                None
            }
            Lookup::Done(Answer::Symbol(c)) => Some(c.clone()),
            Lookup::Done(Answer::NoSymbol) => {
                head.push(note("The index has no symbol here.".into()));
                None
            }
            Lookup::Idle => {
                if view.stale != Staleness::Removed {
                    head.push(note(
                        "Definition and references need an index (Build index in the Context pane)."
                            .into(),
                    ));
                }
                None
            }
        };
        if let Some(c) = c {
            let mark_col = crate::pane_style::colors(&self.theme, cx).kind(&c.kind.to_lowercase());
            let mut id = 0;
            head.push(heading("Definition".into()));
            if c.definitions.is_empty() {
                head.push(note("Defined outside the indexed project.".into()));
            }
            for place in &c.definitions {
                head.push(
                    self.place_row(id, place, true, false, mark_col, cx)
                        .into_any_element(),
                );
                id += 1;
            }
            let title = format!("References ({})", c.reference_count);
            if wide {
                refs.push(heading(title));
            } else {
                head.push(heading(title));
            }
            let target = if wide { &mut refs } else { &mut head };
            if c.reference_count == 0 {
                target.push(note("No references in the index.".into()));
            }
            let mut columns: Vec<gpui::AnyElement> = Vec::new();
            for file in &c.references {
                let mut rows: Vec<gpui::AnyElement> = vec![
                    div()
                        .px_3()
                        .pt_1()
                        .text_size(px(11.5))
                        .border_t_1()
                        .border_color(border)
                        .child(format!("{} ({})", file.path, file.places.len()))
                        .into_any_element(),
                ];
                for place in &file.places {
                    rows.push(
                        self.place_row(id, place, false, false, mark_col, cx)
                            .into_any_element(),
                    );
                    id += 1;
                }
                if wide {
                    columns.push(
                        div()
                            .w(px(300.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .children(rows)
                            .into_any_element(),
                    );
                } else {
                    target.extend(rows);
                }
            }
            if wide {
                target.push(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(columns)
                        .into_any_element(),
                );
            }
            if c.omitted > 0 {
                target.push(note(format!("\u{2026} and {} more not listed.", c.omitted)));
            }
        }

        let probe = self
            .features
            .probe_if_on(crate::features::Feature::Overscroll);
        let scroll = self.pins.get(&pane).map(|v| v.scroll.clone());
        let areas = &self.scroll_areas;
        let column = |id: &'static str, slot: usize, rows: Vec<gpui::AnyElement>| {
            let list = div()
                .flex()
                .flex_col()
                .pb_2()
                // Rows keep their height and the column scrolls, rather than rows shrinking.
                .child(div().flex_none().flex().flex_col().children(rows));
            let area = match &scroll {
                Some(s) => {
                    let (handle, over) = &s[slot];
                    crate::scroll::ScrollArea::with(handle.clone(), over.clone())
                }
                None => areas.get(format!("{id}-{}", pane.0)),
            };
            area.vertical(id, probe.clone(), list).min_h_0().h_full()
        };
        let body = div()
            .id(("pin-body", pane.0))
            .size_full()
            .flex()
            .text_size(px(12.5));
        if wide {
            body.child(
                column("pin-left", 0, head)
                    .w(gpui::relative(0.38))
                    .flex_none()
                    .border_r_1()
                    .border_color(border),
            )
            .child(column("pin-refs", 1, refs).flex_1().min_w_0())
            .into_any_element()
        } else {
            body.child(column("pin-list", 0, head).w_full())
                .into_any_element()
        }
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(chorong, [TogglePin]);

pub(super) const COMMANDS: &[Command] = &[
    c(
        "chorong::TogglePin",
        "Pin Context Here, or Unpin",
        Layer::View,
        166,
    ),
    // Handled by the layout shell, which asks `layout_pin` what to pin.
    c(
        "chorong::PinContextToBottomBar",
        "Pin Context to Bottom Bar",
        Layer::View,
        167,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    on!(d, cx, TogglePin, |this, _w, cx| this.toggle_pin(cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slower gather for the pin a pane showed before must not land on the pin it shows now.
    #[test]
    fn a_gather_lands_only_on_the_view_it_was_started_for() {
        let gathered = || Gathered {
            answer: Some(Answer::NoSymbol),
            line: None,
            stale: Staleness::Unchanged,
        };
        let mut other = pin(None);
        other.byte_offset = 40;
        // The pane's pin changed while the index stayed: gather 2 replaced gather 1's view.
        let (mut loads, mut gathers) = (Generation::default(), Generation::default());
        let (index7, index8) = (loads.next(), loads.next());
        let (gather1, gather2) = (gathers.next(), gathers.next());
        let mut view = PinView::new(other.clone(), Some(index7), gather2);
        assert!(view.shows(&other, Some(index7)));
        assert!(!view.shows(&pin(None), Some(index7)));
        assert!(
            !view.shows(&other, Some(index8)),
            "a new index load regathers"
        );
        assert!(!view.land(gather1, gathered()));
        assert!(matches!(view.lookup, Lookup::Loading));
        assert!(view.land(gather2, gathered()));
        assert!(matches!(view.lookup, Lookup::Done(Answer::NoSymbol)));
    }

    fn pin(hash: Option<&str>) -> Pin {
        Pin {
            symbol: "LineCol".into(),
            symbol_kind: "struct".into(),
            path: "src/lib.rs".into(),
            byte_offset: 3,
            commit: "04e63f8".into(),
            content_hash: hash.map(str::to_string),
        }
    }

    #[test]
    fn a_pin_knows_when_its_file_changed() {
        let text = "fn a() {}\n";
        let hash = cho_layout::content_hash(text.as_bytes());
        let p = pin(Some(&hash));
        let never = || -> Option<String> { panic!("the fingerprint decides") };
        assert_eq!(staleness(&p, Some(text), never), Staleness::Unchanged);
        assert_eq!(
            staleness(&p, Some("fn a() { }\n"), never),
            Staleness::Changed
        );
        assert_eq!(staleness(&p, None, never), Staleness::Removed);
        // A pin saved without a fingerprint compares with the file at its commit.
        let old = pin(None);
        assert_eq!(
            staleness(&old, Some(text), || Some(text.into())),
            Staleness::Unchanged
        );
        assert_eq!(
            staleness(&old, Some(text), || Some("x".into())),
            Staleness::Changed
        );
        assert_eq!(staleness(&old, Some(text), || None), Staleness::Unknown);
    }

    #[test]
    fn line_at_finds_the_pinned_line() {
        let text = "a\n  let x = 1;\nb";
        assert_eq!(line_at(text, 6), Some((2, "let x = 1;".into())));
        assert_eq!(line_at(text, 0), Some((1, "a".into())));
        assert_eq!(line_at(text, text.len()), Some((3, "b".into())));
        assert_eq!(line_at(text, 99), None);
        assert_eq!(line_at("é", 1), None);
    }
}

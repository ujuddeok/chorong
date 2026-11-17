//! Everything that belongs to one Editor pane: its document, folds, caret, highlight groups,
//! banner, link, back/forward history and scroll position. `Chorong` keeps one per Editor pane of
//! the layout (`Chorong::editors`) and reconciles them with the layout whenever it changes.
//!
//! The pure part (which panes gain or lose a state, which one is active) is `plan` and
//! `choose_active`, tested without GPUI.

use super::compare::{Decl, Diff, Prepared};
use super::*;
use crate::history::History;
use std::collections::{BTreeMap, BTreeSet};

/// Whether row `row` (each `item` tall) lies wholly inside a viewport `viewport` tall that is
/// scrolled `top` down the list.
pub(crate) fn row_in_view(row: usize, top: f32, viewport: f32, item: f32) -> bool {
    if item <= 0.0 || viewport <= 0.0 {
        return false;
    }
    let (start, end) = (row as f32 * item, (row + 1) as f32 * item);
    start >= top - 0.5 && end <= top + viewport + 0.5
}

/// Where a code pane's list is scrolled, as last laid out: `top` pixels down, rows `row_h`
/// tall, `height` pixels visible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Viewport {
    pub top: f32,
    pub row_h: f32,
    pub height: f32,
}

/// Where the view sat in its document, so it can be put back after the document is swapped
/// (a Before/After flip).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewAnchor {
    pub viewport: Viewport,
    pub at: Anchor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Anchor {
    /// The caret's row was in view, this far below the viewport's top.
    Caret { screen_y: f32 },
    /// The caret was scrolled out of view: `line` was the top line, scrolled `offset` pixels
    /// into its row.
    Top { line: usize, offset: f32 },
}

/// The scroll offset that puts `row` (of `rows`, each `row_h` tall) `screen_y` pixels below the
/// top of a viewport `height` tall, clamped to the list: near either end the row lands as close
/// to that place as the list allows, still in view. None without a laid-out viewport.
pub(crate) fn anchored_offset(
    row: usize,
    screen_y: f32,
    rows: usize,
    vp: &Viewport,
) -> Option<f32> {
    if vp.row_h <= 0.0 || vp.height <= 0.0 {
        return None;
    }
    let max = (rows as f32 * vp.row_h - vp.height).max(0.0);
    Some((row as f32 * vp.row_h - screen_y).clamp(0.0, max))
}

/// Where a code pane's late JSON folds are (`EditorState::late_folds`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LateFolds {
    /// Nothing to wait for.
    #[default]
    None,
    /// The document wants them; the job starts at the next render.
    Wanted,
    /// Being found in the background.
    Running,
}

#[derive(Default)]
pub(crate) struct EditorState {
    /// Shared by `Arc` so a split starts from the same parsed document without reloading.
    pub doc: Option<Arc<Document>>,
    pub folds: Option<Folds>,
    /// The caret, a UTF-8 byte offset into the file. The selected line follows it.
    pub caret: Option<usize>,
    pub marks: Marks,
    /// The find bar (Cmd-F), while it is open on this pane.
    pub find: Option<crate::find::FindBar>,
    /// A large JSON file's folds, found after it showed (`Document::folds_later`).
    pub late_folds: LateFolds,
    /// Fold All (`true`) or Unfold All (`false`) pressed while `late_folds` was pending: runs
    /// when they arrive.
    pub fold_all_later: Option<bool>,
    pub banner: Option<Banner>,
    /// The `chorong://` link this pane last followed, and what git says about its commit.
    pub link: Option<LinkInfo>,
    pub link_facts: Option<mismatch::LinkFacts>,
    /// Whether the link's symbol was found in the file (`None` until it was looked up).
    pub link_found: Option<bool>,
    /// The document is at another commit and the cached index was built at exactly it.
    pub rev_index_ok: bool,
    /// Which version of the file this pane shows while the window is comparing.
    pub side: Side,
    /// The gutter marks for `side`, computed once when the document loaded.
    pub gutter: Option<Arc<Gutter>>,
    /// The other side of this file, built in the background while comparing.
    pub prepared: Option<Prepared>,
    /// The declarations of `doc`, for keeping the place across a switch.
    pub decls: Option<Arc<Vec<Decl>>>,
    /// The hunks between the two sides of this file, for mapping lines.
    pub diff: Option<Arc<Diff>>,
    /// The innermost declaration (index into `decls`) at the caret when a linked split last
    /// looked (`None` = never looked).
    pub last_decl: Option<Option<usize>>,
    /// Where the Changes list asked this pane to land once it shows the other side.
    pub pending_place: Option<super::changes::ChangePlace>,
    /// A linked column followed its partner here: this line is highlighted briefly (from this
    /// instant), `compare.follow_highlight`.
    pub flash: Option<(usize, std::time::Instant)>,
    /// In a linked pair that opened a new file, this column holds the place: the partner
    /// follows it once both have loaded.
    pub lead_pair: bool,
    /// The side `b` asked for while this pane was off screen (in the tray, a hidden tab): it
    /// flips when it is shown again.
    pub wanted_side: Option<Side>,
    /// Kind of each call in `doc.analysis.calls`, filled in the background once an index
    /// answers; empty until then (every call reads as unknown).
    pub call_kinds: Vec<CallKind>,
    /// The blame column's answer for `doc` (Toggle Blame), tagged with the `epoch` it is for.
    pub blame: Option<super::blame::PaneBlame>,
    /// Toggle Blame is on in this pane (each code pane has its own).
    pub blame_on: bool,
    /// The file being loaded in the background, if any.
    pub loading: Option<PathBuf>,
    /// Bumped whenever a different document is installed, to drop stale async results.
    pub epoch: u64,
    /// Bumped by every open, to drop the result of an older one.
    pub generation: Generation,
    pub history: History,
    pub error: Option<String>,
    /// Where the last jump landed (the identifier there, or the line), with the caret it
    /// left: outlined until the caret moves. A tour step's jump sets none.
    pub landed: Option<(usize, std::ops::Range<usize>)>,
    /// Where a tour step put the caret: its line is not painted as selected (the step's tint
    /// shows instead) until the caret moves.
    pub step_caret: Option<usize>,
    pub source_scroll: UniformListScrollHandle,
    pub source_over: crate::overscroll::Overscroll,
    /// The step banner above the code: its scroll, and its stretch at the ends.
    pub note_scroll: gpui::ScrollHandle,
    pub note_over: crate::overscroll::Overscroll,
    /// The banner text's height as last laid out (0 before the first layout): the banner is
    /// never taller than this.
    pub note_fit: std::rc::Rc<std::cell::Cell<f32>>,
    /// The minimap's cached shape and drag state.
    pub minimap: crate::minimap::Minimap,
    /// The overview lane's cached marks.
    pub overview: crate::overview::Overview,
    /// The tour stop or call-stack frame opened here: its note above the code and its lines
    /// tinted while its file is on screen.
    pub step_mark: Option<super::investigation_view::StepMark>,
    /// The text selection (drag, double or triple click, Shift-click, Select All), tagged with
    /// the `epoch` it was made in. Read it through `selection_range`.
    pub selection: Option<crate::selection::Selection>,
    /// A drag selecting text is under way in this pane.
    pub drag: Option<SelectDrag>,
}

/// A text-selecting drag: how far the pointer is past the list's top (negative) or bottom
/// (positive), in pixels, and whether the auto-scroll loop is running for it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SelectDrag {
    pub beyond: f32,
    pub scrolling: bool,
}

impl EditorState {
    /// The selected bytes, if a selection of this document selects anything.
    pub fn selection_range(&self) -> Option<std::ops::Range<usize>> {
        let (sel, doc) = (self.selection.as_ref()?, self.doc.as_ref()?);
        let r = (sel.epoch == self.epoch).then(|| sel.range(&doc.buffer))?;
        (r.start < r.end).then_some(r)
    }

    /// The zero-based line the caret is on.
    pub fn selected_line(&self) -> Option<usize> {
        let (doc, caret) = (self.doc.as_ref()?, self.caret?);
        Some(doc.buffer.line_of(caret))
    }

    /// Whether `line` is shown in a row of its own and that row is fully inside the viewport
    /// as last laid out. Unknown (nothing laid out yet, or a scroll still pending) reads as
    /// not visible.
    pub fn line_in_view(&self, line: usize) -> bool {
        let (Some(folds), Some(vp)) = (&self.folds, self.viewport()) else {
            return false;
        };
        folds.visible_line(line) == line
            && row_in_view(folds.row_near_line(line), vp.top, vp.height, vp.row_h)
    }

    /// The list's scroll as last laid out; None before a layout or while a scroll is pending.
    pub fn viewport(&self) -> Option<Viewport> {
        let state = self.source_scroll.0.borrow();
        let size = state.last_item_size?;
        if state.deferred_scroll_to_item.is_some() {
            return None;
        }
        // GPUI's `item` is the list's own (viewport) size; every code row is LINE_HEIGHT tall.
        let vp = Viewport {
            top: -f32::from(state.base_handle.offset().y),
            row_h: LINE_HEIGHT,
            height: f32::from(size.item.height),
        };
        (vp.row_h > 0.0 && vp.height > 0.0).then_some(vp)
    }

    /// Where the view sits now: the caret's screen row when it is in view, else the top line.
    pub fn anchor(&self) -> Option<ViewAnchor> {
        let (doc, folds, vp) = (self.doc.as_ref()?, self.folds.as_ref()?, self.viewport()?);
        let caret_line = self.caret.map(|c| doc.buffer.line_of(c));
        let at = match caret_line.filter(|l| self.line_in_view(*l)) {
            Some(l) => Anchor::Caret {
                screen_y: folds.row_near_line(l) as f32 * vp.row_h - vp.top,
            },
            None => {
                let row = (vp.top / vp.row_h).floor() as usize;
                Anchor::Top {
                    line: folds.line_at_row(row),
                    offset: vp.top - row as f32 * vp.row_h,
                }
            }
        };
        Some(ViewAnchor { viewport: vp, at })
    }

    /// Scrolls so the view sits as `anchor` says, for the caret on `caret_line` (an anchor's
    /// top line is already on this document's side). Returns the top row it scrolled to, or
    /// None when it could not (no viewport): the caller centres instead.
    pub fn keep_view(&mut self, anchor: &ViewAnchor, caret_line: usize) -> Option<usize> {
        let folds = self.folds.as_mut()?;
        let (row, screen_y) = match anchor.at {
            Anchor::Caret { screen_y } => {
                folds.reveal(caret_line);
                (folds.row_near_line(caret_line), screen_y)
            }
            Anchor::Top { line, offset } => (folds.row_near_line(line), -offset),
        };
        let vp = &anchor.viewport;
        let top = anchored_offset(row, screen_y, folds.rows().len(), vp)?;
        let mut state = self.source_scroll.0.borrow_mut();
        // The new document's own pending scroll (to its top) gives way to this one.
        state.deferred_scroll_to_item = None;
        let x = state.base_handle.offset().x;
        state.base_handle.set_offset(gpui::point(x, gpui::px(-top)));
        Some((top / vp.row_h).round() as usize)
    }

    /// A copy for a pane split off this one: the same document (shared), caret, folds, marks,
    /// history and link, with a scroll handle of its own that is moved to the caret. The
    /// banner is not copied.
    pub fn fork(&self) -> EditorState {
        let scroll = UniformListScrollHandle::new();
        if let (Some(folds), Some(doc)) = (&self.folds, &self.doc) {
            let row = self
                .caret
                .map_or(0, |c| folds.row_near_line(doc.buffer.line_of(c)));
            scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        EditorState {
            doc: self.doc.clone(),
            folds: self.folds.clone(),
            caret: self.caret,
            marks: self.marks.clone(),
            find: self.find.clone(),
            // A split waits for the folds with a job of its own.
            late_folds: match self.late_folds {
                LateFolds::None => LateFolds::None,
                _ => LateFolds::Wanted,
            },
            fold_all_later: self.fold_all_later,
            banner: None,
            link: self.link.clone(),
            link_facts: self.link_facts.clone(),
            link_found: self.link_found,
            rev_index_ok: self.rev_index_ok,
            side: self.side,
            gutter: self.gutter.clone(),
            prepared: None,
            decls: self.decls.clone(),
            diff: self.diff.clone(),
            last_decl: None,
            pending_place: None,
            flash: None,
            lead_pair: false,
            wanted_side: None,
            call_kinds: self.call_kinds.clone(),
            blame: None,
            blame_on: false,
            loading: None,
            epoch: 1,
            generation: Generation::default(),
            history: self.history.clone(),
            error: None,
            landed: None,
            step_caret: None,
            source_scroll: scroll,
            source_over: Default::default(),
            note_scroll: Default::default(),
            note_over: Default::default(),
            note_fit: Default::default(),
            minimap: Default::default(),
            overview: Default::default(),
            step_mark: self.step_mark.clone(),
            selection: None,
            drag: None,
        }
    }
}

/// A pane that needs a state.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Add {
    pub pane: PaneId,
    /// The path the layout records for it (repository-relative, `/`-separated; may be empty).
    pub path: String,
    /// An existing pane showing the same file to copy the state of.
    pub copy_from: Option<PaneId>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Plan {
    pub drop: Vec<PaneId>,
    pub add: Vec<Add>,
}

/// Which states to drop and add. `layout` is every Editor pane of the layout with its recorded
/// path; `known` maps each existing state to the file it shows (`""` when none). A new pane
/// copies the active pane when that shows the same file, else any other that does.
pub(super) fn plan(
    layout: &BTreeMap<PaneId, String>,
    known: &BTreeMap<PaneId, String>,
    active: PaneId,
) -> Plan {
    let drop = known
        .keys()
        .filter(|p| !layout.contains_key(p))
        .copied()
        .collect();
    let add = layout
        .iter()
        .filter(|(p, _)| !known.contains_key(p))
        .map(|(pane, path)| {
            let same = |p: &&PaneId| !path.is_empty() && known.get(*p) == Some(path);
            let copy_from = Some(&active)
                .filter(layout_has(layout))
                .filter(same)
                .or_else(|| known.keys().filter(layout_has(layout)).find(same))
                .copied();
            Add {
                pane: *pane,
                path: path.clone(),
                copy_from,
            }
        })
        .collect();
    Plan { drop, add }
}

fn layout_has(layout: &BTreeMap<PaneId, String>) -> impl Fn(&&PaneId) -> bool + '_ {
    move |p| layout.contains_key(*p)
}

/// The active editor after a layout change. A focus change to an Editor pane wins; else the
/// active one stays while it exists; else the most recently focused remaining editor, else the
/// lowest id.
pub(super) fn choose_active(
    active: PaneId,
    focused: Option<PaneId>,
    last_focused: Option<PaneId>,
    recent: &[PaneId],
    editors: &BTreeSet<PaneId>,
) -> Option<PaneId> {
    if let Some(f) = focused.filter(|f| Some(*f) != last_focused && editors.contains(f)) {
        return Some(f);
    }
    if editors.contains(&active) {
        return Some(active);
    }
    recent
        .iter()
        .find(|p| editors.contains(p))
        .or_else(|| editors.iter().next())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchor_puts_its_row_back_at_the_same_screen_place() {
        let vp = Viewport {
            top: 200.0,
            row_h: 20.0,
            height: 100.0,
        };
        // The caret was 40 px down: its new row 30 goes 40 px down, so the top is row 28.
        assert_eq!(anchored_offset(30, 40.0, 100, &vp), Some(560.0));
        // A top line scrolled 5 px into its row stays so.
        assert_eq!(anchored_offset(12, -5.0, 100, &vp), Some(245.0));
        // Near the start or the end the list can't move that far: as close as it can, in view.
        assert_eq!(anchored_offset(1, 80.0, 100, &vp), Some(0.0));
        assert_eq!(anchored_offset(99, 0.0, 100, &vp), Some(1900.0));
        // A list shorter than the viewport never scrolls.
        assert_eq!(anchored_offset(3, 0.0, 4, &vp), Some(0.0));
        // No layout yet: no answer (the caller centres instead).
        let none = Viewport { height: 0.0, ..vp };
        assert_eq!(anchored_offset(3, 0.0, 100, &none), None);
    }

    #[test]
    fn a_row_is_in_view_only_when_wholly_inside_the_viewport() {
        // 20 px rows, a 100 px viewport scrolled 40 px: rows 2 to 6 are whole.
        assert!(row_in_view(2, 40.0, 100.0, 20.0));
        assert!(row_in_view(6, 40.0, 100.0, 20.0));
        assert!(!row_in_view(1, 40.0, 100.0, 20.0));
        assert!(!row_in_view(7, 40.0, 100.0, 20.0));
        // Half a row scrolled off the top is not in view.
        assert!(!row_in_view(2, 50.0, 100.0, 20.0));
        assert!(!row_in_view(0, 0.0, 0.0, 20.0));
    }

    fn map(v: &[(u64, &str)]) -> BTreeMap<PaneId, String> {
        v.iter().map(|(i, p)| (PaneId(*i), p.to_string())).collect()
    }

    #[test]
    fn a_new_pane_gets_a_state_copied_from_the_one_showing_its_file() {
        let layout = map(&[(1, "a.rs"), (5, "a.rs"), (6, "b.rs"), (7, "")]);
        let known = map(&[(1, "a.rs")]);
        let p = plan(&layout, &known, PaneId(1));
        assert!(p.drop.is_empty());
        assert_eq!(
            p.add,
            vec![
                Add {
                    pane: PaneId(5),
                    path: "a.rs".into(),
                    copy_from: Some(PaneId(1))
                },
                Add {
                    pane: PaneId(6),
                    path: "b.rs".into(),
                    copy_from: None
                },
                Add {
                    pane: PaneId(7),
                    path: String::new(),
                    copy_from: None
                },
            ]
        );
    }

    #[test]
    fn the_copy_prefers_the_active_pane_and_skips_closed_ones() {
        let layout = map(&[(1, "a.rs"), (2, "a.rs"), (3, "a.rs")]);
        let known = map(&[(1, "a.rs"), (2, "a.rs"), (9, "a.rs")]);
        let p = plan(&layout, &known, PaneId(2));
        assert_eq!(p.drop, vec![PaneId(9)]);
        assert_eq!(p.add[0].copy_from, Some(PaneId(2)));
        let p = plan(&layout, &known, PaneId(9));
        assert_eq!(p.add[0].copy_from, Some(PaneId(1)));
    }

    #[test]
    fn a_closed_pane_drops_its_state() {
        let layout = map(&[(1, "a.rs")]);
        let known = map(&[(1, "a.rs"), (2, "b.rs")]);
        let p = plan(&layout, &known, PaneId(1));
        assert_eq!(p.drop, vec![PaneId(2)]);
        assert!(p.add.is_empty());
        assert_eq!(plan(&layout, &layout, PaneId(1)), Plan::default());
    }

    #[test]
    fn the_active_editor_follows_focus_and_falls_back_to_the_most_recent() {
        let eds: BTreeSet<_> = [PaneId(1), PaneId(5), PaneId(8)].into();
        let ids = |v: &[u64]| v.iter().map(|i| PaneId(*i)).collect::<Vec<_>>();
        // Focus moved to an editor.
        assert_eq!(
            choose_active(PaneId(1), Some(PaneId(5)), Some(PaneId(2)), &[], &eds),
            Some(PaneId(5))
        );
        // Focus moved to a dock pane: the active editor stays.
        assert_eq!(
            choose_active(PaneId(1), Some(PaneId(2)), Some(PaneId(5)), &[], &eds),
            Some(PaneId(1))
        );
        // Focus unchanged: an explicit choice (a click) stays.
        assert_eq!(
            choose_active(PaneId(1), Some(PaneId(5)), Some(PaneId(5)), &[], &eds),
            Some(PaneId(1))
        );
        // The active editor closed: most recent remaining editor, else the lowest id.
        let left: BTreeSet<_> = [PaneId(1), PaneId(8)].into();
        assert_eq!(
            choose_active(PaneId(5), None, None, &ids(&[3, 8, 1]), &left),
            Some(PaneId(8))
        );
        assert_eq!(
            choose_active(PaneId(5), None, None, &ids(&[3]), &left),
            Some(PaneId(1))
        );
        assert_eq!(
            choose_active(PaneId(5), None, None, &[], &BTreeSet::new()),
            None
        );
    }
}

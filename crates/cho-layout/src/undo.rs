//! Layout undo and redo (design/layout-keyboard.md, "Layout undo").
//!
//! Arrangement changes are recorded: moves, splits, resizes (one step per split or dock until
//! the gesture is sealed), minimise, dock show and hide, close, focus layouts and their pages.
//! Focus moves, tab activation, opening panes and navigation are not. Undo restores where
//! panes are, never what they show: an editor keeps its current file and caret, a pane opened
//! since goes to its default place, and a pane closed since comes back with the content it
//! had when it closed.
//!
//! Snapshots are shared (`Arc`), so the clone every operation makes for atomicity copies
//! pointers, not a hundred layouts.

use crate::arrangement::{Arrangement, DockId};
use crate::focus::{FocusSession, PageOrigin};
use crate::history::FocusRecord;
use crate::tree::{Edge, NodeId};
use crate::workspace::{ReopenReport, Result, Target};
use crate::{LayoutError, PaneContent, PaneId, PaneSource, Workspace};
use std::collections::BTreeMap;
use std::sync::Arc;

/// How many steps are kept.
pub const UNDO_LIMIT: usize = 100;

/// Which repeated operation a step stands for, so a drag or a run of resize keys makes one
/// step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coalesce {
    Split(NodeId),
    Dock(DockId),
}

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    arrangement: Arrangement,
    focus: Option<FocusSession>,
    panes: BTreeMap<PaneId, PaneContent>,
    focused: Option<PaneId>,
}

/// The undo and redo stacks. Not saved, and left out of workspace equality.
#[derive(Clone, Debug, Default)]
pub struct UndoStack {
    past: Vec<(Arc<Snapshot>, Option<Coalesce>)>,
    future: Vec<Arc<Snapshot>>,
    /// Set when a gesture ends, so the next resize starts a new step even on the same split.
    sealed: bool,
}

impl UndoStack {
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn len(&self) -> usize {
        self.past.len()
    }

    pub fn is_empty(&self) -> bool {
        self.past.is_empty()
    }

    fn push(&mut self, before: Snapshot, key: Option<Coalesce>) {
        let merge =
            key.is_some() && !self.sealed && self.past.last().is_some_and(|(_, k)| *k == key);
        if !merge {
            self.past.push((Arc::new(before), key));
            if self.past.len() > UNDO_LIMIT {
                self.past.remove(0);
            }
        }
        self.future.clear();
        self.sealed = false;
    }
}

impl Workspace {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            arrangement: self.arrangement.clone(),
            focus: self.focus.clone(),
            panes: self.panes.clone(),
            focused: self.focused,
        }
    }

    fn differs_from(&self, s: &Snapshot) -> bool {
        self.arrangement != s.arrangement
            || self.focus != s.focus
            || !self.panes.keys().eq(s.panes.keys())
    }

    /// Runs an operation and records it as one undo step if it changed the layout. Nested
    /// operations (a split that opens a pane and moves it) record once, at the outside.
    pub(crate) fn recorded<T>(
        &mut self,
        key: Option<Coalesce>,
        f: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if self.undo_depth > 0 {
            return f(self);
        }
        let before = self.snapshot();
        self.undo_depth += 1;
        let out = f(self);
        self.undo_depth -= 1;
        if out.is_ok() && self.differs_from(&before) {
            self.undo.push(before, key);
        }
        out
    }

    /// Runs several operations as one: all or nothing, and one undo step (a drop that moves
    /// every tab of a group, a key that adds a page and moves a pane to it).
    pub fn batch<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.recorded(None, |ws| ws.transact(f))
    }

    pub fn undo_stack(&self) -> &UndoStack {
        &self.undo
    }

    /// Forgets undo and redo (after loading a layout, for example).
    pub fn clear_undo(&mut self) {
        self.undo = UndoStack::default();
    }

    /// Ends a gesture (a drag, a resize session): the next resize is a new undo step.
    pub fn seal_undo(&mut self) {
        self.undo.sealed = true;
    }

    /// Undoes the last recorded layout change.
    pub fn undo(&mut self) -> Result<()> {
        let (before, _) = self.undo.past.pop().ok_or(LayoutError::NothingToUndo)?;
        self.undo.future.push(Arc::new(self.snapshot()));
        self.apply_snapshot(&before);
        self.undo.sealed = true;
        Ok(())
    }

    /// Redoes the last undone layout change.
    pub fn redo(&mut self) -> Result<()> {
        let after = self.undo.future.pop().ok_or(LayoutError::NothingToRedo)?;
        self.undo.past.push((Arc::new(self.snapshot()), None));
        self.apply_snapshot(&after);
        self.undo.sealed = true;
        Ok(())
    }

    /// Puts panes back where `s` had them, keeping today's content.
    fn apply_snapshot(&mut self, s: &Snapshot) {
        for (id, content) in &s.panes {
            self.panes.entry(*id).or_insert_with(|| content.clone());
        }
        let opened: Vec<PaneId> = self
            .panes
            .keys()
            .copied()
            .filter(|p| !s.panes.contains_key(p))
            .collect();
        self.arrangement = s.arrangement.clone();
        self.focus = s.focus.clone();
        for p in opened {
            self.place_default(p);
        }
        self.settle_editor_area();
        let panes = &self.panes;
        self.recent.retain(|p| panes.contains_key(p));
        if self.focused.is_none_or(|p| !self.panes.contains_key(&p)) {
            self.focused = s.focused;
        }
    }

    // ----- recorded operations ---------------------------------------------------------------
    // Each runs its `_raw` implementation in workspace.rs and records it as one step.

    /// Moves one pane (a tab) to `target`. In a focus layout this rearranges the current page
    /// only (a pane on another page or in the tray comes to this one); `Tray` sends a pane back
    /// to the tray.
    ///
    /// Recorded for undo.
    pub fn move_pane(&mut self, pane: PaneId, target: Target) -> Result<()> {
        self.recorded(None, |ws| ws.move_pane_raw(pane, target))
    }

    /// Moves a whole group (dragged by its header) to `target`, keeping its tab order, active
    /// tab and minimised state.
    ///
    /// Recorded for undo.
    pub fn move_group(&mut self, group: NodeId, target: Target) -> Result<()> {
        self.recorded(None, |ws| ws.move_group_raw(group, target))
    }

    /// Splits beside `pane` on `edge`. An editor pane is duplicated (same file and caret) and
    /// the new pane's id returned; another pane can be split off its tab strip when it shares
    /// one, and its own id is returned.
    ///
    /// Recorded for undo.
    pub fn split(&mut self, pane: PaneId, edge: Edge) -> Result<PaneId> {
        self.recorded(None, |ws| ws.split_raw(pane, edge))
    }

    /// Drags the handle between child `handle` and `handle + 1` of a split by `delta`, a share
    /// of the split's length (positive grows the first). Each child keeps at least
    /// [`crate::MIN_WEIGHT`]. Returns the delta actually applied.
    ///
    /// Consecutive resizes of the same split make one undo step until
    /// [`Self::seal_undo`].
    pub fn resize_split(&mut self, split: NodeId, handle: usize, delta: f32) -> Result<f32> {
        self.recorded(Some(Coalesce::Split(split)), |ws| {
            ws.resize_split_raw(split, handle, delta)
        })
    }

    /// Sets a dock's width (side docks) or height (bottom bar), clamped to its range. Returns
    /// the size set.
    ///
    /// Consecutive resizes of a dock make one undo step.
    pub fn resize_dock(&mut self, dock: DockId, size: f32) -> Result<f32> {
        self.recorded(Some(Coalesce::Dock(dock)), |ws| {
            ws.resize_dock_raw(dock, size)
        })
    }

    /// Folds the group holding `pane` to its header, or unfolds it. Returns the new state.
    ///
    /// Recorded for undo.
    pub fn toggle_minimize(&mut self, pane: PaneId) -> Result<bool> {
        self.recorded(None, |ws| ws.toggle_minimize_raw(pane))
    }

    /// Shows or hides a dock. Hidden docks keep their panes. Returns the new visibility.
    ///
    /// Recorded for undo.
    pub fn toggle_dock(&mut self, dock: DockId) -> Result<bool> {
        self.recorded(None, |ws| ws.toggle_dock_raw(dock))
    }

    /// Closes a pane everywhere. Closing a pin unpins it. If no page of a focus layout has a
    /// pane left, the focus layout ends without recording history (there is nothing to record).
    ///
    /// Recorded: undo brings it back with its content.
    pub fn close(&mut self, pane: PaneId) -> Result<()> {
        self.recorded(None, |ws| ws.close_raw(pane))
    }

    /// Close as the pane menu and a tab's × mean it: in a focus layout a pane on a page goes
    /// to the tray, kept for later, as closing a page does with its panes; anywhere else (or
    /// a pane already in the tray) it closes. Recorded for undo.
    pub fn dismiss(&mut self, pane: PaneId) -> Result<()> {
        let on_page = self.focus().is_some_and(|f| f.page_of(pane).is_some());
        if on_page {
            self.move_pane(pane, Target::Tray)
        } else {
            self.close(pane)
        }
    }

    /// Maximises `pane`: a focus layout opens with one page holding it alone, and everything
    /// else in the tray. `now` is seconds since the Unix epoch.
    ///
    /// Recorded for undo.
    pub fn maximize(&mut self, pane: PaneId, now: u64) -> Result<()> {
        self.recorded(None, |ws| ws.maximize_raw(pane, now))
    }

    /// Ends the focus layout, every page of it: the normal arrangement comes back exactly as it
    /// was, except that the editor pane read last becomes its group's active tab (so the
    /// reading position is kept), and panes opened during focus are in their default places.
    /// The pages are recorded in focus history and the record returned (`None` when no page
    /// had a pane).
    ///
    /// Recorded: undo reopens it.
    pub fn restore(&mut self, now: u64) -> Result<Option<FocusRecord>> {
        self.recorded(None, |ws| ws.restore_raw(now))
    }

    /// Adds a page after the last one and shows it. Returns its index.
    ///
    /// A page with no origin (the focus bar's `+`) starts with the focused pane, taken from
    /// the page it was on, unless it is alone there (then the new page starts empty, so no
    /// page is left empty behind it). A page with an origin (a tour step) starts empty;
    /// [`Workspace::reset_page`] fills it from the panes the origin declares. Panes come to a
    /// page from the tray or from other pages.
    ///
    /// Recorded for undo.
    pub fn new_page(&mut self, origin: Option<PageOrigin>) -> Result<usize> {
        self.recorded(None, |ws| ws.new_page_raw(origin))
    }

    /// Closes page `index`; its panes go to the tray. Closing the last page ends the focus
    /// layout (as Restore does, recording it).
    ///
    /// Recorded for undo.
    pub fn close_page(&mut self, index: usize, now: u64) -> Result<()> {
        self.recorded(None, |ws| ws.close_page_raw(index, now))
    }

    /// Moves page `from` to position `to`, keeping the same page on screen.
    ///
    /// Recorded for undo.
    pub fn move_page(&mut self, from: usize, to: usize) -> Result<()> {
        self.recorded(None, |ws| ws.move_page_raw(from, to))
    }

    /// Moves `pane` (from any page, or the tray) to page `index`, at the right of what is
    /// there. The page on screen does not change.
    ///
    /// Recorded for undo.
    pub fn move_pane_to_page(&mut self, pane: PaneId, index: usize) -> Result<()> {
        self.recorded(None, |ws| ws.move_pane_to_page_raw(pane, index))
    }

    /// Puts a pane from the tray (or another page) onto an empty current page. Used when the
    /// page has no group to drop beside.
    ///
    /// Recorded for undo.
    pub fn place_on_empty_page(&mut self, pane: PaneId) -> Result<()> {
        self.recorded(None, |ws| ws.place_on_empty_page_raw(pane))
    }

    /// Rebuilds page `index` from its origin (Cmd-K R on a tour page): the panes the origin
    /// declares, in order and side by side, taken from the tray or other pages, or opened
    /// fresh when no pane shows that source and `available` says it can be shown. Whatever
    /// else was on the page goes to the tray. Sources that cannot be shown are reported as
    /// missing. A page with no origin, or an origin with no panes, cannot be reset.
    ///
    /// Recorded for undo.
    pub fn reset_page(
        &mut self,
        index: usize,
        available: impl Fn(&PaneSource) -> bool,
    ) -> Result<ReopenReport> {
        self.recorded(None, |ws| ws.reset_page_raw(index, available))
    }

    /// Reopens focus history entry `index` with current content, every page of it. An open
    /// focus layout is restored first. Sources that cannot be shown are left out and reported
    /// with their page; a page left with no pane is dropped.
    ///
    /// Recorded for undo.
    pub fn reopen_focus(
        &mut self,
        index: usize,
        now: u64,
        available: impl Fn(&PaneSource) -> bool,
    ) -> Result<ReopenReport> {
        self.recorded(None, |ws| ws.reopen_focus_raw(index, now, available))
    }
}

//! The workspace: every pane, the normal arrangement, the focus session when there is one, and
//! focus history. Every operation is deterministic and atomic: on error nothing changes.

use crate::arrangement::{Arrangement, DockId, Region};
use crate::focus::{FocusPage, FocusSession, MAIN_SHARE, PageArrangement, PageOrigin};
use crate::history::{self, FocusRecord, HistoryEntry, RecordNode};
use crate::tree::{Axis, Edge, Group, MIN_WEIGHT, Node, NodeId, normalize, normalize_in_place};
use crate::{DefaultPlace, PaneContent, PaneId, PaneKind, PaneSource, Pin};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("there is no pane {0:?}")]
    UnknownPane(PaneId),
    #[error("there is no group {0:?} here")]
    UnknownGroup(NodeId),
    #[error("there is no split {0:?} here")]
    UnknownSplit(NodeId),
    #[error("code panes stay in the code area")]
    EditorOutsideEditorArea,
    #[error("code tabs and tool tabs do not share a tab strip")]
    MixedKinds,
    #[error("there is no code group to put it beside")]
    NoEditorGroup,
    #[error("only code panes have a reading position")]
    NotAnEditor,
    #[error("that would change nothing")]
    NoOp,
    #[error("the docks are hidden in a focus layout")]
    DocksHiddenInFocus,
    #[error("there is no focus layout")]
    NotInFocus,
    #[error("a focus layout is already open")]
    AlreadyInFocus,
    #[error("this pane cannot be split")]
    CannotSplit,
    #[error("pane {0:?} is not on screen")]
    NotOnScreen(PaneId),
    #[error("there is no focus page {0}")]
    NoSuchPage(usize),
    #[error("only a page made for a tour step can be reset")]
    NoPageOrigin,
    #[error("there is no focus layout {0} in the history")]
    NoSuchHistoryEntry(usize),
    #[error("none of that focus layout's panes can be shown any more")]
    NothingToReopen,
    #[error("nothing that way can take it")]
    NothingThatWay,
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("nothing to redo")]
    NothingToRedo,
    #[error("focus a pane in a dock first")]
    NotInDock,
}

pub type Result<T> = std::result::Result<T, LayoutError>;

/// Where a moved or opened pane goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The dock's default spot: stacked at the bottom of a side dock, or as the last tab of the
    /// bottom bar. Tool panes only.
    Dock(DockId),
    /// The editor area: an editor pane becomes a tab of the current editor group; a tool pane
    /// becomes its own group to the right of it.
    EditorArea,
    /// A new group beside (or above, or below) an existing group.
    Beside { group: NodeId, edge: Edge },
    /// A tab in an existing group's tab strip, before the tab now at `index` (clamped).
    Tabs { group: NodeId, index: usize },
    /// Off screen, waiting in the tray. Focus layouts only.
    Tray,
    /// A new group along the whole of one edge of the editor area (in a focus layout, of the
    /// current page), spanning every group there: a full-width row at the bottom, say.
    AreaEdge(Edge),
}

/// The share of the area a pane put along its whole edge ([`Target::AreaEdge`]) starts with:
/// less than half, so a full-width row does not squash everything above it.
pub const AREA_EDGE_SHARE: f32 = 1.0 / 3.0;

/// Where Pin puts a pin (layout.md, "Pinning context").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinPlace {
    /// Directly under the live Context pane, in the same dock.
    Here,
    /// As a tab in the bottom bar, after the tool tabs.
    BottomBar,
}

/// What reopening a focus layout did.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ReopenReport {
    /// Sources that could not be shown and were left out, with the page they were on (as
    /// recorded).
    pub missing: Vec<(usize, PaneSource)>,
    /// Panes created for sources that were closed but still exist.
    pub created: Vec<PaneId>,
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub(crate) panes: BTreeMap<PaneId, PaneContent>,
    pub(crate) arrangement: Arrangement,
    pub(crate) focus: Option<FocusSession>,
    pub(crate) history: Vec<FocusRecord>,
    pub(crate) focused: Option<PaneId>,
    pub(crate) next_id: u64,
    /// Most recently focused first.
    pub(crate) recent: Vec<PaneId>,
    pub(crate) undo: crate::undo::UndoStack,
    /// Nesting of recorded operations, so only the outermost records.
    pub(crate) undo_depth: u32,
    /// Which docks Toggle all docks hid, to bring the same ones back.
    pub(crate) all_docks: Option<[bool; 3]>,
}

/// Two workspaces are equal when they hold the same panes, arrangement, focus layout, focus
/// history, focused pane, recency, id counter and remembered docks. Undo and redo are left out
/// (they are not saved, so a loaded workspace equals the one saved); compare
/// [`Workspace::undo_stack`] separately where it matters.
impl PartialEq for Workspace {
    fn eq(&self, other: &Self) -> bool {
        // Destructured so a new field cannot be left out of the comparison unnoticed.
        let Workspace {
            panes,
            arrangement,
            focus,
            history,
            focused,
            next_id,
            recent,
            undo: _,
            undo_depth: _,
            all_docks,
        } = self;
        *panes == other.panes
            && *arrangement == other.arrangement
            && *focus == other.focus
            && *history == other.history
            && *focused == other.focused
            && *next_id == other.next_id
            && *recent == other.recent
            && *all_docks == other.all_docks
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

fn is_editor(kind: Option<PaneKind>) -> bool {
    kind == Some(PaneKind::Editor)
}

impl Workspace {
    /// An empty workspace: no panes, every dock empty.
    pub fn new() -> Self {
        Self {
            panes: BTreeMap::new(),
            arrangement: Arrangement::default(),
            focus: None,
            history: Vec::new(),
            focused: None,
            next_id: 1,
            recent: Vec::new(),
            undo: Default::default(),
            undo_depth: 0,
            all_docks: None,
        }
    }

    /// The default layout of layout.md: the file tree on the left, Context and Outline stacked
    /// on the right, an empty editor area and bottom bar.
    pub fn with_defaults() -> Self {
        let mut ws = Self::new();
        for content in [
            PaneContent::FileTree,
            PaneContent::Context,
            PaneContent::Outline,
        ] {
            ws.open(content, None).expect("default panes place cleanly");
        }
        ws
    }

    // ----- reading ---------------------------------------------------------------------------

    pub fn panes(&self) -> &BTreeMap<PaneId, PaneContent> {
        &self.panes
    }

    pub fn content(&self, pane: PaneId) -> Option<&PaneContent> {
        self.panes.get(&pane)
    }

    pub fn kind(&self, pane: PaneId) -> Option<PaneKind> {
        self.panes.get(&pane).map(PaneContent::kind)
    }

    /// The normal arrangement. While a focus layout is open this is what Restore returns to.
    pub fn arrangement(&self) -> &Arrangement {
        &self.arrangement
    }

    pub fn focus(&self) -> Option<&FocusSession> {
        self.focus.as_ref()
    }

    pub fn in_focus(&self) -> bool {
        self.focus.is_some()
    }

    pub fn focused(&self) -> Option<PaneId> {
        self.focused
    }

    pub fn history(&self) -> &[FocusRecord] {
        &self.history
    }

    /// Panes on no page while a focus layout is open, in reading order of the normal
    /// arrangement (left dock, editor, right dock, bottom bar). Empty outside focus.
    pub fn tray(&self) -> Vec<PaneId> {
        let Some(f) = &self.focus else {
            return Vec::new();
        };
        let on_pages: BTreeSet<PaneId> = f.on_pages().into_iter().collect();
        self.arrangement
            .panes()
            .into_iter()
            .filter(|p| !on_pages.contains(p))
            .collect()
    }

    /// The group holding `pane` in what is on screen now (the current focus page if a focus
    /// layout is open).
    pub fn group_of(&self, pane: PaneId) -> Option<NodeId> {
        match &self.focus {
            Some(f) => f.group_of(pane),
            None => self.arrangement.locate(pane).map(|(_, g)| g),
        }
    }

    /// A group on screen now.
    pub fn group(&self, id: NodeId) -> Option<&Group> {
        match &self.focus {
            Some(f) => f.group(id),
            None => self.arrangement_group(id),
        }
    }

    /// Where a tab dragged onto `group` at `index` would actually land, after the rule that
    /// pins follow tool tabs. The lab draws its 2 px accent line here.
    pub fn tab_drop_index(&self, group: NodeId, pane: PaneId, index: usize) -> Option<usize> {
        let g = self.group(group)?;
        let mut tabs = g.tabs.clone();
        let mut index = index.min(tabs.len());
        if let Some(i) = tabs.iter().position(|p| *p == pane) {
            tabs.remove(i);
            if index > i {
                index -= 1;
            }
        }
        Some(self.clamp_tab_index(&tabs, pane, index))
    }

    // ----- opening and closing ---------------------------------------------------------------

    /// Adds a pane showing `content` at `target`, or at its kind's default place. In a focus
    /// layout the pane also gets its default place in the normal arrangement, so Restore has
    /// somewhere to put it; on screen it goes to `target` (`Dock` and `None` mean the tray).
    pub fn open(&mut self, content: PaneContent, target: Option<Target>) -> Result<PaneId> {
        self.transact(|ws| {
            let id = ws.alloc_pane(content);
            if ws.focus.is_none() {
                match target {
                    Some(t) => ws.place(id, t)?,
                    None => ws.place_default(id),
                }
                return Ok(id);
            }
            ws.place_default(id);
            match target {
                Some(t @ (Target::Beside { .. } | Target::Tabs { .. } | Target::AreaEdge(_))) => {
                    ws.place_on_focus(id, t)?
                }
                Some(Target::EditorArea) => {
                    let near = ws
                        .focus
                        .as_ref()
                        .and_then(|f| f.last_editor.and_then(|e| f.group_of(e)));
                    if let Some(group) = near {
                        ws.place_on_focus(
                            id,
                            Target::Tabs {
                                group,
                                index: usize::MAX,
                            },
                        )?;
                    }
                }
                Some(Target::Dock(_) | Target::Tray) | None => {}
            }
            Ok(id)
        })
    }

    /// The work of [`Workspace::close`], not recorded for undo.
    pub(crate) fn close_raw(&mut self, pane: PaneId) -> Result<()> {
        self.transact(|ws| {
            ws.require(pane)?;
            let group = ws.group_of(pane);
            ws.panes.remove(&pane);
            ws.recent.retain(|p| *p != pane);
            ws.detach_from_arrangement(pane);
            ws.settle_editor_area();
            if let Some(f) = &mut ws.focus {
                f.remove(pane);
                if f.on_pages().is_empty() {
                    ws.focus = None;
                } else {
                    if f.maximized == pane {
                        f.maximized = f.on_pages()[0];
                    }
                    f.forget_off_page_editor();
                }
            }
            if ws.focused == Some(pane) {
                ws.focused = group
                    .and_then(|g| ws.group(g))
                    .and_then(Group::active_pane)
                    .or_else(|| ws.current_editor_group().and_then(|g| ws.active_in(g)));
            }
            Ok(())
        })
    }

    /// Makes `pane` the active tab of its group and the focused pane.
    pub fn activate(&mut self, pane: PaneId) -> Result<()> {
        self.transact(|ws| {
            ws.require(pane)?;
            let group = ws.group_of(pane).ok_or(LayoutError::NotOnScreen(pane))?;
            let g = ws.group_mut(group).expect("group just found");
            g.active = g
                .tabs
                .iter()
                .position(|p| *p == pane)
                .expect("pane in group");
            ws.focused = Some(pane);
            ws.touch(pane);
            ws.note_editor(pane);
            Ok(())
        })
    }

    /// Records where an editor pane is reading. `caret` is a UTF-8 byte offset into `path`.
    pub fn set_editor_position(&mut self, pane: PaneId, path: &str, caret: usize) -> Result<()> {
        match self.panes.get_mut(&pane) {
            Some(PaneContent::Editor { path: p, caret: c }) => {
                *p = path.to_string();
                *c = caret;
                self.note_editor(pane);
                Ok(())
            }
            Some(_) => Err(LayoutError::NotAnEditor),
            None => Err(LayoutError::UnknownPane(pane)),
        }
    }

    // ----- moving ----------------------------------------------------------------------------

    /// The work of [`Workspace::move_pane`], not recorded for undo.
    pub(crate) fn move_pane_raw(&mut self, pane: PaneId, target: Target) -> Result<()> {
        self.transact(|ws| {
            ws.validate(pane, target)?;
            if let Target::Tabs { group, index } = target
                && ws.group_of(pane) == Some(group)
            {
                return ws.reorder(group, pane, index);
            }
            if ws.focus.is_some() {
                return ws.move_on_focus(pane, target);
            }
            ws.detach_from_arrangement(pane);
            ws.place(pane, target)?;
            ws.settle_editor_area();
            Ok(())
        })
    }

    /// The work of [`Workspace::move_group`], not recorded for undo.
    pub(crate) fn move_group_raw(&mut self, group: NodeId, target: Target) -> Result<()> {
        self.transact(|ws| {
            let g = ws
                .group(group)
                .cloned()
                .ok_or(LayoutError::UnknownGroup(group))?;
            if matches!(target, Target::Beside { group: t, .. } | Target::Tabs { group: t, .. } if t == group)
            {
                return Err(LayoutError::NoOp);
            }
            let first = g.tabs[0];
            ws.move_pane(first, target)?;
            if matches!(target, Target::Tray) {
                for p in &g.tabs[1..] {
                    ws.move_pane(*p, Target::Tray)?;
                }
                return Ok(());
            }
            let new_group = ws.group_of(first).expect("moved pane is on screen");
            for p in &g.tabs[1..] {
                ws.move_pane(
                    *p,
                    Target::Tabs {
                        group: new_group,
                        index: usize::MAX,
                    },
                )?;
            }
            let landed = ws.group_mut(new_group).expect("group exists");
            if g.tabs.len() == landed.tabs.len() {
                landed.minimized = g.minimized;
            }
            if let Some(a) = g.active_pane()
                && let Some(i) = landed.tabs.iter().position(|p| *p == a)
            {
                landed.active = i;
            }
            Ok(())
        })
    }

    /// The work of [`Workspace::split`], not recorded for undo.
    pub(crate) fn split_raw(&mut self, pane: PaneId, edge: Edge) -> Result<PaneId> {
        self.transact(|ws| {
            let content = ws
                .panes
                .get(&pane)
                .cloned()
                .ok_or(LayoutError::UnknownPane(pane))?;
            let group = ws.group_of(pane).ok_or(LayoutError::NotOnScreen(pane))?;
            if content.kind() == PaneKind::Editor {
                return ws.open(content, Some(Target::Beside { group, edge }));
            }
            if ws.group(group).map_or(0, |g| g.tabs.len()) < 2 {
                return Err(LayoutError::CannotSplit);
            }
            ws.move_pane(pane, Target::Beside { group, edge })?;
            Ok(pane)
        })
    }

    // ----- sizes, minimise, docks ------------------------------------------------------------

    /// The work of [`Workspace::resize_split`], not recorded for undo.
    pub(crate) fn resize_split_raw(
        &mut self,
        split: NodeId,
        handle: usize,
        delta: f32,
    ) -> Result<f32> {
        if !delta.is_finite() {
            return Err(LayoutError::NoOp);
        }
        let s = self
            .split_mut(split)
            .ok_or(LayoutError::UnknownSplit(split))?;
        if handle + 1 >= s.children.len() {
            return Err(LayoutError::UnknownSplit(split));
        }
        let (a, b) = (s.children[handle].weight, s.children[handle + 1].weight);
        let lo = (MIN_WEIGHT - a).min(0.0);
        let hi = (b - MIN_WEIGHT).max(0.0);
        let d = delta.clamp(lo, hi);
        s.children[handle].weight = a + d;
        s.children[handle + 1].weight = b - d;
        Ok(d)
    }

    /// The work of [`Workspace::resize_dock`], not recorded for undo.
    pub(crate) fn resize_dock_raw(&mut self, dock: DockId, size: f32) -> Result<f32> {
        if self.focus.is_some() {
            return Err(LayoutError::DocksHiddenInFocus);
        }
        if !size.is_finite() {
            return Err(LayoutError::NoOp);
        }
        let (lo, hi) = dock.size_range();
        let size = size.clamp(lo, hi);
        self.arrangement.dock_mut(dock).size = size;
        Ok(size)
    }

    /// The work of [`Workspace::toggle_minimize`], not recorded for undo.
    pub(crate) fn toggle_minimize_raw(&mut self, pane: PaneId) -> Result<bool> {
        self.require(pane)?;
        let group = self.group_of(pane).ok_or(LayoutError::NotOnScreen(pane))?;
        let g = self.group_mut(group).expect("group just found");
        g.minimized = !g.minimized;
        Ok(g.minimized)
    }

    /// The work of [`Workspace::toggle_dock`], not recorded for undo.
    pub(crate) fn toggle_dock_raw(&mut self, dock: DockId) -> Result<bool> {
        if self.focus.is_some() {
            return Err(LayoutError::DocksHiddenInFocus);
        }
        let d = self.arrangement.dock_mut(dock);
        d.visible = !d.visible;
        Ok(d.visible)
    }

    // ----- pins ------------------------------------------------------------------------------

    /// Freezes the context into a pin pane and returns its id. Focus does not move: pinning is
    /// for reading on. In a focus layout the pin goes to its place in the normal arrangement,
    /// and on screen under the Context pane if that is on the current page (otherwise to the
    /// tray).
    pub fn pin_context(&mut self, pin: Pin, place: PinPlace) -> Result<PaneId> {
        self.transact(|ws| {
            let id = ws.alloc_pane(PaneContent::PinnedContext(pin));
            let context = ws.first_of_kind(PaneKind::Context);
            match (place, context.and_then(|c| ws.arrangement.locate(c))) {
                (PinPlace::Here, Some((region, group))) => {
                    let split_id = ws.alloc_node();
                    let node = ws.new_group(vec![id]);
                    ws.arrangement
                        .root_mut(region)
                        .as_mut()
                        .expect("region holds the context pane")
                        .insert_beside(group, Edge::Bottom, node, split_id)
                        .expect("context group exists");
                    ws.show_region(region);
                }
                (PinPlace::Here, None) => ws.place_in_dock(DockId::Right, id),
                (PinPlace::BottomBar, _) => ws.place_in_dock(DockId::Bottom, id),
            }
            if let (Some(f), PinPlace::Here, Some(c)) = (&ws.focus, place, context)
                && let Some(group) = f.group_of(c)
            {
                ws.place_on_focus(
                    id,
                    Target::Beside {
                        group,
                        edge: Edge::Bottom,
                    },
                )?;
            }
            Ok(id)
        })
    }

    // ----- focus layouts ---------------------------------------------------------------------

    /// The work of [`Workspace::maximize`], not recorded for undo.
    pub(crate) fn maximize_raw(&mut self, pane: PaneId, now: u64) -> Result<()> {
        self.require(pane)?;
        if self.focus.is_some() {
            return Err(LayoutError::AlreadyInFocus);
        }
        let root = self.new_group(vec![pane]);
        self.focus = Some(FocusSession {
            pages: vec![FocusPage {
                root: Some(root),
                origin: None,
            }],
            current: 0,
            maximized: pane,
            last_editor: is_editor(self.kind(pane)).then_some(pane),
            started_at: now,
        });
        self.focused = Some(pane);
        Ok(())
    }

    /// Cmd-K M: maximise `pane`, or restore when a focus layout is open (the maximise button
    /// reads Restore size there). Returns whether a focus layout is open afterwards.
    pub fn toggle_maximize(&mut self, pane: PaneId, now: u64) -> Result<bool> {
        if self.focus.is_some() {
            self.restore(now)?;
            Ok(false)
        } else {
            self.maximize(pane, now)?;
            Ok(true)
        }
    }

    /// The arrangement Restore would return to now: the normal arrangement with the reading
    /// position applied. This is what gets saved while a focus layout is open.
    pub(crate) fn restored_arrangement(&self) -> Arrangement {
        let mut a = self.arrangement.clone();
        if let Some(e) = self.focus.as_ref().and_then(|f| f.last_editor)
            && let Some((region, group)) = a.locate(e)
            && let Some(g) = a.root_mut(region).as_mut().and_then(|n| n.group_mut(group))
        {
            g.active = g.tabs.iter().position(|p| *p == e).expect("pane in group");
        }
        a
    }

    /// The work of [`Workspace::restore`], not recorded for undo.
    pub(crate) fn restore_raw(&mut self, now: u64) -> Result<Option<FocusRecord>> {
        let arrangement = self.restored_arrangement();
        let session = self.focus.take().ok_or(LayoutError::NotInFocus)?;
        let record = FocusRecord::of(&session, &self.panes, now);
        if let Some(r) = &record {
            history::push(&mut self.history, r.clone());
        }
        self.arrangement = arrangement;
        Ok(record)
    }

    // ----- focus pages -----------------------------------------------------------------------

    /// The work of [`Workspace::new_page`], not recorded for undo.
    pub(crate) fn new_page_raw(&mut self, origin: Option<PageOrigin>) -> Result<usize> {
        self.transact(|ws| {
            let focused = ws.focused;
            let f = ws.focus.as_mut().ok_or(LayoutError::NotInFocus)?;
            let bring = focused.filter(|p| {
                origin.is_none() && f.page_of(*p) == Some(f.current) && f.on_screen().len() > 1
            });
            f.pages.push(FocusPage { root: None, origin });
            let i = f.pages.len() - 1;
            if let Some(pane) = bring {
                ws.move_pane_to_page_raw(pane, i)?;
            }
            ws.go_to_page(i)?;
            if bring.is_some() {
                ws.focused = bring;
            }
            Ok(i)
        })
    }

    /// The work of [`Workspace::reset_page`], not recorded for undo.
    pub(crate) fn reset_page_raw(
        &mut self,
        index: usize,
        available: impl Fn(&PaneSource) -> bool,
    ) -> Result<ReopenReport> {
        self.transact(|ws| {
            let f = ws.focus.as_mut().ok_or(LayoutError::NotInFocus)?;
            let page = f
                .pages
                .get_mut(index)
                .ok_or(LayoutError::NoSuchPage(index))?;
            let (declared, arrangement) = match &page.origin {
                Some(o) if !o.panes.is_empty() => (o.panes.clone(), o.arrangement),
                _ => return Err(LayoutError::NoPageOrigin),
            };
            page.root = None;
            let mut report = ReopenReport::default();
            let mut used = BTreeSet::new();
            let mut placed: Vec<PaneId> = Vec::new();
            for src in &declared {
                // A matching pane waiting in the tray is taken before one on another page,
                // so a host can open the panes a page should get and keep other pages whole.
                let in_tray = ws
                    .tray()
                    .into_iter()
                    .find(|p| !used.contains(p) && ws.panes[p].source() == *src);
                let Some(id) = in_tray
                    .or_else(|| ws.pane_for_source(src, &available, &used, &mut report.created))
                else {
                    report.missing.push((index, src.clone()));
                    continue;
                };
                ws.focus.as_mut().expect("in focus").remove(id);
                used.insert(id);
                placed.push(id);
            }
            let root = ws.arranged(&placed, arrangement);
            let first_editor = placed.iter().copied().find(|p| is_editor(ws.kind(*p)));
            let f = ws.focus.as_mut().expect("in focus");
            f.pages[index].root = root;
            if first_editor.is_some() {
                f.last_editor = first_editor;
            }
            f.forget_off_page_editor();
            ws.go_to_page(index)?;
            Ok(report)
        })
    }

    /// One node holding `panes` (one group each) laid out by `arrangement`.
    fn arranged(&mut self, panes: &[PaneId], arrangement: PageArrangement) -> Option<Node> {
        let groups: Vec<Node> = panes.iter().map(|p| self.new_group(vec![*p])).collect();
        let line = |ws: &mut Self, axis: Axis, nodes: Vec<Node>| -> Option<Node> {
            let mut out: Option<Node> = None;
            for node in nodes {
                out = Some(match out {
                    None => node,
                    Some(old) => {
                        let id = ws.alloc_node();
                        old.append(axis, node, id)
                    }
                });
            }
            out
        };
        match arrangement {
            PageArrangement::Single | PageArrangement::SideBySide => line(self, Axis::Row, groups),
            PageArrangement::Stacked => line(self, Axis::Column, groups),
            PageArrangement::MainSide => {
                let mut it = groups.into_iter();
                let main = it.next()?;
                let Some(side) = line(self, Axis::Column, it.collect()) else {
                    return Some(main);
                };
                let id = self.alloc_node();
                match main.append(Axis::Row, side, id) {
                    Node::Split(mut s) => {
                        s.children[0].weight = MAIN_SHARE;
                        s.children[1].weight = 1.0 - MAIN_SHARE;
                        Some(Node::Split(s))
                    }
                    other => Some(other),
                }
            }
            PageArrangement::Grid => {
                let mut rows = Vec::new();
                let mut it = groups.into_iter().peekable();
                while it.peek().is_some() {
                    let pair: Vec<Node> = it.by_ref().take(2).collect();
                    rows.extend(line(self, Axis::Row, pair));
                }
                line(self, Axis::Column, rows)
            }
        }
    }

    /// Shows page `index`. The reading position (the last editor and its caret) carries over.
    pub fn go_to_page(&mut self, index: usize) -> Result<()> {
        let f = self.focus.as_mut().ok_or(LayoutError::NotInFocus)?;
        if index >= f.pages.len() {
            return Err(LayoutError::NoSuchPage(index));
        }
        f.current = index;
        // Focus the reading position if it is on this page, else the page's first active tab.
        let on_page = f.on_screen();
        let pick = f
            .last_editor
            .filter(|e| on_page.contains(e))
            .or_else(|| f.first_active());
        if pick.is_some() {
            self.focused = pick;
        }
        Ok(())
    }

    /// The next page; `NoOp` on the last.
    pub fn next_page(&mut self) -> Result<()> {
        let f = self.focus.as_ref().ok_or(LayoutError::NotInFocus)?;
        if f.current + 1 >= f.pages.len() {
            return Err(LayoutError::NoOp);
        }
        self.go_to_page(f.current + 1)
    }

    /// The previous page; `NoOp` on the first.
    pub fn prev_page(&mut self) -> Result<()> {
        let f = self.focus.as_ref().ok_or(LayoutError::NotInFocus)?;
        if f.current == 0 {
            return Err(LayoutError::NoOp);
        }
        self.go_to_page(f.current - 1)
    }

    /// The work of [`Workspace::close_page`], not recorded for undo.
    pub(crate) fn close_page_raw(&mut self, index: usize, now: u64) -> Result<()> {
        let f = self.focus.as_mut().ok_or(LayoutError::NotInFocus)?;
        if index >= f.pages.len() {
            return Err(LayoutError::NoSuchPage(index));
        }
        if f.pages.len() == 1 {
            return self.restore(now).map(|_| ());
        }
        f.pages.remove(index);
        if f.current > index || f.current == f.pages.len() {
            f.current -= 1;
        }
        f.forget_off_page_editor();
        let current = f.current;
        self.go_to_page(current)
    }

    /// The work of [`Workspace::move_page`], not recorded for undo.
    pub(crate) fn move_page_raw(&mut self, from: usize, to: usize) -> Result<()> {
        let f = self.focus.as_mut().ok_or(LayoutError::NotInFocus)?;
        let n = f.pages.len();
        if from >= n {
            return Err(LayoutError::NoSuchPage(from));
        }
        if to >= n {
            return Err(LayoutError::NoSuchPage(to));
        }
        if from == to {
            return Err(LayoutError::NoOp);
        }
        let shown = f.current;
        let page = f.pages.remove(from);
        f.pages.insert(to, page);
        f.current = if shown == from {
            to
        } else if from < shown && to >= shown {
            shown - 1
        } else if from > shown && to <= shown {
            shown + 1
        } else {
            shown
        };
        Ok(())
    }

    /// The work of [`Workspace::move_pane_to_page`], not recorded for undo.
    pub(crate) fn move_pane_to_page_raw(&mut self, pane: PaneId, index: usize) -> Result<()> {
        self.transact(|ws| {
            ws.require(pane)?;
            let f = ws.focus.as_ref().ok_or(LayoutError::NotInFocus)?;
            if index >= f.pages.len() {
                return Err(LayoutError::NoSuchPage(index));
            }
            if f.page_of(pane) == Some(index) {
                return Err(LayoutError::NoOp);
            }
            let split_id = ws.alloc_node();
            let node = ws.new_group(vec![pane]);
            let f = ws.focus.as_mut().expect("in focus");
            f.remove(pane);
            let page = &mut f.pages[index];
            page.root = Some(match page.root.take() {
                None => node,
                Some(old) => old.append(Axis::Row, node, split_id),
            });
            if ws.focused == Some(pane) && index != f.current {
                ws.focused = f.first_active();
            }
            Ok(())
        })
    }

    /// Sends `pane` to the page after the current one, adding that page when the current page
    /// is the last. The page on screen does not change. One undo step; returns the page.
    pub fn move_pane_to_next_page(&mut self, pane: PaneId) -> Result<usize> {
        self.batch(|ws| {
            let f = ws.focus.as_ref().ok_or(LayoutError::NotInFocus)?;
            let (current, next) = (f.current, f.current + 1);
            let added = next == f.pages.len();
            if added {
                ws.new_page(None)?;
                ws.go_to_page(current)?;
                // A new page starts with the focused pane, which may be this one.
                if ws.focus.as_ref().and_then(|f| f.page_of(pane)) == Some(next) {
                    return Ok(next);
                }
            }
            ws.move_pane_to_page(pane, next)?;
            Ok(next)
        })
    }

    // ----- focus history ---------------------------------------------------------------------

    /// Focus history as the Reopen focus layout list shows it. `available` says whether a
    /// source that has no open pane can still be shown (the file exists, the investigation is
    /// loaded); pins never can.
    pub fn focus_history(
        &self,
        now: u64,
        available: impl Fn(&PaneSource) -> bool,
    ) -> Vec<HistoryEntry> {
        self.history
            .iter()
            .enumerate()
            .map(|(index, r)| HistoryEntry {
                index,
                label: r.label(),
                ago: history::ago(now, r.ended_at),
                pages: r.pages.len(),
                missing: r
                    .pages
                    .iter()
                    .enumerate()
                    .flat_map(|(i, p)| p.layout.sources().into_iter().map(move |s| (i, s)))
                    .filter(|(_, s)| !self.can_show(s, &available))
                    .map(|(i, s)| (i, s.clone()))
                    .collect(),
            })
            .collect()
    }

    /// The work of [`Workspace::reopen_focus`], not recorded for undo.
    pub(crate) fn reopen_focus_raw(
        &mut self,
        index: usize,
        now: u64,
        available: impl Fn(&PaneSource) -> bool,
    ) -> Result<ReopenReport> {
        let record = self
            .history
            .get(index)
            .cloned()
            .ok_or(LayoutError::NoSuchHistoryEntry(index))?;
        self.transact(|ws| {
            if ws.focus.is_some() {
                ws.restore(now)?;
            }
            let mut report = ReopenReport::default();
            let mut used = BTreeSet::new();
            let mut pages = Vec::new();
            let mut current = 0;
            for (i, page) in record.pages.iter().enumerate() {
                let mut missing = Vec::new();
                let built = ws.build_from_record(
                    &page.layout,
                    &available,
                    &mut used,
                    &mut report.created,
                    &mut missing,
                );
                report.missing.extend(missing.into_iter().map(|s| (i, s)));
                if let Some(root) = built.and_then(normalize) {
                    if i <= record.current {
                        current = pages.len();
                    }
                    pages.push(FocusPage {
                        root: Some(root),
                        origin: page.origin.clone(),
                    });
                }
            }
            if pages.is_empty() {
                return Err(LayoutError::NothingToReopen);
            }
            let all: Vec<PaneId> = pages
                .iter()
                .filter_map(|p| p.root.as_ref())
                .flat_map(Node::panes)
                .collect();
            let maximized = all
                .iter()
                .copied()
                .find(|p| ws.panes[p].source() == record.maximized)
                .unwrap_or(all[0]);
            let last_editor = all.iter().copied().find(|p| is_editor(ws.kind(*p)));
            ws.focus = Some(FocusSession {
                pages,
                current,
                maximized,
                last_editor,
                started_at: now,
            });
            ws.go_to_page(current)?;
            Ok(report)
        })
    }

    // ----- internals -------------------------------------------------------------------------

    /// Runs `f`, undoing everything it did if it fails.
    pub(crate) fn transact<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let before = self.clone();
        let out = f(self);
        if out.is_err() {
            *self = before;
        }
        out
    }

    pub(crate) fn require(&self, pane: PaneId) -> Result<()> {
        if self.panes.contains_key(&pane) {
            Ok(())
        } else {
            Err(LayoutError::UnknownPane(pane))
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub(crate) fn alloc_node(&mut self) -> NodeId {
        NodeId(self.alloc_id())
    }

    fn alloc_pane(&mut self, content: PaneContent) -> PaneId {
        let id = PaneId(self.alloc_id());
        self.panes.insert(id, content);
        id
    }

    pub(crate) fn new_group(&mut self, tabs: Vec<PaneId>) -> Node {
        Node::Group(Group::new(self.alloc_node(), tabs))
    }

    fn first_of_kind(&self, kind: PaneKind) -> Option<PaneId> {
        self.arrangement
            .panes()
            .into_iter()
            .find(|p| self.kind(*p) == Some(kind))
    }

    fn active_in(&self, group: NodeId) -> Option<PaneId> {
        self.group(group).and_then(Group::active_pane)
    }

    fn arrangement_group(&self, id: NodeId) -> Option<&Group> {
        Region::ALL
            .iter()
            .find_map(|r| self.arrangement.root(*r).as_ref().and_then(|n| n.group(id)))
    }

    /// Whether a group holds editor panes (groups never mix kinds outside focus).
    pub(crate) fn is_editor_group(&self, g: &Group) -> bool {
        is_editor(self.kind(g.tabs[0]))
    }

    fn note_editor(&mut self, pane: PaneId) {
        if !is_editor(self.kind(pane)) {
            return;
        }
        if let Some(f) = &mut self.focus
            && f.page_of(pane).is_some()
        {
            f.last_editor = Some(pane);
        }
    }

    fn can_show(&self, source: &PaneSource, available: &impl Fn(&PaneSource) -> bool) -> bool {
        self.panes.values().any(|c| c.source() == *source)
            || (source.to_content().is_some() && available(source))
    }

    /// A pane showing `src` that is not in `used`: an existing one, else a new one in its
    /// default place (added to `created`) when `available` says it can be shown.
    fn pane_for_source(
        &mut self,
        src: &PaneSource,
        available: &impl Fn(&PaneSource) -> bool,
        used: &BTreeSet<PaneId>,
        created: &mut Vec<PaneId>,
    ) -> Option<PaneId> {
        let existing = self
            .arrangement
            .panes()
            .into_iter()
            .find(|p| !used.contains(p) && self.panes[p].source() == *src);
        if existing.is_some() {
            return existing;
        }
        let content = src.to_content().filter(|_| available(src))?;
        let id = self.alloc_pane(content);
        self.place_default(id);
        created.push(id);
        Some(id)
    }

    fn build_from_record(
        &mut self,
        node: &RecordNode,
        available: &impl Fn(&PaneSource) -> bool,
        used: &mut BTreeSet<PaneId>,
        created: &mut Vec<PaneId>,
        missing: &mut Vec<PaneSource>,
    ) -> Option<Node> {
        match node {
            RecordNode::Group {
                tabs,
                active,
                minimized,
            } => {
                let mut ids = Vec::new();
                for src in tabs {
                    let Some(id) = self.pane_for_source(src, available, used, created) else {
                        missing.push(src.clone());
                        continue;
                    };
                    used.insert(id);
                    ids.push(id);
                }
                let mut g = Group::new(self.alloc_node(), ids);
                g.active = *active;
                g.minimized = *minimized;
                Some(Node::Group(g))
            }
            RecordNode::Split { axis, children } => {
                let id = self.alloc_node();
                let kids = children
                    .iter()
                    .filter_map(|c| {
                        self.build_from_record(&c.node, available, used, created, missing)
                            .map(|n| crate::tree::Child {
                                weight: c.weight,
                                node: n,
                            })
                    })
                    .collect();
                Some(Node::Split(crate::tree::Split {
                    id,
                    axis: *axis,
                    children: kids,
                }))
            }
        }
    }

    /// The editor group being read (the focused editor's), else the first editor group.
    pub(crate) fn current_editor_group(&self) -> Option<NodeId> {
        let editor = self.arrangement.editor.as_ref()?;
        self.focused
            .filter(|p| is_editor(self.kind(*p)))
            .and_then(|p| editor.group_of(p))
            .or_else(|| {
                editor
                    .groups()
                    .into_iter()
                    .find(|g| self.is_editor_group(g))
                    .map(|g| g.id)
            })
    }

    /// Keeps the editor area's rule that it holds at least one editor group whenever it holds
    /// anything: when the last editor closes, its tool groups go to their default places.
    pub(crate) fn settle_editor_area(&mut self) {
        let Some(root) = &self.arrangement.editor else {
            return;
        };
        if root.groups().iter().any(|g| self.is_editor_group(g)) {
            return;
        }
        let tools = root.panes();
        self.arrangement.editor = None;
        for p in tools {
            self.place_default(p);
        }
    }

    pub(crate) fn group_mut(&mut self, id: NodeId) -> Option<&mut Group> {
        match &mut self.focus {
            Some(f) => f.group_mut(id),
            None => {
                let region = self.arrangement.region_of_node(id)?;
                self.arrangement
                    .root_mut(region)
                    .as_mut()
                    .and_then(|n| n.group_mut(id))
            }
        }
    }

    pub(crate) fn split_mut(&mut self, id: NodeId) -> Option<&mut crate::tree::Split> {
        match &mut self.focus {
            Some(f) => f.split_mut(id),
            None => {
                let region = self.arrangement.region_of_node(id)?;
                self.arrangement
                    .root_mut(region)
                    .as_mut()
                    .and_then(|n| n.split_mut(id))
            }
        }
    }

    /// Pins follow every other tab: a pin's index is clamped to after the tool tabs, a tool
    /// tab's to before the pins.
    fn clamp_tab_index(&self, tabs: &[PaneId], pane: PaneId, index: usize) -> usize {
        let is_pin = |p: &PaneId| self.kind(*p).is_some_and(PaneKind::is_pin);
        let tools = tabs.iter().filter(|p| !is_pin(p)).count();
        let index = index.min(tabs.len());
        if is_pin(&pane) {
            index.max(tools)
        } else {
            index.min(tools)
        }
    }

    /// Inserts `pane` as a tab of `group`, in the normal arrangement (`on_focus` false) or the
    /// current focus page.
    fn insert_tab(
        &mut self,
        group: NodeId,
        pane: PaneId,
        index: usize,
        on_focus: bool,
    ) -> Result<()> {
        let found = if on_focus {
            self.focus.as_ref().and_then(|f| f.group(group))
        } else {
            self.arrangement_group(group)
        };
        let tabs = found.ok_or(LayoutError::UnknownGroup(group))?.tabs.clone();
        let at = self.clamp_tab_index(&tabs, pane, index);
        let g = if on_focus {
            self.focus.as_mut().and_then(|f| f.group_mut(group))
        } else {
            let region = self.arrangement.region_of_node(group);
            region.and_then(|r| {
                self.arrangement
                    .root_mut(r)
                    .as_mut()
                    .and_then(|n| n.group_mut(group))
            })
        }
        .expect("group just found");
        g.tabs.insert(at, pane);
        g.active = at;
        Ok(())
    }

    fn reorder(&mut self, group: NodeId, pane: PaneId, index: usize) -> Result<()> {
        let at = self
            .tab_drop_index(group, pane, index)
            .ok_or(LayoutError::UnknownGroup(group))?;
        let g = self.group_mut(group).expect("group exists");
        let from = g
            .tabs
            .iter()
            .position(|p| *p == pane)
            .expect("pane in group");
        if from == at {
            return Err(LayoutError::NoOp);
        }
        g.tabs.remove(from);
        g.tabs.insert(at, pane);
        g.active = at;
        Ok(())
    }

    fn detach_from_arrangement(&mut self, pane: PaneId) {
        for r in Region::ALL {
            let root = self.arrangement.root_mut(r);
            if let Some(n) = root
                && n.remove_pane(pane)
            {
                normalize_in_place(root);
            }
        }
    }

    fn show_region(&mut self, region: Region) {
        if let Region::Dock(d) = region {
            self.arrangement.dock_mut(d).visible = true;
        }
    }

    /// Checks a normal-arrangement target before anything is detached, for clear errors.
    /// Editor panes never go in a dock; tab strips never mix editor and tool tabs; a tool pane
    /// sent to the editor area needs an editor group to stand beside.
    pub(crate) fn check_target(&self, pane: PaneId, target: Target) -> Result<()> {
        let editor = is_editor(self.kind(pane));
        match target {
            Target::Dock(_) if editor => Err(LayoutError::EditorOutsideEditorArea),
            Target::Dock(_) => Ok(()),
            Target::EditorArea if !editor && self.current_editor_group().is_none() => {
                Err(LayoutError::NoEditorGroup)
            }
            Target::EditorArea => Ok(()),
            Target::Beside { group, .. } | Target::Tabs { group, .. } => {
                let g = self
                    .arrangement_group(group)
                    .ok_or(LayoutError::UnknownGroup(group))?;
                let region = self
                    .arrangement
                    .region_of_node(group)
                    .expect("group is placed");
                if editor && region != Region::Editor {
                    return Err(LayoutError::EditorOutsideEditorArea);
                }
                if matches!(target, Target::Tabs { .. }) && self.is_editor_group(g) != editor {
                    return Err(LayoutError::MixedKinds);
                }
                Ok(())
            }
            Target::AreaEdge(edge) => self.area_edge_check(pane, edge),
            Target::Tray => Err(LayoutError::NotInFocus),
        }
    }

    /// Places a detached pane in the normal arrangement.
    fn place(&mut self, pane: PaneId, target: Target) -> Result<()> {
        self.check_target(pane, target)?;
        match target {
            Target::Dock(d) => self.place_in_dock(d, pane),
            Target::EditorArea if is_editor(self.kind(pane)) => self.place_in_editor(pane),
            Target::EditorArea => {
                let group = self.current_editor_group().expect("checked");
                return self.place(
                    pane,
                    Target::Beside {
                        group,
                        edge: Edge::Right,
                    },
                );
            }
            Target::Beside { group, edge } => {
                let region = self.arrangement.region_of_node(group).expect("checked");
                let split_id = self.alloc_node();
                let node = self.new_group(vec![pane]);
                self.arrangement
                    .root_mut(region)
                    .as_mut()
                    .expect("checked")
                    .insert_beside(group, edge, node, split_id)
                    .map_err(|_| LayoutError::UnknownGroup(group))?;
                self.show_region(region);
            }
            Target::Tabs { group, index } => {
                self.insert_tab(group, pane, index, false)?;
                let region = self.arrangement.region_of_node(group).expect("checked");
                self.show_region(region);
            }
            Target::Tray => return Err(LayoutError::NotInFocus),
            Target::AreaEdge(edge) => {
                let split_id = self.alloc_node();
                let node = self.new_group(vec![pane]);
                let root = self.arrangement.root_mut(Region::Editor);
                *root = Some(match root.take() {
                    Some(old) => old.at_edge(edge, node, split_id, AREA_EDGE_SHARE),
                    None => node,
                });
            }
        }
        Ok(())
    }

    /// The tree a [`Target::AreaEdge`] spans: the current focus page, or the editor area.
    fn area_root(&self) -> Option<&Node> {
        match &self.focus {
            Some(f) => f.root(),
            None => self.arrangement.root(Region::Editor).as_ref(),
        }
    }

    /// Whether `pane` can go along the whole of `edge` of the area: there must be something
    /// to span, and the pane must not already be all there is, or alone along that edge.
    pub(crate) fn area_edge_check(&self, pane: PaneId, edge: Edge) -> Result<()> {
        let alone = |n: &Node| matches!(n, Node::Group(g) if g.tabs == [pane]);
        match self.area_root() {
            None => Err(LayoutError::NoEditorGroup),
            Some(root) if alone(root) => Err(LayoutError::NoOp),
            Some(Node::Split(s)) if s.end(edge).is_some_and(|c| alone(&c.node)) => {
                Err(LayoutError::NoOp)
            }
            Some(_) => Ok(()),
        }
    }

    /// One group already runs along the whole of `edge` of the area (the code area, or the
    /// focus page), so a drop beside that group does what [`Target::AreaEdge`] would. The UI
    /// then offers only the drop beside the group.
    pub fn area_edge_is_one_group(&self, edge: Edge) -> bool {
        match self.area_root() {
            Some(Node::Group(_)) => true,
            Some(Node::Split(s)) => s
                .end(edge)
                .is_some_and(|c| matches!(c.node, Node::Group(_))),
            None => false,
        }
    }

    pub(crate) fn place_default(&mut self, pane: PaneId) {
        match self.kind(pane).expect("pane exists").default_place() {
            DefaultPlace::Dock(d) => self.place_in_dock(d, pane),
            DefaultPlace::EditorArea => self.place_in_editor(pane),
        }
    }

    /// A side dock stacks the pane at its bottom; the bottom bar adds it as a tab.
    fn place_in_dock(&mut self, dock: DockId, pane: PaneId) {
        let first = self
            .arrangement
            .dock(dock)
            .root
            .as_ref()
            .map(|n| n.first_group().id);
        match (dock, first) {
            (_, None) => {
                let node = self.new_group(vec![pane]);
                self.arrangement.dock_mut(dock).root = Some(node);
            }
            (DockId::Bottom, Some(group)) => {
                self.insert_tab(group, pane, usize::MAX, false)
                    .expect("first group exists");
            }
            (DockId::Left | DockId::Right, Some(_)) => {
                let split_id = self.alloc_node();
                let node = self.new_group(vec![pane]);
                let d = self.arrangement.dock_mut(dock);
                let old = d.root.take().expect("dock has a root");
                d.root = Some(old.append(Axis::Column, node, split_id));
            }
        }
        self.arrangement.dock_mut(dock).visible = true;
    }

    /// An editor pane goes into the current editor group, right after its active tab.
    fn place_in_editor(&mut self, pane: PaneId) {
        match self.current_editor_group() {
            None => {
                let node = self.new_group(vec![pane]);
                self.arrangement.editor = match self.arrangement.editor.take() {
                    None => Some(node),
                    // Only tool groups are left (the last editor is being moved within the
                    // editor area): the editor goes on the left. Normalising flattens the row.
                    Some(old) => {
                        let id = self.alloc_node();
                        normalize(Node::Split(crate::tree::Split {
                            id,
                            axis: Axis::Row,
                            children: vec![
                                crate::tree::Child { weight: 0.5, node },
                                crate::tree::Child {
                                    weight: 0.5,
                                    node: old,
                                },
                            ],
                        }))
                    }
                };
            }
            Some(group) => {
                let g = self
                    .arrangement
                    .editor
                    .as_mut()
                    .and_then(|n| n.group_mut(group))
                    .expect("editor group exists");
                let at = g.active + 1;
                g.tabs.insert(at, pane);
                g.active = at;
            }
        }
    }

    // focus-layout placement

    /// Places a pane [`Workspace::validate`] accepted on the current page, or in the tray.
    fn move_on_focus(&mut self, pane: PaneId, target: Target) -> Result<()> {
        let f = self.focus.as_mut().expect("in focus");
        f.remove(pane);
        if target == Target::Tray {
            f.forget_off_page_editor();
            if self.focused == Some(pane) {
                self.focused = f.first_active();
            }
            return Ok(());
        }
        self.place_on_focus(pane, target)
    }

    fn place_on_focus(&mut self, pane: PaneId, target: Target) -> Result<()> {
        match target {
            Target::Beside { group, edge } => {
                let split_id = self.alloc_node();
                let node = self.new_group(vec![pane]);
                let f = self.focus.as_mut().expect("in focus");
                f.root_mut()
                    .as_mut()
                    .ok_or(LayoutError::UnknownGroup(group))?
                    .insert_beside(group, edge, node, split_id)
                    .map_err(|_| LayoutError::UnknownGroup(group))?;
            }
            Target::Tabs { group, index } => self.insert_tab(group, pane, index, true)?,
            Target::AreaEdge(edge) => {
                let split_id = self.alloc_node();
                let node = self.new_group(vec![pane]);
                let root = self.focus.as_mut().expect("in focus").root_mut();
                *root = Some(match root.take() {
                    Some(old) => old.at_edge(edge, node, split_id, AREA_EDGE_SHARE),
                    None => node,
                });
            }
            _ => return Err(LayoutError::DocksHiddenInFocus),
        }
        self.note_editor(pane);
        Ok(())
    }

    /// The work of [`Workspace::place_on_empty_page`], not recorded for undo.
    pub(crate) fn place_on_empty_page_raw(&mut self, pane: PaneId) -> Result<()> {
        self.transact(|ws| {
            ws.require(pane)?;
            let f = ws.focus.as_ref().ok_or(LayoutError::NotInFocus)?;
            if f.root().is_some() {
                return Err(LayoutError::NoOp);
            }
            let node = ws.new_group(vec![pane]);
            let f = ws.focus.as_mut().expect("in focus");
            f.remove(pane);
            *f.root_mut() = Some(node);
            ws.note_editor(pane);
            Ok(())
        })
    }
}

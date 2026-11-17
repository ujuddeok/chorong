//! What the keyboard needs from the model (design/layout-keyboard.md, section 4): spatial
//! neighbours, directional focus and moves, valid destinations, keyboard resize and
//! equalise, numbered editor groups and jump labels, toggling every dock, and the
//! most-recently-used order.
//!
//! The model never measures. The viewer passes each group's rectangle from its last layout
//! pass; every answer here is a pure function of those rectangles and the workspace, with
//! deterministic tie-breaks.

use crate::arrangement::{DockId, Region};
use crate::tree::{Axis, Edge, Node, NodeId};
use crate::workspace::{Result, Target};
use crate::{LayoutError, PaneId, PaneKind, Workspace};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const ALL: [Direction; 4] = [
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ];

    pub fn axis(self) -> Axis {
        match self {
            Direction::Left | Direction::Right => Axis::Row,
            Direction::Up | Direction::Down => Axis::Column,
        }
    }

    pub fn edge(self) -> Edge {
        match self {
            Direction::Left => Edge::Left,
            Direction::Right => Edge::Right,
            Direction::Up => Edge::Top,
            Direction::Down => Edge::Bottom,
        }
    }

    pub fn opposite(self) -> Direction {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        }
    }

    /// Towards the end of its axis (right or down).
    pub fn is_forward(self) -> bool {
        matches!(self, Direction::Right | Direction::Down)
    }

    /// The dock at the window edge this way. There is no top dock.
    pub fn dock(self) -> Option<DockId> {
        match self {
            Direction::Left => Some(DockId::Left),
            Direction::Right => Some(DockId::Right),
            Direction::Down => Some(DockId::Bottom),
            Direction::Up => None,
        }
    }
}

/// A rectangle in logical pixels, as the viewer measured a group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    fn right(&self) -> f32 {
        self.x + self.w
    }
    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Edges closer than this count as touching (handles and borders sit between groups).
const SLACK: f32 = 1.0;

/// The nearest group from `from` in direction `dir` among `rects` that `ok` accepts. A
/// candidate must lie entirely beyond `from`'s edge that way and overlap it on the other axis.
/// Ties break by the smallest gap, then the largest overlap, then `rank` (lower is more
/// recently used), then reading order (the order of `rects`). Linear in `rects`.
pub fn neighbour(
    rects: &[(NodeId, Rect)],
    from: NodeId,
    dir: Direction,
    ok: impl Fn(NodeId) -> bool,
    rank: impl Fn(NodeId) -> usize,
) -> Option<NodeId> {
    let f = rects.iter().find(|(id, _)| *id == from)?.1;
    let mut best: Option<((i64, i64, usize, usize), NodeId)> = None;
    for (i, (id, r)) in rects.iter().enumerate() {
        if *id == from || !ok(*id) {
            continue;
        }
        let (gap, overlap) = match dir {
            Direction::Right => (r.x - f.right(), f.bottom().min(r.bottom()) - f.y.max(r.y)),
            Direction::Left => (f.x - r.right(), f.bottom().min(r.bottom()) - f.y.max(r.y)),
            Direction::Down => (r.y - f.bottom(), f.right().min(r.right()) - f.x.max(r.x)),
            Direction::Up => (f.y - r.bottom(), f.right().min(r.right()) - f.x.max(r.x)),
        };
        if gap < -SLACK || overlap <= SLACK {
            continue;
        }
        let key = (
            gap.max(0.0).round() as i64,
            -(overlap.round() as i64),
            rank(*id),
            i,
        );
        if best.as_ref().is_none_or(|(k, _)| key < *k) {
            best = Some((key, *id));
        }
    }
    best.map(|(_, id)| id)
}

/// How far one keyboard resize step goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeStep {
    /// Share of a split.
    pub share: f32,
    /// Logical pixels, for a dock.
    pub px: f32,
}

impl ResizeStep {
    /// Arrow keys in the resize sub-mode: 5% of a split, 24 px of a dock.
    pub const NORMAL: ResizeStep = ResizeStep {
        share: 0.05,
        px: 24.0,
    };
    /// Shift-arrows: 1%, 4 px.
    pub const FINE: ResizeStep = ResizeStep {
        share: 0.01,
        px: 4.0,
    };
}

/// Home-row letters for jump labels, skipping the mode's command keys (layout-keyboard.md).
pub const LABEL_LETTERS: [char; 15] = [
    'a', 'd', 'f', 'g', 'j', 'k', 'l', ';', 'e', 'i', 'o', 'q', 'c', 'z', 'y',
];

/// A jump label on a group: editor groups are numbered 1 to 9, others get letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Label {
    Number(u8),
    Letter(char),
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Label::Number(n) => write!(f, "{n}"),
            Label::Letter(c) => write!(f, "{c}"),
        }
    }
}

impl Workspace {
    // ----- most recently used ----------------------------------------------------------------

    /// Panes in most-recently-focused order (activation moves a pane to the front). Saved with
    /// the layout; feeds the switcher and neighbour tie-breaks.
    pub fn recent_panes(&self) -> &[PaneId] {
        &self.recent
    }

    pub(crate) fn touch(&mut self, pane: PaneId) {
        self.recent.retain(|p| *p != pane);
        self.recent.insert(0, pane);
    }

    /// A group's recency: the position of its most recent pane (`usize::MAX` if none).
    fn group_rank(&self, group: NodeId) -> usize {
        self.group(group).map_or(usize::MAX, |g| {
            g.tabs
                .iter()
                .filter_map(|p| self.recent.iter().position(|r| r == p))
                .min()
                .unwrap_or(usize::MAX)
        })
    }

    /// [`neighbour`] over groups on screen, breaking ties by recency.
    pub fn neighbour(
        &self,
        rects: &[(NodeId, Rect)],
        from: NodeId,
        dir: Direction,
    ) -> Option<NodeId> {
        neighbour(
            rects,
            from,
            dir,
            |id| self.group(id).is_some(),
            |id| self.group_rank(id),
        )
    }

    // ----- focus -----------------------------------------------------------------------------

    /// Focuses the active pane of `group`.
    pub fn focus_group(&mut self, group: NodeId) -> Result<PaneId> {
        let pane = self
            .group(group)
            .and_then(|g| g.active_pane())
            .ok_or(LayoutError::UnknownGroup(group))?;
        self.activate(pane)?;
        Ok(pane)
    }

    /// Focuses the neighbour of the focused pane's group that way (Cmd-K Cmd-arrows, arrows in
    /// Layout mode). Crosses region boundaries; hidden docks have no rectangles, so they are
    /// skipped.
    pub fn focus_dir(&mut self, dir: Direction, rects: &[(NodeId, Rect)]) -> Result<PaneId> {
        let from = self
            .focused
            .and_then(|p| self.group_of(p))
            .ok_or(LayoutError::NothingThatWay)?;
        let to = self
            .neighbour(rects, from, dir)
            .ok_or(LayoutError::NothingThatWay)?;
        self.focus_group(to)
    }

    /// Editor groups on screen in reading order: the numbers of Cmd-1…9.
    pub fn editor_groups(&self) -> Vec<NodeId> {
        self.screen_groups()
            .into_iter()
            .filter(|(_, editor)| *editor)
            .map(|(id, _)| id)
            .collect()
    }

    /// Focuses editor group `n`, counted from 1.
    pub fn focus_editor_group(&mut self, n: usize) -> Result<PaneId> {
        let group = *self
            .editor_groups()
            .get(n.wrapping_sub(1))
            .ok_or(LayoutError::NothingThatWay)?;
        self.focus_group(group)
    }

    /// Jump labels for every group on screen: editor groups numbered 1 to 9 in reading order,
    /// then other groups lettered from `alphabet` in reading order (left dock, editor area,
    /// right dock, bottom bar; a focus page in its own reading order). Groups past nine
    /// numbers or the alphabet get no label. Same input, same labels.
    pub fn labels(&self, alphabet: &[char]) -> Vec<(Label, NodeId)> {
        let mut out = Vec::new();
        let mut numbers = 1u8..=9;
        let mut letters = alphabet.iter();
        let groups = self.screen_groups();
        for (id, editor) in &groups {
            if *editor && let Some(n) = numbers.next() {
                out.push((Label::Number(n), *id));
            }
        }
        for (id, editor) in &groups {
            if !*editor && let Some(c) = letters.next() {
                out.push((Label::Letter(*c), *id));
            }
        }
        out
    }

    /// Jump labels for the tray's panes in a focus layout, carrying on from `screen` (the
    /// labels [`Self::labels`] gave the groups on screen): code panes take the numbers left,
    /// others the letters left, in tray order. Empty outside focus or once both run out.
    pub fn tray_labels(
        &self,
        alphabet: &[char],
        screen: &[(Label, NodeId)],
    ) -> Vec<(Label, PaneId)> {
        let used_numbers = screen
            .iter()
            .filter(|(l, _)| matches!(l, Label::Number(_)))
            .count() as u8;
        let used_letters = screen
            .iter()
            .filter(|(l, _)| matches!(l, Label::Letter(_)))
            .count();
        let mut numbers = (used_numbers + 1)..=9;
        let mut letters = alphabet.iter().skip(used_letters);
        self.tray()
            .into_iter()
            .filter_map(|p| {
                let label = if self.kind(p) == Some(PaneKind::Editor) {
                    Label::Number(numbers.next()?)
                } else {
                    Label::Letter(*letters.next()?)
                };
                Some((label, p))
            })
            .collect()
    }

    /// Groups on screen in reading order, each with whether it holds editors. Hidden docks
    /// are left out.
    fn screen_groups(&self) -> Vec<(NodeId, bool)> {
        self.screen_roots()
            .into_iter()
            .flat_map(Node::groups)
            .map(|g| {
                (
                    g.id,
                    g.tabs
                        .iter()
                        .any(|p| self.kind(*p) == Some(PaneKind::Editor)),
                )
            })
            .collect()
    }

    /// Every group on screen, in reading order (hidden docks left out).
    pub fn screen_group_ids(&self) -> Vec<NodeId> {
        self.screen_roots()
            .into_iter()
            .flat_map(Node::groups)
            .map(|g| g.id)
            .collect()
    }

    /// The trees on screen in reading order: the current focus page, or the regions of the
    /// normal arrangement that are shown (hidden docks are left out).
    pub(crate) fn screen_roots(&self) -> Vec<&Node> {
        match &self.focus {
            Some(f) => f.root().into_iter().collect(),
            None => Region::ALL
                .iter()
                .filter(|r| match r {
                    Region::Dock(d) => self.arrangement.dock(*d).visible,
                    Region::Editor => true,
                })
                .filter_map(|r| self.arrangement.root(*r).as_ref())
                .collect(),
        }
    }

    // ----- validation ------------------------------------------------------------------------

    /// Whether moving `pane` to `target` would be accepted, without changing anything and
    /// without copying the workspace. This is the one set of move rules:
    /// [`Workspace::move_pane`] checks it first and then only places the pane.
    ///
    /// - On the pane's own group: beside it needs another tab left behind, and a tab position
    ///   is a reorder that must change the order.
    /// - In a focus layout: the docks and the editor area are hidden; the tray takes panes
    ///   that are on a page; any group on the current page takes any pane, beside or as a tab.
    /// - Otherwise (`check_target`): editors stay in the editor area and tab strips
    ///   never mix editors and tools.
    /// - Along a whole edge ([`Target::AreaEdge`]), there must be something to span that the
    ///   pane is not alone along already.
    pub fn validate(&self, pane: PaneId, target: Target) -> Result<()> {
        self.require(pane)?;
        self.validate_from(pane, self.group_of(pane), target)
    }

    /// [`Workspace::validate`] for a known pane in group `from` (on screen), so a caller
    /// trying many targets finds the group once.
    fn validate_from(&self, pane: PaneId, from: Option<NodeId>, target: Target) -> Result<()> {
        if let Target::Beside { group, .. } | Target::Tabs { group, .. } = target
            && from == Some(group)
        {
            return self.own_group_check(pane, group, target);
        }
        let Some(f) = &self.focus else {
            return self.check_target(pane, target);
        };
        match target {
            Target::Dock(_) | Target::EditorArea => Err(LayoutError::DocksHiddenInFocus),
            Target::Tray if f.page_of(pane).is_none() => Err(LayoutError::NoOp),
            Target::Tray => Ok(()),
            Target::Beside { group, .. } | Target::Tabs { group, .. } => f
                .group(group)
                .map(|_| ())
                .ok_or(LayoutError::UnknownGroup(group)),
            Target::AreaEdge(edge) => self.area_edge_check(pane, edge),
        }
    }

    /// A move of `pane` beside or into `group`, the group it is in.
    fn own_group_check(&self, pane: PaneId, group: NodeId, target: Target) -> Result<()> {
        let tabs = self
            .group(group)
            .map(|g| g.tabs.as_slice())
            .unwrap_or_default();
        match target {
            Target::Beside { .. } if tabs.len() < 2 => Err(LayoutError::NoOp),
            Target::Tabs { index, .. } => {
                let now = tabs.iter().position(|p| *p == pane);
                if self.tab_drop_index(group, pane, index) == now {
                    Err(LayoutError::NoOp)
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }

    /// Every place `pane` could be moved to now, in a fixed order: the docks, the editor area,
    /// then for each group on screen beside it (left, right, top, bottom) and onto its tab
    /// strip, then along each edge of the area, then the tray. Feeds Move pane to…. Each is a
    /// target [`Workspace::validate`] accepts.
    pub fn valid_targets(&self, pane: PaneId) -> Vec<Target> {
        let mut out: Vec<Target> = DockId::ALL.map(Target::Dock).into();
        out.push(Target::EditorArea);
        for (group, _) in self.screen_groups() {
            out.extend(Edge::ALL.map(|edge| Target::Beside { group, edge }));
            out.push(Target::Tabs {
                group,
                index: usize::MAX,
            });
        }
        out.extend(Edge::ALL.map(Target::AreaEdge));
        out.push(Target::Tray);
        if self.require(pane).is_err() {
            return Vec::new();
        }
        let from = self.group_of(pane);
        out.retain(|t| self.validate_from(pane, from, *t).is_ok());
        out
    }

    // ----- directional moves -----------------------------------------------------------------

    pub(crate) fn region_of_group(&self, group: NodeId) -> Option<Region> {
        match &self.focus {
            Some(_) => Some(Region::Editor),
            None => self.arrangement.region_of_node(group),
        }
    }

    /// Where a directional move of `pane` (or of its whole group) would go:
    ///
    /// - onto the next group that way, as its last tab, when that group may hold it;
    /// - a tool pane meeting the editor area from a dock enters it, as a new group beside the
    ///   first group it meets; inside a region, groups that cannot hold it are passed over;
    /// - at the window edge, a tool pane goes to that side's dock (there is no top dock);
    /// - an editor pane that meets a dock or the window edge makes a new group at that edge
    ///   of the editor area (when its group has another tab to leave behind);
    /// - a pane already at the edge, with nothing further that way, goes along the whole of
    ///   that edge of the editor area or focus page ([`Target::AreaEdge`]).
    fn dir_target(
        &self,
        pane: PaneId,
        whole_group: bool,
        dir: Direction,
        rects: &[(NodeId, Rect)],
    ) -> Result<Target> {
        let own = self.group_of(pane).ok_or(LayoutError::NotOnScreen(pane))?;
        let own_region = self.region_of_group(own);
        let editor = self.kind(pane) == Some(PaneKind::Editor);
        let check = |t: Target| -> bool {
            if whole_group {
                // A group moves as its first pane would, except onto its own strip.
                !matches!(t, Target::Beside { group, .. } | Target::Tabs { group, .. } if group == own)
                    && self.validate(pane, t).is_ok()
            } else {
                self.validate(pane, t).is_ok()
            }
        };
        let mut cur = own;
        let mut seen = 0;
        while let Some(n) = self.neighbour(rects, cur, dir) {
            seen += 1;
            if seen > rects.len() {
                break;
            }
            let tabs = Target::Tabs {
                group: n,
                index: usize::MAX,
            };
            if check(tabs) {
                return Ok(tabs);
            }
            let n_region = self.region_of_group(n);
            if n_region != own_region {
                let beside = Target::Beside {
                    group: n,
                    edge: dir.opposite().edge(),
                };
                if !editor && check(beside) {
                    return Ok(beside);
                }
                if editor {
                    // An editor meeting a dock: a new group at the editor area's edge.
                    break;
                }
            }
            cur = n;
        }
        if !editor
            && self.focus.is_none()
            && let Some(d) = dir.dock()
            && own_region != Some(Region::Dock(d))
            && check(Target::Dock(d))
        {
            return Ok(Target::Dock(d));
        }
        let edge = Target::Beside {
            group: cur,
            edge: dir.edge(),
        };
        if editor && !whole_group && check(edge) {
            // Leaves its group's other tabs behind.
            if cur != own || self.group(own).is_some_and(|g| g.tabs.len() > 1) {
                return Ok(edge);
            }
        }
        // Already at the edge with nothing further that way: the next press spans the whole
        // edge of the code area (or page), under or beside every group there.
        let span = Target::AreaEdge(dir.edge());
        if cur == own && own_region == Some(Region::Editor) && check(span) {
            return Ok(span);
        }
        Err(LayoutError::NothingThatWay)
    }

    /// Where Shift-arrows (or, with `whole_group`, Alt-arrows) in Layout mode would move
    /// `pane` (or its group) each way, for the mode's preview. Ways with nowhere to go are
    /// left out.
    pub fn dir_targets(
        &self,
        pane: PaneId,
        whole_group: bool,
        rects: &[(NodeId, Rect)],
    ) -> Vec<(Direction, Target)> {
        let pane = if whole_group {
            // A group moves as its first tab would (`move_group_dir`).
            match self.group_of(pane).and_then(|g| self.group(g)) {
                Some(g) => g.tabs[0],
                None => return Vec::new(),
            }
        } else {
            pane
        };
        Direction::ALL
            .into_iter()
            .filter_map(|d| Some((d, self.dir_target(pane, whole_group, d, rects).ok()?)))
            .collect()
    }

    /// Shift-arrows in Layout mode: moves the pane one step that way. Returns where it went.
    pub fn move_pane_dir(
        &mut self,
        pane: PaneId,
        dir: Direction,
        rects: &[(NodeId, Rect)],
    ) -> Result<Target> {
        let t = self.dir_target(pane, false, dir, rects)?;
        self.move_pane(pane, t)?;
        Ok(t)
    }

    /// Alt-arrows in Layout mode: moves the whole group one step that way, keeping its tabs.
    pub fn move_group_dir(
        &mut self,
        group: NodeId,
        dir: Direction,
        rects: &[(NodeId, Rect)],
    ) -> Result<Target> {
        let first = self
            .group(group)
            .map(|g| g.tabs[0])
            .ok_or(LayoutError::UnknownGroup(group))?;
        let t = self.dir_target(first, true, dir, rects)?;
        self.move_group(group, t)?;
        Ok(t)
    }

    // ----- keyboard resize -------------------------------------------------------------------

    /// One arrow in the resize sub-mode: the pane's edge that way moves that way by `step`. If
    /// the pane has no movable edge that way, its opposite edge moves that way instead (it
    /// shrinks), so every arrow does something visible. Splits are searched from the pane
    /// outwards; past them, a dock pane resizes its dock and an editor-area pane resizes the
    /// dock on that side. Returns the change applied (a share, or pixels for a dock).
    pub fn resize_pane(&mut self, pane: PaneId, dir: Direction, step: ResizeStep) -> Result<f32> {
        let group = self.group_of(pane).ok_or(LayoutError::NotOnScreen(pane))?;
        let root = self
            .root_holding(group)
            .cloned()
            .ok_or(LayoutError::NotOnScreen(pane))?;
        let path = root.path_to(group).unwrap_or_default();
        let along: Vec<(NodeId, usize, usize)> = path
            .iter()
            .rev()
            .filter_map(|(split, i)| match root.split(*split) {
                Some(s) if s.axis == dir.axis() => Some((*split, *i, s.children.len())),
                _ => None,
            })
            .collect();
        let signed = if dir.is_forward() {
            step.share
        } else {
            -step.share
        };
        // The edge that way: a sibling on that side somewhere up the tree.
        for (split, i, n) in &along {
            let handle = if dir.is_forward() {
                (i + 1 < *n).then_some(*i)
            } else {
                (*i > 0).then(|| i - 1)
            };
            if let Some(h) = handle {
                return self.resize_split(*split, h, signed);
            }
        }
        // The region's edge that way: a dock's own edge, or the dock beside the editor area.
        let region = match self.focus {
            Some(_) => None,
            None => self.arrangement.region_of_node(group),
        };
        let that_way = match (region, dir) {
            (Some(Region::Dock(DockId::Left)), Direction::Right) => Some((DockId::Left, true)),
            (Some(Region::Dock(DockId::Right)), Direction::Left) => Some((DockId::Right, true)),
            (Some(Region::Dock(DockId::Bottom)), Direction::Up) => Some((DockId::Bottom, true)),
            (Some(Region::Editor), Direction::Left) => Some((DockId::Left, false)),
            (Some(Region::Editor), Direction::Right) => Some((DockId::Right, false)),
            (Some(Region::Editor), Direction::Down) => Some((DockId::Bottom, false)),
            _ => None,
        };
        if let Some((dock, grow)) = that_way
            && let Some(delta) = self.step_dock(dock, grow, step)
        {
            return delta;
        }
        // Otherwise the opposite edge moves that way: splits innermost first, then the region.
        for (split, i, n) in &along {
            let handle = if dir.is_forward() {
                (*i > 0).then(|| i - 1)
            } else {
                (i + 1 < *n).then_some(*i)
            };
            if let Some(h) = handle {
                return self.resize_split(*split, h, signed);
            }
        }
        let opposite = match (region, dir) {
            (Some(Region::Dock(DockId::Left)), Direction::Left) => Some((DockId::Left, false)),
            (Some(Region::Dock(DockId::Right)), Direction::Right) => Some((DockId::Right, false)),
            (Some(Region::Dock(DockId::Bottom)), Direction::Down) => Some((DockId::Bottom, false)),
            (Some(Region::Editor), Direction::Up) => Some((DockId::Bottom, true)),
            _ => None,
        };
        opposite
            .and_then(|(dock, grow)| self.step_dock(dock, grow, step))
            .unwrap_or(Err(LayoutError::NothingThatWay))
    }

    /// The tree on screen that holds `group`: the focus page, or its region's.
    fn root_holding(&self, group: NodeId) -> Option<&Node> {
        match &self.focus {
            Some(f) => f.root(),
            None => self
                .arrangement
                .region_of_node(group)
                .and_then(|r| self.arrangement.root(r).as_ref()),
        }
    }

    /// Grows or shrinks a shown dock by one step; `None` if the dock is hidden or empty.
    fn step_dock(&mut self, dock: DockId, grow: bool, step: ResizeStep) -> Option<Result<f32>> {
        let d = self.arrangement.dock(dock);
        if !d.visible || d.root.is_none() {
            return None;
        }
        let old = d.size;
        let px = if grow { step.px } else { -step.px };
        Some(self.resize_dock(dock, old + px).map(|new| new - old))
    }

    /// Gives every child of `split` the same share. Recorded for undo.
    pub fn equalize(&mut self, split: NodeId) -> Result<()> {
        self.recorded(None, |ws| {
            let s = ws
                .split_mut(split)
                .ok_or(LayoutError::UnknownSplit(split))?;
            let w = 1.0 / s.children.len() as f32;
            s.children.iter_mut().for_each(|c| c.weight = w);
            Ok(())
        })
    }

    /// `=` in Layout mode: equalises the split holding the pane's group (the innermost).
    pub fn equalize_around(&mut self, pane: PaneId) -> Result<()> {
        let group = self.group_of(pane).ok_or(LayoutError::NotOnScreen(pane))?;
        let split = self
            .root_holding(group)
            .and_then(|r| r.path_to(group))
            .and_then(|p| p.last().map(|(s, _)| *s))
            .ok_or(LayoutError::NoOp)?;
        self.equalize(split)
    }

    // ----- docks -----------------------------------------------------------------------------

    /// Cmd-Alt-Y: hides every dock, or, if none is visible, shows the ones that were visible
    /// before (all of them if nothing was remembered). Returns whether docks are now shown.
    pub fn toggle_all_docks(&mut self) -> Result<bool> {
        if self.focus.is_some() {
            return Err(LayoutError::DocksHiddenInFocus);
        }
        self.recorded(None, |ws| {
            let visible = DockId::ALL.map(|d| ws.arrangement.dock(d).visible);
            if visible.iter().any(|v| *v) {
                ws.all_docks = Some(visible);
                for d in DockId::ALL {
                    ws.arrangement.dock_mut(d).visible = false;
                }
                Ok(false)
            } else {
                let restore = ws.all_docks.take().unwrap_or([true; 3]);
                for (d, v) in DockId::ALL.into_iter().zip(restore) {
                    ws.arrangement.dock_mut(d).visible = v;
                }
                Ok(true)
            }
        })
    }
}

impl Workspace {
    /// An approximate rectangle for every group on screen in a `width` × `height` window,
    /// from dock sizes and split shares alone (no headers, handles or minimised folding). For
    /// tests and for callers without measurements; the viewer passes what it measured.
    pub fn rects(&self, width: f32, height: f32) -> Vec<(NodeId, Rect)> {
        fn walk(node: &Node, r: Rect, out: &mut Vec<(NodeId, Rect)>) {
            match node {
                Node::Group(g) => out.push((g.id, r)),
                Node::Split(s) => {
                    let mut at = 0.0;
                    for c in &s.children {
                        let part = match s.axis {
                            Axis::Row => Rect::new(r.x + at * r.w, r.y, c.weight * r.w, r.h),
                            Axis::Column => Rect::new(r.x, r.y + at * r.h, r.w, c.weight * r.h),
                        };
                        at += c.weight;
                        walk(&c.node, part, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        if let Some(f) = &self.focus {
            if let Some(root) = f.root() {
                walk(root, Rect::new(0.0, 0.0, width, height), &mut out);
            }
            return out;
        }
        let shown = |d: DockId| {
            let dock = self.arrangement.dock(d);
            (dock.visible && dock.root.is_some()).then_some(dock.size)
        };
        let bottom = shown(DockId::Bottom).unwrap_or(0.0).min(height);
        let top_h = height - bottom;
        let left = shown(DockId::Left).unwrap_or(0.0);
        let right = shown(DockId::Right).unwrap_or(0.0);
        let mid = (width - left - right).max(0.0);
        let place = |region: Region, r: Rect, out: &mut Vec<(NodeId, Rect)>| {
            if let Some(root) = self.arrangement.root(region) {
                walk(root, r, out);
            }
        };
        if left > 0.0 {
            place(
                Region::Dock(DockId::Left),
                Rect::new(0.0, 0.0, left, top_h),
                &mut out,
            );
        }
        place(Region::Editor, Rect::new(left, 0.0, mid, top_h), &mut out);
        if right > 0.0 {
            place(
                Region::Dock(DockId::Right),
                Rect::new(left + mid, 0.0, right, top_h),
                &mut out,
            );
        }
        if bottom > 0.0 {
            place(
                Region::Dock(DockId::Bottom),
                Rect::new(0.0, top_h, width, bottom),
                &mut out,
            );
        }
        out
    }
}

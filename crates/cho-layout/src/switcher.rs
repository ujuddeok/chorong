//! What the pickers and the Ctrl-Tab switcher need from the model (design/layout-keyboard.md,
//! "Focusing panes", "The command palette's role"): every pane with its place, revealing a pane
//! wherever it is, the destinations of Move pane to…, bringing a pane in from the tray, the
//! recency order and its stepping, and keyboard dock sizing.
//!
//! All of it is pure and deterministic; the shell only draws the answers.

use crate::arrangement::{DockId, Region};
use crate::tree::{Edge, NodeId};
use crate::workspace::{Result, Target};
use crate::{LayoutError, PaneId, PaneKind, Workspace};

/// How far one Ctrl-Alt-= / Ctrl-Alt-- press changes a dock (layout-keyboard.md, "Splitting,
/// resizing, minimise and maximise"): one keyboard resize step of a dock, 24 px.
pub const DOCK_STEP: f32 = crate::ResizeStep::NORMAL.px;

/// Where a pane is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The editor area.
    Editor,
    Dock(DockId),
    /// A focus page, counted from 0.
    Page(usize),
    /// Waiting off screen while a focus layout is open.
    Tray,
}

/// One row of Focus pane… and of the switcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneEntry {
    pub pane: PaneId,
    pub title: String,
    pub kind: PaneKind,
    pub place: Place,
    /// In a dock that is hidden.
    pub hidden: bool,
    /// In a group folded to its header.
    pub minimized: bool,
    pub focused: bool,
}

impl PaneEntry {
    /// The place as a list shows it: `Right dock`, `Editor area`, `Tray`, `Page 2`, with
    /// `, hidden` or `, minimised` after it when that applies.
    pub fn place_label(&self) -> String {
        let mut s = place_name(self.place);
        if self.hidden {
            s.push_str(", hidden");
        }
        if self.minimized {
            s.push_str(", minimised");
        }
        s
    }
}

fn place_name(place: Place) -> String {
    match place {
        Place::Editor => "Code area".into(),
        Place::Dock(DockId::Left) => "Left dock".into(),
        Place::Dock(DockId::Right) => "Right dock".into(),
        Place::Dock(DockId::Bottom) => "Bottom bar".into(),
        Place::Page(i) => format!("Page {}", i + 1),
        Place::Tray => "Tray".into(),
    }
}

/// One row of Move pane to…: a place the pane could go now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    pub label: String,
    /// The region or page the target is in, for telling same-named groups apart.
    pub place: String,
    pub target: Target,
}

impl Workspace {
    // ----- every pane, with its place --------------------------------------------------------

    /// Every pane in recency order: the focused pane, then the others by when they were last
    /// focused, then panes never focused in id order. The switcher and Focus pane… list this.
    pub fn recency_order(&self) -> Vec<PaneId> {
        let mut out: Vec<PaneId> = Vec::with_capacity(self.panes.len());
        let push = |p: PaneId, out: &mut Vec<PaneId>| {
            if self.panes.contains_key(&p) && !out.contains(&p) {
                out.push(p);
            }
        };
        if let Some(p) = self.focused {
            push(p, &mut out);
        }
        for p in &self.recent {
            push(*p, &mut out);
        }
        for p in self.panes.keys() {
            push(*p, &mut out);
        }
        out
    }

    /// Where `pane` is now, and whether that place is currently hidden or folded.
    fn place_of(&self, pane: PaneId) -> (Place, bool, bool) {
        if let Some(f) = &self.focus {
            return match f.page_of(pane) {
                Some(i) => {
                    let folded = self
                        .group_of(pane)
                        .and_then(|g| self.group(g))
                        .is_some_and(|g| g.minimized);
                    (Place::Page(i), false, folded)
                }
                None => (Place::Tray, false, false),
            };
        }
        match self.arrangement.locate(pane) {
            Some((region, group)) => {
                let folded = self.group(group).is_some_and(|g| g.minimized);
                match region {
                    Region::Editor => (Place::Editor, false, folded),
                    Region::Dock(d) => (Place::Dock(d), !self.arrangement.dock(d).visible, folded),
                }
            }
            None => (Place::Tray, false, false),
        }
    }

    /// The entry for one pane.
    pub fn pane_entry(&self, pane: PaneId) -> Option<PaneEntry> {
        let content = self.panes.get(&pane)?;
        let (place, hidden, minimized) = self.place_of(pane);
        Some(PaneEntry {
            pane,
            title: content.title(),
            kind: content.kind(),
            place,
            hidden,
            minimized,
            focused: self.focused == Some(pane),
        })
    }

    /// Every pane, hidden, minimised and in the tray included, in [`Self::recency_order`].
    pub fn pane_entries(&self) -> Vec<PaneEntry> {
        self.recency_order()
            .into_iter()
            .filter_map(|p| self.pane_entry(p))
            .collect()
    }

    // ----- revealing and bringing ------------------------------------------------------------

    /// Focuses `pane` wherever it is (Focus pane…, the switcher): shows its dock, unfolds its
    /// group, shows its focus page, or brings it in from the tray, to the right of the focused
    /// pane. Showing, unfolding and bringing are layout changes and undoable; the focus move
    /// itself is not.
    pub fn reveal(&mut self, pane: PaneId) -> Result<()> {
        self.require(pane)?;
        self.recorded(None, |ws| {
            if let Some(f) = &ws.focus {
                match f.page_of(pane) {
                    Some(i) if i != f.current => ws.go_to_page(i)?,
                    Some(_) => {}
                    None => ws.bring_from_tray(pane, Edge::Right)?,
                }
            } else if let Some((Region::Dock(d), _)) = ws.arrangement.locate(pane) {
                ws.arrangement.dock_mut(d).visible = true;
            }
            if let Some(g) = ws.group_of(pane).and_then(|g| ws.group_mut(g)) {
                g.minimized = false;
            }
            ws.activate(pane)
        })
    }

    /// Brings a pane in from the tray beside the focused pane, on `edge` of its group (Right
    /// for Enter, Bottom for Shift-Enter in Bring from tray…). On an empty page it becomes the
    /// page's only pane. `NoOp` when the pane is not in the tray. Recorded for undo.
    pub fn bring_from_tray(&mut self, pane: PaneId, edge: Edge) -> Result<()> {
        self.require(pane)?;
        if !self.tray().contains(&pane) {
            return Err(LayoutError::NoOp);
        }
        self.recorded(None, |ws| {
            let beside = ws.focused.and_then(|p| ws.group_of(p)).or_else(|| {
                ws.focus
                    .as_ref()
                    .and_then(|f| f.root())
                    .map(|r| r.first_group().id)
            });
            match beside {
                Some(group) => ws.move_pane_raw(pane, Target::Beside { group, edge }),
                None => ws.place_on_empty_page_raw(pane),
            }?;
            ws.activate(pane)
        })
    }

    /// Puts `pane` from the tray where the focused pane is (its group, at its tab position) and
    /// sends the focused pane to the tray: flipping through files in one spot of a focus page.
    /// On an empty page `pane` becomes the page's only pane. `NoOp` when `pane` is not in the
    /// tray. One undo step.
    pub fn swap_from_tray(&mut self, pane: PaneId) -> Result<()> {
        self.require(pane)?;
        if !self.tray().contains(&pane) {
            return Err(LayoutError::NoOp);
        }
        let here = self
            .focused
            .and_then(|f| self.group_of(f).map(|g| (f, g)))
            .and_then(|(f, g)| {
                let index = self.group(g)?.tabs.iter().position(|t| *t == f)?;
                Some((f, g, index))
            });
        self.recorded(None, |ws| {
            match here {
                Some((current, group, index)) => {
                    ws.move_pane_raw(pane, Target::Tabs { group, index })?;
                    ws.move_pane_raw(current, Target::Tray)?;
                }
                None => ws.place_on_empty_page_raw(pane)?,
            }
            ws.activate(pane)
        })
    }

    /// The code pane in the tray that comes after (or before) the focused one, in the order of
    /// the normal arrangement (for files opened together, the order they were given), wrapping
    /// round. `None` outside focus or with no code pane in the tray.
    pub fn tray_file(&self, forward: bool) -> Option<PaneId> {
        let tray = self.tray();
        let files: Vec<PaneId> = self
            .arrangement
            .panes()
            .into_iter()
            .filter(|p| self.kind(*p) == Some(PaneKind::Editor))
            .collect();
        let n = files.len();
        let at = self
            .focused
            .and_then(|f| files.iter().position(|p| *p == f));
        (1..=n)
            .map(|step| match (at, forward) {
                (Some(i), true) => files[(i + step) % n],
                (Some(i), false) => files[(i + n - step % n) % n],
                (None, true) => files[step - 1],
                (None, false) => files[n - step],
            })
            .find(|p| tray.contains(p))
    }

    /// Panes waiting in the tray, with their entries, for Bring from tray….
    pub fn tray_entries(&self) -> Vec<PaneEntry> {
        let tray = self.tray();
        self.recency_order()
            .into_iter()
            .filter(|p| tray.contains(p))
            .filter_map(|p| self.pane_entry(p))
            .collect()
    }

    // ----- Move pane to… ---------------------------------------------------------------------

    /// Every place `pane` could be moved to now, named for a list: `Left dock`, `Editor area`,
    /// `Beside Outline (below)`, `Tab in Call stack`, `Tray`. The same targets as
    /// [`Workspace::valid_targets`], in the same order.
    pub fn destinations(&self, pane: PaneId) -> Vec<Destination> {
        self.valid_targets(pane)
            .into_iter()
            .filter_map(|target| {
                let (label, place) = match target {
                    Target::Dock(d) => (place_name(Place::Dock(d)), String::new()),
                    Target::EditorArea => ("Code area".into(), String::new()),
                    Target::Tray => ("Tray".into(), String::new()),
                    Target::Beside { group, edge } => {
                        let (title, place) = self.group_name(group)?;
                        let side = match edge {
                            Edge::Left => "left",
                            Edge::Right => "right",
                            Edge::Top => "above",
                            Edge::Bottom => "below",
                        };
                        (format!("Beside {title} ({side})"), place)
                    }
                    Target::Tabs { group, .. } => {
                        let (title, place) = self.group_name(group)?;
                        (format!("Tab in {title}"), place)
                    }
                    // The same as the row beside the group already along that edge.
                    Target::AreaEdge(edge) if self.area_edge_is_one_group(edge) => return None,
                    Target::AreaEdge(edge) => {
                        // A focus page fills the window; the code area sits between the
                        // docks, so say which width it is.
                        let (whole, area) = if self.in_focus() {
                            ("Whole", "This page")
                        } else {
                            ("Code area", "Code area")
                        };
                        let label = match edge {
                            Edge::Top => format!("{whole} width, top"),
                            Edge::Bottom => format!("{whole} width, bottom"),
                            Edge::Left => format!("{whole} height, left"),
                            Edge::Right => format!("{whole} height, right"),
                        };
                        (label, area.into())
                    }
                };
                Some(Destination {
                    label,
                    place,
                    target,
                })
            })
            .collect()
    }

    /// A group's name (its active tab's title) and where it is.
    fn group_name(&self, group: NodeId) -> Option<(String, String)> {
        let g = self.group(group)?;
        let title = self.panes.get(&g.active_pane()?)?.title();
        let place = match (&self.focus, self.region_of_group(group)) {
            // Only the current page's groups are targets.
            (Some(f), _) => place_name(Place::Page(f.current)),
            (None, Some(Region::Editor)) => place_name(Place::Editor),
            (None, Some(Region::Dock(d))) => place_name(Place::Dock(d)),
            (None, None) => String::new(),
        };
        Some((title, place))
    }

    // ----- dock sizes ------------------------------------------------------------------------

    /// The dock holding `pane` in the normal arrangement, shown or hidden.
    pub(crate) fn dock_of(&self, pane: PaneId) -> Option<DockId> {
        match self.arrangement.locate(pane) {
            Some((Region::Dock(d), _)) => Some(d),
            _ => None,
        }
    }

    /// The dock of the focused pane (Ctrl-Alt-= and friends act on it). `NotInDock` when the
    /// focus is in the editor area, and in a focus layout, where docks are hidden.
    pub fn focused_dock(&self) -> Result<DockId> {
        if self.focus.is_some() {
            return Err(LayoutError::DocksHiddenInFocus);
        }
        self.focused
            .and_then(|p| self.dock_of(p))
            .ok_or(LayoutError::NotInDock)
    }

    /// Grows (`+`) or shrinks (`-`) the focused pane's dock by [`DOCK_STEP`], clamped to the
    /// dock's range. Returns the new size. Consecutive steps are one undo step.
    pub fn step_focused_dock(&mut self, grow: bool) -> Result<f32> {
        let dock = self.focused_dock()?;
        let size = self.arrangement.dock(dock).size;
        self.resize_dock(dock, size + if grow { DOCK_STEP } else { -DOCK_STEP })
    }

    /// Puts the focused pane's dock back to its default size. Returns the size.
    pub fn reset_focused_dock(&mut self) -> Result<f32> {
        let dock = self.focused_dock()?;
        self.resize_dock(dock, dock.default_size())
    }
}

/// The Ctrl-Tab switcher's state: the recency order when it opened, and the selected row. Holds
/// no reference to the workspace, so the order stays put while the switcher is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switcher {
    order: Vec<PaneId>,
    index: usize,
}

impl Switcher {
    /// Opens the switcher on the previous pane (the second in the order), or on the
    /// least recent one when `backwards` (Ctrl-Shift-Tab). `None` when there is nothing to
    /// switch to.
    pub fn open(ws: &Workspace, backwards: bool) -> Option<Self> {
        let order = ws.recency_order();
        if order.len() < 2 {
            return None;
        }
        let index = if backwards { order.len() - 1 } else { 1 };
        Some(Self { order, index })
    }

    /// Moves the selection one row, wrapping at both ends.
    pub fn step(&mut self, backwards: bool) {
        let n = self.order.len();
        self.index = if backwards {
            (self.index + n - 1) % n
        } else {
            (self.index + 1) % n
        };
    }

    /// Selects a row (a click).
    pub fn select(&mut self, index: usize) {
        self.index = index.min(self.order.len().saturating_sub(1));
    }

    pub fn selected(&self) -> PaneId {
        self.order[self.index]
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn order(&self) -> &[PaneId] {
        &self.order
    }
}

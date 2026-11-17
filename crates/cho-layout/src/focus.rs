//! Focus sessions and their pages.
//!
//! A focus session is temporary: it holds one or more pages, each its own split tree, and a
//! current page. Every pane that is on no page waits in the tray, which the whole session
//! shares. The normal arrangement is left untouched while the session is open, which is what
//! makes Restore exact.

use crate::tree::{Group, Node, NodeId, Split};
use crate::{PaneId, PaneSource};
use serde::{Deserialize, Serialize};

/// How [`crate::Workspace::reset_page`] lays a page's declared panes out. A tour names one per
/// step and never gives sizes or positions, so the same tour always gives the same pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PageArrangement {
    /// One pane, the whole page (more go side by side).
    Single,
    /// Side by side, equal widths (the arrangement before tours named one).
    #[default]
    SideBySide,
    /// One above the other, equal heights.
    Stacked,
    /// The first pane large on the left ([`MAIN_SHARE`] of the width), the rest stacked
    /// beside it.
    MainSide,
    /// Two by two, rows of two from the top.
    Grid,
}

/// The first pane's share of the width in [`PageArrangement::MainSide`].
pub const MAIN_SHARE: f32 = 0.63;

impl PageArrangement {
    pub const ALL: [PageArrangement; 5] = [
        PageArrangement::Single,
        PageArrangement::SideBySide,
        PageArrangement::Stacked,
        PageArrangement::MainSide,
        PageArrangement::Grid,
    ];

    /// The name a tour gives it: `single`, `side-by-side`, `stacked`, `main-side`, `grid`.
    pub fn name(self) -> &'static str {
        match self {
            PageArrangement::Single => "single",
            PageArrangement::SideBySide => "side-by-side",
            PageArrangement::Stacked => "stacked",
            PageArrangement::MainSide => "main-side",
            PageArrangement::Grid => "grid",
        }
    }

    pub fn parse(name: &str) -> Option<PageArrangement> {
        PageArrangement::ALL.into_iter().find(|a| a.name() == name)
    }
}

/// Where a page came from, so a later "reset page" can rebuild it. Only tours make pages with
/// an origin; every field is optional so later origins fit without a format break.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageOrigin {
    /// The tour step the page was made for.
    #[serde(default)]
    pub tour_step: Option<String>,
    /// The page template the step named.
    #[serde(default)]
    pub template: Option<String>,
    /// The panes the step declared, in order.
    #[serde(default)]
    pub panes: Vec<PaneSource>,
    /// How the declared panes are laid out.
    #[serde(default)]
    pub arrangement: PageArrangement,
}

/// One page of a focus session. A new page starts empty; panes come from the tray.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FocusPage {
    pub root: Option<Node>,
    pub origin: Option<PageOrigin>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FocusSession {
    /// Never empty.
    pub pages: Vec<FocusPage>,
    /// The page on screen.
    pub current: usize,
    /// The pane that was maximised to start the session.
    pub maximized: PaneId,
    /// The editor pane read most recently in the session, on any page. It becomes the active
    /// editor tab on restore, so restoring keeps the reading position, and it carries across
    /// page switches. Always on a page: an editor that leaves every page (to the tray, or
    /// closed with its page) is forgotten.
    pub last_editor: Option<PaneId>,
    pub started_at: u64,
}

impl FocusSession {
    /// The page on screen.
    pub fn page(&self) -> &FocusPage {
        &self.pages[self.current]
    }

    /// The tree on screen, if the current page has any panes.
    pub fn root(&self) -> Option<&Node> {
        self.page().root.as_ref()
    }

    pub(crate) fn root_mut(&mut self) -> &mut Option<Node> {
        &mut self.pages[self.current].root
    }

    /// Panes on the current page, in reading order.
    pub fn on_screen(&self) -> Vec<PaneId> {
        self.root().map(Node::panes).unwrap_or_default()
    }

    /// The page holding `pane`.
    pub fn page_of(&self, pane: PaneId) -> Option<usize> {
        self.pages
            .iter()
            .position(|p| p.root.as_ref().is_some_and(|r| r.group_of(pane).is_some()))
    }

    /// Every pane on any page, page by page.
    pub fn on_pages(&self) -> Vec<PaneId> {
        self.pages
            .iter()
            .filter_map(|p| p.root.as_ref())
            .flat_map(Node::panes)
            .collect()
    }

    pub(crate) fn node_ids(&self) -> Vec<NodeId> {
        self.pages
            .iter()
            .filter_map(|p| p.root.as_ref())
            .flat_map(Node::node_ids)
            .collect()
    }

    pub fn group(&self, id: NodeId) -> Option<&Group> {
        self.root().and_then(|r| r.group(id))
    }

    pub fn group_of(&self, pane: PaneId) -> Option<NodeId> {
        self.root().and_then(|r| r.group_of(pane))
    }

    pub(crate) fn group_mut(&mut self, id: NodeId) -> Option<&mut Group> {
        self.root_mut().as_mut().and_then(|r| r.group_mut(id))
    }

    pub(crate) fn split_mut(&mut self, id: NodeId) -> Option<&mut Split> {
        self.root_mut().as_mut().and_then(|r| r.split_mut(id))
    }

    /// The active pane of the first group on screen.
    pub(crate) fn first_active(&self) -> Option<PaneId> {
        self.root().and_then(|r| r.first_group().active_pane())
    }

    /// Forgets [`FocusSession::last_editor`] when it is on no page any more. Every operation
    /// that sends panes off the pages calls this.
    pub(crate) fn forget_off_page_editor(&mut self) {
        if self.last_editor.is_some_and(|e| self.page_of(e).is_none()) {
            self.last_editor = None;
        }
    }

    /// Takes `pane` off whichever page holds it. Returns whether it was on one.
    pub(crate) fn remove(&mut self, pane: PaneId) -> bool {
        for page in &mut self.pages {
            if let Some(r) = &mut page.root
                && r.remove_pane(pane)
            {
                crate::tree::normalize_in_place(&mut page.root);
                return true;
            }
        }
        false
    }
}

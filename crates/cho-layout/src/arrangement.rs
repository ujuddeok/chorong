//! The normal window arrangement: three docks and the editor area.

use crate::PaneId;
use crate::tree::{Node, NodeId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockId {
    Left,
    Right,
    /// The full-width bottom bar.
    Bottom,
}

impl DockId {
    pub const ALL: [DockId; 3] = [DockId::Left, DockId::Right, DockId::Bottom];

    /// Allowed size in logical pixels: width for side docks, height for the bottom bar.
    pub fn size_range(self) -> (f32, f32) {
        match self {
            DockId::Left | DockId::Right => (160.0, 640.0),
            // Low enough for one row of chips (the Highlights pane) under a pane header.
            DockId::Bottom => (64.0, 600.0),
        }
    }

    pub fn default_size(self) -> f32 {
        match self {
            DockId::Left => 260.0,
            DockId::Right => 320.0,
            DockId::Bottom => 220.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DockId::Left => "left dock",
            DockId::Right => "right dock",
            DockId::Bottom => "bottom bar",
        }
    }
}

/// Where a group lives in the normal arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    Dock(DockId),
    Editor,
}

impl Region {
    /// Regions in reading order: left dock, editor area, right dock, bottom bar. Used wherever
    /// an order is needed (the tray, repairs), so results never depend on hashing.
    pub const ALL: [Region; 4] = [
        Region::Dock(DockId::Left),
        Region::Editor,
        Region::Dock(DockId::Right),
        Region::Dock(DockId::Bottom),
    ];
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dock {
    #[serde(default)]
    pub root: Option<Node>,
    /// Width (side docks) or height (bottom bar) in logical pixels.
    pub size: f32,
    #[serde(default = "yes")]
    pub visible: bool,
}

fn yes() -> bool {
    true
}

impl Dock {
    pub fn new(id: DockId) -> Self {
        Self {
            root: None,
            size: id.default_size(),
            visible: true,
        }
    }
}

/// Which pane is where, outside a focus layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Arrangement {
    pub left: Dock,
    pub right: Dock,
    pub bottom: Dock,
    #[serde(default)]
    pub editor: Option<Node>,
}

impl Default for Arrangement {
    fn default() -> Self {
        Self {
            left: Dock::new(DockId::Left),
            right: Dock::new(DockId::Right),
            bottom: Dock::new(DockId::Bottom),
            editor: None,
        }
    }
}

impl Arrangement {
    pub fn dock(&self, id: DockId) -> &Dock {
        match id {
            DockId::Left => &self.left,
            DockId::Right => &self.right,
            DockId::Bottom => &self.bottom,
        }
    }

    pub fn dock_mut(&mut self, id: DockId) -> &mut Dock {
        match id {
            DockId::Left => &mut self.left,
            DockId::Right => &mut self.right,
            DockId::Bottom => &mut self.bottom,
        }
    }

    pub fn root(&self, region: Region) -> &Option<Node> {
        match region {
            Region::Dock(d) => &self.dock(d).root,
            Region::Editor => &self.editor,
        }
    }

    pub fn root_mut(&mut self, region: Region) -> &mut Option<Node> {
        match region {
            Region::Dock(d) => &mut self.dock_mut(d).root,
            Region::Editor => &mut self.editor,
        }
    }

    /// Every pane, in [`Region::ALL`] order.
    pub fn panes(&self) -> Vec<PaneId> {
        Region::ALL
            .iter()
            .filter_map(|r| self.root(*r).as_ref())
            .flat_map(|n| n.panes())
            .collect()
    }

    /// The region and group holding `pane`.
    pub fn locate(&self, pane: PaneId) -> Option<(Region, NodeId)> {
        Region::ALL.iter().find_map(|r| {
            self.root(*r)
                .as_ref()
                .and_then(|n| n.group_of(pane))
                .map(|g| (*r, g))
        })
    }

    /// The region holding the group or split `id`.
    pub fn region_of_node(&self, id: NodeId) -> Option<Region> {
        Region::ALL.iter().copied().find(|r| {
            self.root(*r)
                .as_ref()
                .is_some_and(|n| n.group(id).is_some() || n.split(id).is_some())
        })
    }

    pub fn node_ids(&self) -> Vec<NodeId> {
        Region::ALL
            .iter()
            .filter_map(|r| self.root(*r).as_ref())
            .flat_map(|n| n.node_ids())
            .collect()
    }
}

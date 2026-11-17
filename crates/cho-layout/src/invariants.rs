//! Structural invariants every workspace keeps. Tests check them after every operation; loading
//! repairs a saved file until they hold.

use crate::arrangement::{DockId, Region};
use crate::history::HISTORY_LIMIT;
use crate::tree::{Node, NodeId};
use crate::{PaneId, PaneKind, Workspace};
use std::collections::BTreeSet;

impl Workspace {
    /// Checks every invariant, returning the first that fails.
    ///
    /// - every pane is in the normal arrangement exactly once, and nothing else is;
    /// - editor panes are in the editor area only; tab groups in the normal arrangement never
    ///   mix editor and tool panes; an editor area that holds anything holds an editor group;
    /// - a focus session has at least one page and a valid current page; no pane is on two
    ///   pages (or twice on one), and only known panes are; tray + pages = every pane; the last
    ///   editor read is on a page;
    /// - group and split ids are unique and below the id counter, as are pane ids;
    /// - groups are non-empty with a valid active tab, and pins follow tool tabs;
    /// - splits have two or more children, shares are positive and sum to 1, and no split
    ///   directly holds a split along the same axis;
    /// - dock sizes are in range; focus history holds at most 20 entries;
    /// - the focused pane exists.
    pub fn check_invariants(&self) -> Result<(), String> {
        let placed = self.arrangement.panes();
        let mut seen = BTreeSet::new();
        for p in &placed {
            if !seen.insert(*p) {
                return Err(format!("{p:?} is placed twice"));
            }
            if !self.panes.contains_key(p) {
                return Err(format!("{p:?} is placed but does not exist"));
            }
        }
        if let Some(p) = self.panes.keys().find(|p| !seen.contains(p)) {
            return Err(format!("{p:?} exists but is not placed"));
        }
        for r in Region::ALL {
            let Some(root) = self.arrangement.root(r) else {
                continue;
            };
            for g in root.groups() {
                let editors = g
                    .tabs
                    .iter()
                    .filter(|p| self.kind(**p) == Some(PaneKind::Editor));
                let editors = editors.count();
                if editors != 0 && editors != g.tabs.len() {
                    return Err(format!("{:?} mixes editor and tool tabs", g.id));
                }
                if editors != 0 && r != Region::Editor {
                    return Err(format!("{:?} holds editors in {r:?}", g.id));
                }
            }
            if r == Region::Editor && !root.groups().iter().any(|g| self.is_editor_group(g)) {
                return Err("the editor area has no editor group".into());
            }
        }
        let mut nodes: Vec<NodeId> = self.arrangement.node_ids();
        if let Some(f) = &self.focus {
            if f.pages.is_empty() || f.current >= f.pages.len() {
                return Err("focus session has no current page".into());
            }
            let on_pages = f.on_pages();
            let unique: BTreeSet<PaneId> = on_pages.iter().copied().collect();
            if unique.len() != on_pages.len() {
                return Err("a pane is on two focus pages, or twice on one".into());
            }
            if let Some(p) = on_pages.iter().find(|p| !self.panes.contains_key(p)) {
                return Err(format!("{p:?} is on a page but does not exist"));
            }
            let mut all = on_pages.clone();
            all.extend(self.tray());
            all.sort();
            if all.iter().ne(self.panes.keys()) {
                return Err("tray and pages do not hold every pane exactly once".into());
            }
            if !self.panes.contains_key(&f.maximized) {
                return Err("the maximised pane does not exist".into());
            }
            if let Some(e) = f.last_editor
                && f.page_of(e).is_none()
            {
                return Err(format!("the last editor {e:?} is on no page"));
            }
            nodes.extend(f.node_ids());
            for page in &f.pages {
                if let Some(root) = &page.root {
                    check_tree(self, root)?;
                }
            }
        }
        let unique_nodes: BTreeSet<NodeId> = nodes.iter().copied().collect();
        if unique_nodes.len() != nodes.len() {
            return Err("a group or split id is used twice".into());
        }
        if let Some(n) = nodes.iter().find(|n| n.0 >= self.next_id) {
            return Err(format!("{n:?} is not below the id counter"));
        }
        if let Some(p) = self.panes.keys().find(|p| p.0 >= self.next_id) {
            return Err(format!("{p:?} is not below the id counter"));
        }
        if self
            .panes
            .keys()
            .any(|p| unique_nodes.contains(&NodeId(p.0)))
        {
            return Err("a pane id is also a node id".into());
        }
        for r in Region::ALL {
            if let Some(root) = self.arrangement.root(r) {
                check_tree(self, root)?;
            }
        }
        for d in DockId::ALL {
            let (lo, hi) = d.size_range();
            let size = self.arrangement.dock(d).size;
            if !(lo..=hi).contains(&size) {
                return Err(format!("{d:?} size {size} is out of range"));
            }
        }
        if self.history.len() > HISTORY_LIMIT {
            return Err("focus history is too long".into());
        }
        if let Some(p) = self.focused
            && !self.panes.contains_key(&p)
        {
            return Err(format!("focused {p:?} does not exist"));
        }
        Ok(())
    }
}

fn check_tree(ws: &Workspace, node: &Node) -> Result<(), String> {
    match node {
        Node::Group(g) => {
            if g.tabs.is_empty() {
                return Err(format!("{:?} is empty", g.id));
            }
            if g.active >= g.tabs.len() {
                return Err(format!("{:?} has no active tab", g.id));
            }
            let pins: Vec<bool> = g
                .tabs
                .iter()
                .map(|p| ws.kind(*p).is_some_and(PaneKind::is_pin))
                .collect();
            if pins.windows(2).any(|w| w[0] && !w[1]) {
                return Err(format!("{:?} has a tool tab after a pin", g.id));
            }
            Ok(())
        }
        Node::Split(s) => {
            if s.children.len() < 2 {
                return Err(format!("{:?} has fewer than two children", s.id));
            }
            let total: f32 = s.children.iter().map(|c| c.weight).sum();
            if s.children
                .iter()
                .any(|c| !c.weight.is_finite() || c.weight <= 0.0)
                || (total - 1.0).abs() > 1e-3
            {
                return Err(format!("{:?} has bad shares (sum {total})", s.id));
            }
            for c in &s.children {
                if let Node::Split(inner) = &c.node
                    && inner.axis == s.axis
                {
                    return Err(format!("{:?} nests a split on the same axis", s.id));
                }
                check_tree(ws, &c.node)?;
            }
            Ok(())
        }
    }
}

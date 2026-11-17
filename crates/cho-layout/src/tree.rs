//! Split trees of tab groups. Every region of the window (each dock, the editor area, a focus
//! layout) is one optional tree.

use crate::PaneId;
use serde::{Deserialize, Serialize};

/// The id of a group or split. Allocated from the same counter as pane ids' counter, so ids are
/// unique within a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// How a split lays out its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Side by side, left to right.
    Row,
    /// Stacked, top to bottom.
    Column,
}

/// Which side of a group a dropped pane lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    pub fn axis(self) -> Axis {
        match self {
            Edge::Left | Edge::Right => Axis::Row,
            Edge::Top | Edge::Bottom => Axis::Column,
        }
    }

    /// Whether the new pane goes after (right of, or below) the target.
    pub fn is_after(self) -> bool {
        matches!(self, Edge::Right | Edge::Bottom)
    }
}

/// The smallest share of a split one child may have after a resize.
pub const MIN_WEIGHT: f32 = 0.05;

/// One or more panes shown as tabs, one of them active. A group of one pane shows the pane's
/// header instead of a tab strip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: NodeId,
    pub tabs: Vec<PaneId>,
    #[serde(default)]
    pub active: usize,
    /// Folded to its 28 px header in place.
    #[serde(default)]
    pub minimized: bool,
}

impl Group {
    pub fn new(id: NodeId, tabs: Vec<PaneId>) -> Self {
        Self {
            id,
            tabs,
            active: 0,
            minimized: false,
        }
    }

    pub fn active_pane(&self) -> Option<PaneId> {
        self.tabs.get(self.active).copied()
    }
}

/// One child of a split and its share of the split's length (shares sum to 1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Child {
    pub weight: f32,
    pub node: Node,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub id: NodeId,
    pub axis: Axis,
    pub children: Vec<Child>,
}

impl Split {
    /// The child that runs along the whole of `edge` (the first or the last), when the split
    /// lies along that edge's axis.
    pub fn end(&self, edge: Edge) -> Option<&Child> {
        if self.axis != edge.axis() {
            None
        } else if edge.is_after() {
            self.children.last()
        } else {
            self.children.first()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Group(Group),
    Split(Split),
}

impl Node {
    pub fn id(&self) -> NodeId {
        match self {
            Node::Group(g) => g.id,
            Node::Split(s) => s.id,
        }
    }

    /// Every pane in the tree, in reading order (depth first, tabs in order).
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.walk_groups(&mut |g| out.extend(g.tabs.iter().copied()));
        out
    }

    /// Every group, in reading order.
    pub fn groups(&self) -> Vec<&Group> {
        let mut out = Vec::new();
        fn go<'a>(n: &'a Node, out: &mut Vec<&'a Group>) {
            match n {
                Node::Group(g) => out.push(g),
                Node::Split(s) => s.children.iter().for_each(|c| go(&c.node, out)),
            }
        }
        go(self, &mut out);
        out
    }

    /// Every node id (groups and splits), depth first.
    pub fn node_ids(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        fn go(n: &Node, out: &mut Vec<NodeId>) {
            out.push(n.id());
            if let Node::Split(s) = n {
                s.children.iter().for_each(|c| go(&c.node, out));
            }
        }
        go(self, &mut out);
        out
    }

    fn walk_groups(&self, f: &mut impl FnMut(&Group)) {
        match self {
            Node::Group(g) => f(g),
            Node::Split(s) => s.children.iter().for_each(|c| c.node.walk_groups(f)),
        }
    }

    pub fn group(&self, id: NodeId) -> Option<&Group> {
        match self {
            Node::Group(g) => (g.id == id).then_some(g),
            Node::Split(s) => s.children.iter().find_map(|c| c.node.group(id)),
        }
    }

    pub fn group_mut(&mut self, id: NodeId) -> Option<&mut Group> {
        match self {
            Node::Group(g) => (g.id == id).then_some(g),
            Node::Split(s) => s.children.iter_mut().find_map(|c| c.node.group_mut(id)),
        }
    }

    pub fn split(&self, id: NodeId) -> Option<&Split> {
        match self {
            Node::Group(_) => None,
            Node::Split(s) if s.id == id => Some(s),
            Node::Split(s) => s.children.iter().find_map(|c| c.node.split(id)),
        }
    }

    pub fn split_mut(&mut self, id: NodeId) -> Option<&mut Split> {
        match self {
            Node::Group(_) => None,
            Node::Split(s) => {
                if s.id == id {
                    Some(s)
                } else {
                    s.children.iter_mut().find_map(|c| c.node.split_mut(id))
                }
            }
        }
    }

    /// The group holding `pane`.
    pub fn group_of(&self, pane: PaneId) -> Option<NodeId> {
        self.groups()
            .into_iter()
            .find(|g| g.tabs.contains(&pane))
            .map(|g| g.id)
    }

    pub fn first_group(&self) -> &Group {
        match self {
            Node::Group(g) => g,
            Node::Split(s) => s.children[0].node.first_group(),
        }
    }

    /// Removes `pane` from whichever group holds it, keeping the active tab sensible. Leaves
    /// empty groups for [`normalize`] to remove.
    pub fn remove_pane(&mut self, pane: PaneId) -> bool {
        match self {
            Node::Group(g) => match g.tabs.iter().position(|p| *p == pane) {
                Some(i) => {
                    g.tabs.remove(i);
                    if i < g.active || g.active >= g.tabs.len() {
                        g.active = g.active.saturating_sub(1);
                    }
                    true
                }
                None => false,
            },
            Node::Split(s) => s.children.iter_mut().any(|c| c.node.remove_pane(pane)),
        }
    }

    /// Puts `new` beside the node `target` on `edge`. Joins the parent split when it already
    /// runs along that axis, so repeated drops make a flat row or column. Returns `new` back if
    /// `target` is not in this tree.
    pub fn insert_beside(
        &mut self,
        target: NodeId,
        edge: Edge,
        new: Node,
        split_id: NodeId,
    ) -> Result<(), Node> {
        if self.id() == target {
            let old = std::mem::replace(self, Node::Group(Group::new(NodeId(0), Vec::new())));
            let (a, b) = if edge.is_after() {
                (old, new)
            } else {
                (new, old)
            };
            *self = Node::Split(Split {
                id: split_id,
                axis: edge.axis(),
                children: vec![
                    Child {
                        weight: 0.5,
                        node: a,
                    },
                    Child {
                        weight: 0.5,
                        node: b,
                    },
                ],
            });
            return Ok(());
        }
        let Node::Split(s) = self else {
            return Err(new);
        };
        if let Some(i) = s.children.iter().position(|c| c.node.id() == target) {
            if s.axis == edge.axis() {
                let half = s.children[i].weight / 2.0;
                s.children[i].weight = half;
                let at = if edge.is_after() { i + 1 } else { i };
                s.children.insert(
                    at,
                    Child {
                        weight: half,
                        node: new,
                    },
                );
                // Halving again and again would leave slivers: share equally instead.
                if half < MIN_WEIGHT {
                    let w = 1.0 / s.children.len() as f32;
                    s.children.iter_mut().for_each(|c| c.weight = w);
                }
                return Ok(());
            }
            return s.children[i]
                .node
                .insert_beside(target, edge, new, split_id);
        }
        let mut new = new;
        for c in &mut s.children {
            match c.node.insert_beside(target, edge, new, split_id) {
                Ok(()) => return Ok(()),
                Err(back) => new = back,
            }
        }
        Err(new)
    }

    /// Puts `new` along the whole of the tree's `edge`, with `share` of the length: it spans
    /// everything already there. A split that runs along that axis takes it as its first or
    /// last child (the others shrink in proportion); anything else is wrapped in a new split.
    pub fn at_edge(self, edge: Edge, new: Node, split_id: NodeId, share: f32) -> Node {
        let child = Child {
            weight: share,
            node: new,
        };
        match self {
            Node::Split(mut s) if s.axis == edge.axis() => {
                for c in &mut s.children {
                    c.weight *= 1.0 - share;
                }
                match edge.is_after() {
                    true => s.children.push(child),
                    false => s.children.insert(0, child),
                }
                Node::Split(s)
            }
            old => {
                let old = Child {
                    weight: 1.0 - share,
                    node: old,
                };
                Node::Split(Split {
                    id: split_id,
                    axis: edge.axis(),
                    children: match edge.is_after() {
                        true => vec![old, child],
                        false => vec![child, old],
                    },
                })
            }
        }
    }

    /// Appends `new` at the end of the tree along `axis` (a new row or column at the far edge).
    pub fn append(self, axis: Axis, new: Node, split_id: NodeId) -> Node {
        match self {
            Node::Split(mut s) if s.axis == axis => {
                let n = s.children.len() as f32;
                for c in &mut s.children {
                    c.weight *= n / (n + 1.0);
                }
                s.children.push(Child {
                    weight: 1.0 / (n + 1.0),
                    node: new,
                });
                Node::Split(s)
            }
            old => Node::Split(Split {
                id: split_id,
                axis,
                children: vec![
                    Child {
                        weight: 0.5,
                        node: old,
                    },
                    Child {
                        weight: 0.5,
                        node: new,
                    },
                ],
            }),
        }
    }
}

/// Removes empty groups, collapses splits with one child, flattens a split into a parent that
/// runs along the same axis, and rescales shares to sum to 1. Returns `None` for an empty tree.
pub fn normalize(node: Node) -> Option<Node> {
    match node {
        Node::Group(mut g) => {
            if g.tabs.is_empty() {
                return None;
            }
            if g.active >= g.tabs.len() {
                g.active = g.tabs.len() - 1;
            }
            Some(Node::Group(g))
        }
        Node::Split(s) => {
            let mut kids: Vec<Child> = Vec::new();
            for c in s.children {
                match normalize(c.node) {
                    None => {}
                    Some(Node::Split(inner)) if inner.axis == s.axis => {
                        let total: f32 = inner.children.iter().map(|k| k.weight).sum();
                        for k in inner.children {
                            kids.push(Child {
                                weight: c.weight * k.weight / total,
                                node: k.node,
                            });
                        }
                    }
                    Some(n) => kids.push(Child {
                        weight: c.weight,
                        node: n,
                    }),
                }
            }
            match kids.len() {
                0 => None,
                1 => kids.pop().map(|c| c.node),
                _ => {
                    rescale(&mut kids);
                    Some(Node::Split(Split {
                        id: s.id,
                        axis: s.axis,
                        children: kids,
                    }))
                }
            }
        }
    }
}

/// Makes shares sum to 1; equal shares when they are unusable (zero, negative or not finite).
fn rescale(kids: &mut [Child]) {
    let usable = kids.iter().all(|c| c.weight.is_finite() && c.weight > 0.0);
    let total: f32 = kids.iter().map(|c| c.weight).sum();
    if !usable || !total.is_finite() || total <= 0.0 {
        let w = 1.0 / kids.len() as f32;
        kids.iter_mut().for_each(|c| c.weight = w);
    } else if (total - 1.0).abs() > 1e-6 {
        kids.iter_mut().for_each(|c| c.weight /= total);
    }
}

/// Normalises an optional tree in place.
pub fn normalize_in_place(root: &mut Option<Node>) {
    *root = root.take().and_then(normalize);
}

/// Keeps only the tabs for which `keep` is true, in every group. Leaves empty groups for
/// [`normalize`].
pub(crate) fn retain_tabs(node: &mut Node, keep: &mut impl FnMut(PaneId) -> bool) {
    match node {
        Node::Group(g) => {
            let active = g.active_pane();
            g.tabs.retain(|p| keep(*p));
            g.active = active
                .and_then(|a| g.tabs.iter().position(|p| *p == a))
                .unwrap_or(0);
        }
        Node::Split(s) => s
            .children
            .iter_mut()
            .for_each(|c| retain_tabs(&mut c.node, keep)),
    }
}

/// Gives every group and split a fresh id.
pub(crate) fn renumber(node: &mut Node, next: &mut impl FnMut() -> NodeId) {
    match node {
        Node::Group(g) => g.id = next(),
        Node::Split(s) => {
            s.id = next();
            s.children
                .iter_mut()
                .for_each(|c| renumber(&mut c.node, next));
        }
    }
}

/// Applies `f` to every group.
pub(crate) fn for_each_group_mut(node: &mut Node, f: &mut impl FnMut(&mut Group)) {
    match node {
        Node::Group(g) => f(g),
        Node::Split(s) => s
            .children
            .iter_mut()
            .for_each(|c| for_each_group_mut(&mut c.node, f)),
    }
}

impl Node {
    /// The splits from this node down to the node `id`, each with the index of the child the
    /// path goes through, outermost first. Empty when `id` is this node; `None` when it is not
    /// in the tree.
    pub fn path_to(&self, id: NodeId) -> Option<Vec<(NodeId, usize)>> {
        if self.id() == id {
            return Some(Vec::new());
        }
        let Node::Split(s) = self else { return None };
        s.children.iter().enumerate().find_map(|(i, c)| {
            c.node.path_to(id).map(|mut rest| {
                rest.insert(0, (s.id, i));
                rest
            })
        })
    }
}

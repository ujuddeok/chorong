//! Focus history: each focus layout, recorded by its panes' sources when it ends.

use crate::focus::{FocusSession, PageOrigin};
use crate::tree::{Axis, Node};
use crate::{PaneContent, PaneId, PaneSource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How many focus layouts are kept per repository.
pub const HISTORY_LIMIT: usize = 20;

/// A focus layout's shape, with panes recorded by source so it can be reopened with current
/// content in a later session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecordNode {
    Group {
        tabs: Vec<PaneSource>,
        #[serde(default)]
        active: usize,
        #[serde(default)]
        minimized: bool,
    },
    Split {
        axis: Axis,
        children: Vec<RecordChild>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordChild {
    pub weight: f32,
    pub node: RecordNode,
}

impl RecordNode {
    pub(crate) fn from_node(node: &Node, panes: &BTreeMap<PaneId, PaneContent>) -> Self {
        match node {
            Node::Group(g) => RecordNode::Group {
                tabs: g
                    .tabs
                    .iter()
                    .filter_map(|p| panes.get(p).map(PaneContent::source))
                    .collect(),
                active: g.active,
                minimized: g.minimized,
            },
            Node::Split(s) => RecordNode::Split {
                axis: s.axis,
                children: s
                    .children
                    .iter()
                    .map(|c| RecordChild {
                        weight: c.weight,
                        node: RecordNode::from_node(&c.node, panes),
                    })
                    .collect(),
            },
        }
    }

    /// Every source, in reading order.
    pub fn sources(&self) -> Vec<&PaneSource> {
        match self {
            RecordNode::Group { tabs, .. } => tabs.iter().collect(),
            RecordNode::Split { children, .. } => {
                children.iter().flat_map(|c| c.node.sources()).collect()
            }
        }
    }

    /// The same shape ignoring sizes, minimised state and active tabs: two records with equal
    /// shapes are the same focus layout, so history keeps only the newest.
    fn same_shape(&self, other: &RecordNode) -> bool {
        match (self, other) {
            (RecordNode::Group { tabs: a, .. }, RecordNode::Group { tabs: b, .. }) => a == b,
            (
                RecordNode::Split {
                    axis: a,
                    children: ca,
                },
                RecordNode::Split {
                    axis: b,
                    children: cb,
                },
            ) => {
                a == b
                    && ca.len() == cb.len()
                    && ca.iter().zip(cb).all(|(x, y)| x.node.same_shape(&y.node))
            }
            _ => false,
        }
    }
}

/// One page of an ended focus layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordPage {
    pub layout: RecordNode,
    #[serde(default)]
    pub origin: Option<PageOrigin>,
}

/// One ended focus layout: every page that had panes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusRecord {
    pub pages: Vec<RecordPage>,
    /// The page that was on screen when it ended.
    #[serde(default)]
    pub current: usize,
    /// The pane that was maximised to start it.
    pub maximized: PaneSource,
    /// Seconds since the Unix epoch, supplied by the caller (the model never reads a clock).
    pub ended_at: u64,
}

/// A version-1 record: a single page.
#[derive(Deserialize)]
pub(crate) struct RecordV1 {
    layout: RecordNode,
    maximized: PaneSource,
    ended_at: u64,
}

impl From<RecordV1> for FocusRecord {
    fn from(r: RecordV1) -> Self {
        FocusRecord {
            pages: vec![RecordPage {
                layout: r.layout,
                origin: None,
            }],
            current: 0,
            maximized: r.maximized,
            ended_at: r.ended_at,
        }
    }
}

impl FocusRecord {
    /// Records a session's pages; `None` when no page has a pane.
    pub(crate) fn of(
        session: &FocusSession,
        panes: &BTreeMap<PaneId, PaneContent>,
        ended_at: u64,
    ) -> Option<Self> {
        let mut current = 0;
        let mut pages = Vec::new();
        for (i, p) in session.pages.iter().enumerate() {
            let Some(root) = &p.root else { continue };
            if i <= session.current {
                current = pages.len();
            }
            pages.push(RecordPage {
                layout: RecordNode::from_node(root, panes),
                origin: p.origin.clone(),
            });
        }
        (!pages.is_empty()).then(|| FocusRecord {
            pages,
            current,
            maximized: panes[&session.maximized].source(),
            ended_at,
        })
    }

    /// Every source, page by page.
    pub fn sources(&self) -> Vec<&PaneSource> {
        self.pages.iter().flat_map(|p| p.layout.sources()).collect()
    }

    /// `Call stack, buffer.rs, LineCol`: the panes' labels in reading order, pages separated
    /// by ` | `.
    pub fn label(&self) -> String {
        self.pages
            .iter()
            .map(|p| {
                p.layout
                    .sources()
                    .iter()
                    .map(|s| s.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn same_shape(&self, other: &FocusRecord) -> bool {
        self.pages.len() == other.pages.len()
            && self
                .pages
                .iter()
                .zip(&other.pages)
                .all(|(a, b)| a.layout.same_shape(&b.layout))
    }
}

/// Adds `record` at the front, dropping an older record of the same shape and anything past
/// [`HISTORY_LIMIT`].
pub(crate) fn push(history: &mut Vec<FocusRecord>, record: FocusRecord) {
    history.retain(|r| !r.same_shape(&record));
    history.insert(0, record);
    history.truncate(HISTORY_LIMIT);
}

/// A focus history entry as the Reopen focus layout list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub index: usize,
    /// `Call stack, buffer.rs, LineCol`
    pub label: String,
    /// `10 min ago`
    pub ago: String,
    /// How many pages it had.
    pub pages: usize,
    /// Sources that can no longer be shown (a closed pin, a removed file), with the index of
    /// the page they were on. They are left out when reopening.
    pub missing: Vec<(usize, PaneSource)>,
}

/// `just now`, `10 min ago`, `3 h ago`, `2 days ago`.
pub fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "1 day ago".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

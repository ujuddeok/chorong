//! Panes: what a pane shows, independent of where it is.

use serde::{Deserialize, Serialize};

/// A pane's stable id. Allocated from the workspace's counter, never reused while the workspace
/// lives, and saved with the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(pub u64);

/// What kind of pane this is. Decides its icon, its default place and where it may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    FileTree,
    Tour,
    Context,
    PinnedContext,
    Outline,
    CallStack,
    /// A format version 2 comparison: one row per step, a column per side.
    Comparison,
    Graph,
    Editor,
    Search,
    /// The symbols a comparison with a branch touched.
    Changes,
    /// The active code pane's highlight groups.
    Highlights,
    Placeholder,
}

impl PaneKind {
    /// Every kind, in a fixed order.
    pub const ALL: [PaneKind; 13] = [
        PaneKind::FileTree,
        PaneKind::Tour,
        PaneKind::Context,
        PaneKind::PinnedContext,
        PaneKind::Outline,
        PaneKind::CallStack,
        PaneKind::Comparison,
        PaneKind::Graph,
        PaneKind::Editor,
        PaneKind::Search,
        PaneKind::Changes,
        PaneKind::Highlights,
        PaneKind::Placeholder,
    ];

    /// Where a pane of this kind goes when nothing says otherwise (layout.md, "The window").
    pub fn default_place(self) -> DefaultPlace {
        use crate::DockId::*;
        match self {
            PaneKind::FileTree | PaneKind::Tour | PaneKind::Search | PaneKind::Changes => {
                DefaultPlace::Dock(Left)
            }
            PaneKind::Context
            | PaneKind::PinnedContext
            | PaneKind::Outline
            | PaneKind::Placeholder => DefaultPlace::Dock(Right),
            PaneKind::CallStack | PaneKind::Comparison | PaneKind::Graph | PaneKind::Highlights => {
                DefaultPlace::Dock(Bottom)
            }
            PaneKind::Editor => DefaultPlace::EditorArea,
        }
    }

    /// Pins sort after tool tabs in a tab strip (layout.md: "tool tabs first, then a divider,
    /// then pins").
    pub fn is_pin(self) -> bool {
        self == PaneKind::PinnedContext
    }
}

/// A kind's default place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultPlace {
    Dock(crate::DockId),
    EditorArea,
}

/// Frozen context: what a pin records. `byte_offset` is a UTF-8 byte offset into `path` at
/// `commit`; the viewer compares the file with `content_hash` (or, for a pin saved without
/// one, with the file at `commit`) to tell the reader when the file has changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// The symbol's display name, e.g. `SourceBuffer::utf16_to_byte`.
    pub symbol: String,
    /// The kind word shown beside it, e.g. `method`.
    pub symbol_kind: String,
    /// Repository-relative path of the file the pin was taken in.
    pub path: String,
    /// UTF-8 byte offset of the symbol in `path`.
    pub byte_offset: usize,
    /// The commit the pin was taken at.
    pub commit: String,
    /// A fingerprint of the file's text when the pin was taken ([`content_hash`]), so a change
    /// that is not committed yet is noticed too. Absent in pins saved before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

/// The fingerprint a [`Pin`] records: 64-bit FNV-1a of the bytes, as 16 hex digits. Stable
/// across runs and platforms (it is saved with the layout); not a security hash.
pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}

/// FNV-1a, 64-bit: stable across builds and platforms, unlike std's hasher. For names and
/// fingerprints that are saved, never for security.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// What a pane shows. Layout operations never look inside except to classify it; the viewer
/// owns the real content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneContent {
    FileTree,
    Tour {
        investigation: String,
        title: String,
    },
    Context,
    PinnedContext(Pin),
    Outline,
    CallStack {
        investigation: String,
        title: String,
    },
    Comparison {
        investigation: String,
        title: String,
    },
    Graph {
        query: String,
        title: String,
    },
    /// A code view. `caret` is a UTF-8 byte offset into `path`.
    Editor {
        path: String,
        caret: usize,
    },
    Search,
    Changes,
    Highlights,
    Placeholder {
        name: String,
    },
}

impl PaneContent {
    pub fn kind(&self) -> PaneKind {
        match self {
            PaneContent::FileTree => PaneKind::FileTree,
            PaneContent::Tour { .. } => PaneKind::Tour,
            PaneContent::Context => PaneKind::Context,
            PaneContent::PinnedContext(_) => PaneKind::PinnedContext,
            PaneContent::Outline => PaneKind::Outline,
            PaneContent::CallStack { .. } => PaneKind::CallStack,
            PaneContent::Comparison { .. } => PaneKind::Comparison,
            PaneContent::Graph { .. } => PaneKind::Graph,
            PaneContent::Editor { .. } => PaneKind::Editor,
            PaneContent::Search => PaneKind::Search,
            PaneContent::Changes => PaneKind::Changes,
            PaneContent::Highlights => PaneKind::Highlights,
            PaneContent::Placeholder { .. } => PaneKind::Placeholder,
        }
    }

    /// The pane's title, as its header and the focus history show it.
    pub fn title(&self) -> String {
        match self {
            PaneContent::FileTree => "Tree".into(),
            PaneContent::Tour { title, .. }
            | PaneContent::CallStack { title, .. }
            | PaneContent::Comparison { title, .. }
            | PaneContent::Graph { title, .. } => title.clone(),
            PaneContent::Context => "Context".into(),
            PaneContent::PinnedContext(pin) => pin.symbol.clone(),
            PaneContent::Outline => "Outline".into(),
            PaneContent::Editor { path, .. } => match path.rsplit('/').next() {
                Some(name) if !name.is_empty() => name.to_string(),
                // A code pane with no file open yet.
                _ => "Code".into(),
            },
            PaneContent::Search => "Search".into(),
            PaneContent::Changes => "Changes".into(),
            PaneContent::Highlights => "Highlights".into(),
            PaneContent::Placeholder { name } => name.clone(),
        }
    }

    /// What the pane is *of*, without transient state such as the caret. Focus history stores
    /// sources, so a reopened layout shows current content.
    pub fn source(&self) -> PaneSource {
        match self {
            PaneContent::FileTree => PaneSource::FileTree,
            PaneContent::Tour { investigation, .. } => PaneSource::Tour {
                investigation: investigation.clone(),
            },
            PaneContent::Context => PaneSource::Context,
            PaneContent::PinnedContext(pin) => PaneSource::Pin {
                symbol: pin.symbol.clone(),
                commit: pin.commit.clone(),
            },
            PaneContent::Outline => PaneSource::Outline,
            PaneContent::CallStack { investigation, .. } => PaneSource::CallStack {
                investigation: investigation.clone(),
            },
            PaneContent::Comparison { investigation, .. } => PaneSource::Comparison {
                investigation: investigation.clone(),
            },
            PaneContent::Graph { query, .. } => PaneSource::Graph {
                query: query.clone(),
            },
            PaneContent::Editor { path, .. } => PaneSource::Editor { path: path.clone() },
            PaneContent::Search => PaneSource::Search,
            PaneContent::Changes => PaneSource::Changes,
            PaneContent::Highlights => PaneSource::Highlights,
            PaneContent::Placeholder { name } => PaneSource::Placeholder { name: name.clone() },
        }
    }
}

/// The identity of a pane's content, as recorded in focus history.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneSource {
    FileTree,
    Tour { investigation: String },
    Context,
    Pin { symbol: String, commit: String },
    Outline,
    CallStack { investigation: String },
    Comparison { investigation: String },
    Graph { query: String },
    Editor { path: String },
    Search,
    Changes,
    Highlights,
    Placeholder { name: String },
}

impl PaneSource {
    pub fn kind(&self) -> PaneKind {
        match self {
            PaneSource::FileTree => PaneKind::FileTree,
            PaneSource::Tour { .. } => PaneKind::Tour,
            PaneSource::Context => PaneKind::Context,
            PaneSource::Pin { .. } => PaneKind::PinnedContext,
            PaneSource::Outline => PaneKind::Outline,
            PaneSource::CallStack { .. } => PaneKind::CallStack,
            PaneSource::Comparison { .. } => PaneKind::Comparison,
            PaneSource::Graph { .. } => PaneKind::Graph,
            PaneSource::Editor { .. } => PaneKind::Editor,
            PaneSource::Search => PaneKind::Search,
            PaneSource::Changes => PaneKind::Changes,
            PaneSource::Highlights => PaneKind::Highlights,
            PaneSource::Placeholder { .. } => PaneKind::Placeholder,
        }
    }

    /// A short label for lists such as Reopen focus layout.
    pub fn label(&self) -> String {
        match self {
            PaneSource::FileTree => "Tree".into(),
            PaneSource::Tour { investigation } => format!("Tour {investigation}"),
            PaneSource::Context => "Context".into(),
            PaneSource::Pin { symbol, .. } => symbol.clone(),
            PaneSource::Outline => "Outline".into(),
            PaneSource::CallStack { investigation } => format!("Call stack {investigation}"),
            PaneSource::Comparison { investigation } => format!("Comparison {investigation}"),
            PaneSource::Graph { query } => format!("Graph {query}"),
            PaneSource::Editor { path } => path.rsplit('/').next().unwrap_or(path).to_string(),
            PaneSource::Search => "Search".into(),
            PaneSource::Changes => "Changes".into(),
            PaneSource::Highlights => "Highlights".into(),
            PaneSource::Placeholder { name } => name.clone(),
        }
    }

    /// Fresh content for a source, used when reopening a focus layout whose pane was closed but
    /// whose source still exists (an editor at its start, an investigation by its id). Pins
    /// cannot be recreated from a source: the viewer reports them as missing.
    pub fn to_content(&self) -> Option<PaneContent> {
        Some(match self {
            PaneSource::FileTree => PaneContent::FileTree,
            PaneSource::Tour { investigation } => PaneContent::Tour {
                investigation: investigation.clone(),
                title: investigation.clone(),
            },
            PaneSource::Context => PaneContent::Context,
            PaneSource::Pin { .. } => return None,
            PaneSource::Outline => PaneContent::Outline,
            PaneSource::CallStack { investigation } => PaneContent::CallStack {
                investigation: investigation.clone(),
                title: investigation.clone(),
            },
            PaneSource::Comparison { investigation } => PaneContent::Comparison {
                investigation: investigation.clone(),
                title: investigation.clone(),
            },
            PaneSource::Graph { query } => PaneContent::Graph {
                query: query.clone(),
                title: query.clone(),
            },
            PaneSource::Editor { path } => PaneContent::Editor {
                path: path.clone(),
                caret: 0,
            },
            PaneSource::Search => PaneContent::Search,
            PaneSource::Changes => PaneContent::Changes,
            PaneSource::Highlights => PaneContent::Highlights,
            PaneSource::Placeholder { name } => PaneContent::Placeholder { name: name.clone() },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_is_a_tool_pane_placed_like_search() {
        assert!(PaneKind::ALL.contains(&PaneKind::Changes));
        assert_eq!(
            PaneKind::Changes.default_place(),
            PaneKind::Search.default_place()
        );
        let c = PaneContent::Changes;
        assert_eq!(c.kind(), PaneKind::Changes);
        assert_eq!(c.title(), "Changes");
        assert_eq!(c.source().to_content(), Some(PaneContent::Changes));
        assert_eq!(c.source().label(), "Changes");
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#"{"kind":"changes"}"#);
        assert_eq!(serde_json::from_str::<PaneContent>(&json).unwrap(), c);
    }
}

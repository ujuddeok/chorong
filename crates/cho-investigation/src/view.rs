//! Views (format version 2): what a step's focus page shows, never where.
//!
//! A view names panes and an arrangement template. Unknown pane kinds and arrangements
//! parse (so a file from a newer Chorong still opens) and are reported by checking; the step
//! then falls back to the default page.

use crate::Anchor;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A node id, or an anchor written in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnchorRef {
    /// The id of a node (or, where allowed, an external node).
    Node(String),
    /// An anchor written in place.
    Inline(Anchor),
}

impl AnchorRef {
    pub fn node_id(&self) -> Option<&str> {
        match self {
            AnchorRef::Node(id) => Some(id),
            AnchorRef::Inline(_) => None,
        }
    }
}

/// One highlighted chunk in an editor pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Highlight {
    pub anchor: AnchorRef,
    /// A short label (two to five words): the strip above the chunk and its row in the step's
    /// list of chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Commentary (a sentence or two), shown under the label in the list. In a file without
    /// `name`, the note is the label, as it was before `name` existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Highlight {
    /// The label and the commentary, reading an older file's lone `note` as the label.
    pub fn label_and_note(&self) -> (Option<String>, Option<String>) {
        match &self.name {
            Some(name) => (Some(name.clone()), self.note.clone()),
            None => (self.note.clone(), None),
        }
    }
}

/// Which edges a graph pane follows from its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    /// Callers: edges into the node.
    In,
    /// Callees: edges out of the node.
    Out,
    #[default]
    Both,
}

/// Deepest graph a pane may ask for.
pub const MAX_GRAPH_DEPTH: u32 = 5;

/// A pane kind this build knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Pane {
    /// Source code. No `anchor` and no `highlights` means the step's own anchor. With
    /// `highlights`, every chunk must be in one file.
    Editor {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor: Option<AnchorRef>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        highlights: Vec<Highlight>,
    },
    /// The Context pane, pinned to a declaration.
    Context { anchor: AnchorRef },
    /// The outline of one file.
    Outline { path: String },
    /// A graph around a node or anchor.
    Graph {
        of: AnchorRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<Direction>,
        /// 1 to [`MAX_GRAPH_DEPTH`]; 1 when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
    },
    /// This investigation's call stack.
    Callstack,
    /// How one file changed between two revisions: `base` (a branch, tag or commit) and
    /// `head` (one too; absent means the working tree). Shown with the comparison view.
    Diff {
        path: String,
        base: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        head: Option<String>,
    },
    /// The file tree, optionally opened at a folder.
    FileTree {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
}

impl Pane {
    pub fn kind(&self) -> &'static str {
        match self {
            Pane::Editor { .. } => "editor",
            Pane::Context { .. } => "context",
            Pane::Outline { .. } => "outline",
            Pane::Graph { .. } => "graph",
            Pane::Callstack => "callstack",
            Pane::Diff { .. } => "diff",
            Pane::FileTree { .. } => "file-tree",
        }
    }
}

/// The pane kinds this build knows.
pub const PANE_KINDS: [&str; 7] = [
    "editor",
    "context",
    "outline",
    "graph",
    "callstack",
    "diff",
    "file-tree",
];

/// A pane as written: one this build knows, or an unknown kind kept as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneSpec {
    Known(Pane),
    /// An unknown `kind`, kept so it round-trips and can be reported.
    Unknown {
        kind: String,
        raw: serde_json::Value,
    },
}

impl<'de> Deserialize<'de> for PaneSpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(d)?;
        let kind = raw
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| D::Error::custom("a pane needs a string `kind`"))?
            .to_string();
        if PANE_KINDS.contains(&kind.as_str()) {
            serde_json::from_value(raw)
                .map(PaneSpec::Known)
                .map_err(|e| D::Error::custom(format!("pane `{kind}`: {e}")))
        } else {
            Ok(PaneSpec::Unknown { kind, raw })
        }
    }
}

impl Serialize for PaneSpec {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            PaneSpec::Known(p) => p.serialize(s),
            PaneSpec::Unknown { raw, .. } => raw.serialize(s),
        }
    }
}

/// A layout template. Chorong places the panes; files never carry sizes or coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Arrangement {
    Single,
    SideBySide,
    Stacked,
    MainSide,
    Grid,
    /// An arrangement this build doesn't know, kept so it can be reported.
    Unknown(String),
}

impl Arrangement {
    pub fn as_str(&self) -> &str {
        match self {
            Arrangement::Single => "single",
            Arrangement::SideBySide => "side-by-side",
            Arrangement::Stacked => "stacked",
            Arrangement::MainSide => "main-side",
            Arrangement::Grid => "grid",
            Arrangement::Unknown(s) => s,
        }
    }

    fn from_name(s: &str) -> Self {
        match s {
            "single" => Arrangement::Single,
            "side-by-side" => Arrangement::SideBySide,
            "stacked" => Arrangement::Stacked,
            "main-side" => Arrangement::MainSide,
            "grid" => Arrangement::Grid,
            other => Arrangement::Unknown(other.to_string()),
        }
    }

    /// The arrangement used when a view names none: `main-side` for two or more panes.
    pub fn default_for(panes: usize) -> Self {
        if panes >= 2 {
            Arrangement::MainSide
        } else {
            Arrangement::Single
        }
    }
}

impl Serialize for Arrangement {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Arrangement {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Arrangement::from_name(&String::deserialize(d)?))
    }
}

/// What one step's focus page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    /// Absent: `main-side` for two or more panes, else `single`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<Arrangement>,
    pub panes: Vec<PaneSpec>,
}

impl View {
    /// Why this view can't be shown as written (an unknown pane kind or arrangement), so
    /// the step falls back to the default page. `None` when this build can show it.
    pub fn fallback_reason(&self) -> Option<String> {
        if let Some(Arrangement::Unknown(a)) = &self.arrangement {
            return Some(format!(
                "unknown arrangement `{a}`; showing the default page"
            ));
        }
        self.panes.iter().find_map(|p| match p {
            PaneSpec::Unknown { kind, .. } => Some(format!(
                "unknown pane kind `{kind}`; showing the default page"
            )),
            PaneSpec::Known(_) => None,
        })
    }

    /// The arrangement used: the written one, else the default for the pane count.
    pub fn arrangement(&self) -> Arrangement {
        self.arrangement
            .clone()
            .unwrap_or_else(|| Arrangement::default_for(self.panes.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_highlight_has_a_label_and_commentary_and_an_older_note_is_the_label() {
        let new: Highlight = serde_json::from_str(
            r#"{ "anchor": "handle", "name": "Job queued", "note": "One push per request." }"#,
        )
        .unwrap();
        assert_eq!(
            new.label_and_note(),
            (
                Some("Job queued".into()),
                Some("One push per request.".into())
            )
        );
        let old: Highlight =
            serde_json::from_str(r#"{ "anchor": "handle", "note": "Job queued" }"#).unwrap();
        assert_eq!(old.label_and_note(), (Some("Job queued".into()), None));
        let bare: Highlight = serde_json::from_str(r#"{ "anchor": "handle" }"#).unwrap();
        assert_eq!(bare.label_and_note(), (None, None));
    }
}

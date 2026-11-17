//! The graph as data: what `--json` prints and what the HTML page draws.

use crate::views::Direction;
use cho_semantic::IndexStatus;
use cho_syntax::RefKind;
use serde::Serialize;
use std::collections::BTreeMap;

/// Why a reference exists, from Tree-sitter by where the reference sits (`docs/public/graph.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Calls,
    Constructs,
    Renders,
    TypeUses,
    Implements,
    ReExports,
    References,
}

impl EdgeKind {
    pub const ALL: [EdgeKind; 7] = [
        Self::Calls,
        Self::Constructs,
        Self::Renders,
        Self::TypeUses,
        Self::Implements,
        Self::ReExports,
        Self::References,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::Constructs => "constructs",
            Self::Renders => "renders",
            Self::TypeUses => "type-uses",
            Self::Implements => "implements",
            Self::ReExports => "re-exports",
            Self::References => "references",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

impl From<RefKind> for EdgeKind {
    fn from(k: RefKind) -> Self {
        match k {
            RefKind::Calls => Self::Calls,
            RefKind::Constructs => Self::Constructs,
            RefKind::Renders => Self::Renders,
            RefKind::TypeUses => Self::TypeUses,
            RefKind::Implements => Self::Implements,
            RefKind::ReExports => Self::ReExports,
            RefKind::References => Self::References,
        }
    }
}

/// Every edge the engine produces comes from an index occurrence, so it is `Verified`.
/// `Claimed` exists for edges supplied from outside the index (an agent's overlay); the
/// engine never promotes a claim by name matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeStatus {
    Verified,
    Claimed,
    /// Every reference behind the edge uses a built declaration (`dist/x.d.ts`) that the
    /// index loader mapped to the source declaration: not recorded by the index.
    Mapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    /// A declaration, or the code of a file outside any declaration (`scip_kind` `file-level`).
    Symbol,
    File,
    Package,
    /// A third-party package, collapsed to one node.
    External,
    /// "+N more in PACKAGE", standing for nodes dropped by the cap.
    More,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    /// The containing node: symbol -> file -> package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The SCIP symbol, for declarations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// `function`, `struct`, `method`, ... (Tree-sitter outline kind) or `file-level`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scip_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// `chorong://open?...` link, with `rev`, `branch` and `dirty` from the graph's provenance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// `cho open '<link>'`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_cmd: Option<String>,
    /// GitHub or Bitbucket URL, when the repository is a remote one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
    /// Test file or test module, by path and name convention (not by coverage).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_test: bool,
    /// Part of the view's starting set.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focus: bool,
    /// Walk distance from the starting set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,
    /// For `more` nodes: how many declarations were dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<usize>,
    /// For `external` nodes whose package is also a repository package: the index had
    /// no definition for the symbol, so the edge is unresolved.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unresolved_in_repo: bool,
    /// Call sites Tree-sitter sees here that the index did not resolve. Never matched by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolved_calls: Vec<UnresolvedCall>,
}

impl Node {
    pub(crate) fn new(id: String, kind: NodeKind, label: String) -> Self {
        Self {
            id,
            kind,
            label,
            parent: None,
            symbol: None,
            scip_kind: None,
            path: None,
            line: None,
            package: None,
            link: None,
            open_cmd: None,
            web_url: None,
            is_test: false,
            focus: false,
            depth: None,
            hidden: None,
            unresolved_in_repo: false,
            unresolved_calls: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnresolvedCall {
    pub name: String,
    pub line: usize,
}

/// One kind of reference between two nodes, counted.
#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    /// How many references.
    pub count: u32,
    pub status: EdgeStatus,
    /// Of `count`, references through a mapped built declaration (unverified).
    #[serde(skip_serializing_if = "is_zero")]
    pub mapped: u32,
    /// Both ends are in a cycle (they reach each other through the edges of this graph).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub in_cycle: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Edges aggregated to files or packages: "web -> core: 48 references, 6 symbols".
#[derive(Debug, Clone, Serialize)]
pub struct Rollup {
    pub from: String,
    pub to: String,
    pub references: u32,
    /// Distinct target nodes.
    pub symbols: u32,
    pub kinds: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Rollups {
    pub file: Vec<Rollup>,
    pub package: Vec<Rollup>,
}

/// Where and when the graph was made. Paths here are never absolute.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Provenance {
    /// The repository reference used in `chorong://` links (`github:o/n`, `local:/path`).
    /// Not shown in the page header.
    #[serde(skip)]
    pub repo: String,
    /// The worktree relative to where the command ran (`.` or `sub/dir`), or its folder
    /// name. Empty when exported for sharing.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worktree: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub dirty: bool,
    /// Exported for sharing: branch, dirty state and worktree name were dropped.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
}

impl Provenance {
    /// For pages and links that leave the machine: keeps `repo` and `commit`, drops the
    /// branch, the dirty state and the worktree name (`docs/public/url-scheme.md`).
    pub fn for_sharing(self) -> Self {
        Self {
            worktree: String::new(),
            branch: None,
            dirty: false,
            shared: true,
            ..self
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub schema: &'static str,
    pub cho_graph: &'static str,
    #[serde(flatten)]
    pub provenance: Provenance,
    pub index_status: IndexStatus,
    pub limits: &'static str,
}

pub const LIMITS: &str = "Static references only, from the SCIP index. Not seen: dependency injection, event emitters, routing by string, calls on `any`, dynamic import() with a computed path. Calls through a trait point at the trait method.";

/// Which view a graph is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewKind {
    Neighbourhood,
    File,
    Package,
    OutboundClosure,
    BlastRadius,
}

impl ViewKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Neighbourhood => "neighbourhood",
            Self::File => "file",
            Self::Package => "package",
            Self::OutboundClosure => "outbound-closure",
            Self::BlastRadius => "blast-radius",
        }
    }

    /// The level the page opens at.
    pub fn default_level(self) -> Level {
        match self {
            Self::Neighbourhood => Level::Symbol,
            Self::File | Self::Package | Self::OutboundClosure | Self::BlastRadius => {
                Level::Package
            }
        }
    }
}

/// A roll-up level of the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    Symbol,
    File,
    Package,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewInfo {
    pub kind: ViewKind,
    pub title: String,
    pub focus: Vec<String>,
    pub direction: Direction,
    pub depth: Option<usize>,
    pub edge_kinds: Vec<EdgeKind>,
    /// The level the page opens at.
    pub default_level: Level,
    pub max_nodes: usize,
    /// The node cap dropped declarations (see the `more` nodes).
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Graph {
    pub meta: Meta,
    pub view: ViewInfo,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub rollups: Rollups,
    /// Test files that reference the changed code (blast radius). Reachability, not coverage.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reachable_tests: Vec<String>,
    /// Plain statements about what this graph can and cannot say.
    pub notes: Vec<String>,
}

impl Graph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("graph serialises")
    }
}

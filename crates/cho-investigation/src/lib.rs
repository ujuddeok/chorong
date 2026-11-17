//! The investigation format (`docs/public/investigation-format.md`).
//!
//! An investigation is *claims about code*, usually written by an LLM or an
//! agent skill. This crate parses and structurally validates it. Checking each
//! claim against the code (and labelling it verified, claimed or broken) is
//! done by the semantic layer, using [`Verification`].

pub mod check;
pub mod check_v2;
pub mod compare;
pub mod diff;
pub mod drift;
#[cfg(feature = "fetch")]
mod fetch;
pub mod listing;
pub mod recents;
pub mod session;
pub mod source;
pub mod view;

pub use compare::{Cell, Override, Phase, Row, RowResult, Side, Variant};
pub use view::{AnchorRef, Arrangement, Direction, Highlight, Pane, PaneSpec, View};

use cho_core::RepoRef;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;

/// The format version agents are told to write (the skill's version). Version 1 files
/// still load.
pub const FORMAT_VERSION: u32 = 2;

/// The newest format version this crate reads. Every version from 1 up to this one loads.
pub const LATEST_FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// An ordered call chain, outermost caller first.
    Callstack,
    /// An ordered reading tour through related code.
    Tour,
    /// An unordered graph of nodes and edges.
    Diagram,
    /// Grouping, labels and notes layered over Chorong's own PR fact graph.
    PrOverlay,
    /// Two or more paths with their steps aligned by job (format version 2).
    Comparison,
}

/// A place in the code. Names a file and, usually, a declaration in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// Repository-relative path.
    pub path: String,
    /// Declaration name, qualified as the language writes it
    /// (`Queue::push`, `Queue.push`). Absent means the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// A short, exact piece of source inside the anchor, used to find the
    /// precise spot and to detect drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    /// One-based line hint, used only when `symbol` is absent or fails to resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// Version 2: exact text where a range starts, inside the declaration (or file).
    /// Given together with `to`, instead of `excerpt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Version 2: exact text where the range ends, searched after `from`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub anchor: Anchor,
    /// Display label. Defaults to the symbol name, or the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Markdown explanation of this node's role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Version 2: a short step name, e.g. "Verify signature".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Version 2: how the author knows this; kept apart from Chorong's own label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
    /// Version 2: runtime values at this step (`flag = false`, `refused`), as the author
    /// saw them. Never verified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

/// How the author knows a node's claim. Chorong shows it beside its own label and never
/// upgrades it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Evidence {
    /// Read in the code.
    Read,
    /// Inferred (from names, docs, behaviour), not read.
    Inferred,
}

impl Evidence {
    /// The word shown beside a node's label.
    pub fn word(self) -> &'static str {
        match self {
            Evidence::Read => "read",
            Evidence::Inferred => "inferred",
        }
    }
}

/// What an external node stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalKind {
    Model,
    Database,
    Service,
    Vendor,
    Platform,
    Limit,
    Other,
}

/// Version 2: something on the path that isn't code (a model, a database, a vendor, a
/// platform limit). Has no anchor, is always shown as "not in the code", never verified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct External {
    /// Shares the node id namespace.
    pub id: String,
    pub what: ExternalKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

/// Version 2, `tour`: a step that is not code: an instruction for the reader, with
/// commands to copy. Chorong never runs a command; it shows each with a Copy button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    /// Shares the node id namespace.
    pub id: String,
    /// A short step name, e.g. "Run the migration".
    pub name: String,
    /// Markdown instruction, shown inert.
    pub note: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<Command>,
}

/// A command an action step asks the reader to run. Shown as text with a Copy button;
/// never run by Chorong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// A short label above the command, e.g. "Apply".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The exact text copied (may span lines).
    pub text: String,
}

/// Version 2: a call or step the author could not follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Untraced {
    /// What could not be traced, e.g. "call through `hooks.dispatch`".
    pub what: String,
    /// Where it happens, if known; checked like any anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<AnchorRef>,
    /// Why it could not be traced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Relation {
    /// `from` calls `to` directly.
    Calls,
    /// `from` reaches `to` indirectly: callback, trait object, registration, dispatch.
    Dispatches,
    /// `from` refers to `to` without calling it.
    References,
    /// `from` implements or overrides `to`.
    Implements,
    /// Data produced in `from` flows to `to`.
    DataFlow,
    /// `from` contains `to` (module, type, file).
    Contains,
    /// Anything else; explain in `note`.
    Related,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub rel: Relation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub label: String,
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Version 2: `setup` steps are numbered S1, S2…; `call` steps 1, 2….
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
}

/// Which pull request a `pr-overlay` describes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequest {
    pub number: u64,
    /// Merge-base commit (Chorong's "Before").
    pub base: String,
    /// Head commit (Chorong's "After"). Must equal the investigation's `rev`.
    pub head: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Investigation {
    /// Format version.
    #[serde(rename = "chorong")]
    pub cho: u32,
    pub title: String,
    pub kind: Kind,
    /// Repository, in `chorong://` repo syntax (`github:owner/name`, …).
    pub repo: String,
    /// The full commit every anchor refers to.
    pub rev: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PullRequest>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
    /// Reading order for `callstack` and `tour`: node ids, first to last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// Who or what produced this, e.g. `"callstack-skill 0.3"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<String>,
    /// Version 2: summary bullets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<String>,
    /// Version 2: nodes that aren't code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub externals: Vec<External>,
    /// Version 2, `tour`: steps that are instructions, not code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    /// Version 2: what the author could not trace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untraced: Vec<Untraced>,
    /// Version 2, `tour` and `callstack`: step (node) id to what its focus page shows.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub views: BTreeMap<String, View>,
    /// Version 2, `comparison`: the paths compared.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sides: Vec<Side>,
    /// Version 2, `comparison`: steps aligned by job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Row>,
    /// Version 2, `tour`: branches, each overriding the steps it names (by step id).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
}

/// Whether a claim held up against the code. Assigned by Chorong, never by the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verification {
    /// The index confirms it (e.g. a SCIP reference from `from`'s body to `to`).
    Verified,
    /// The anchors resolve, but Chorong can't confirm the relationship.
    Claimed,
    /// An anchor doesn't resolve at the pinned revision.
    Broken,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("not valid investigation JSON: {0}")]
    Json(String),
    #[error(
        "unsupported format version {0} (this build reads versions 1 to {LATEST_FORMAT_VERSION})"
    )]
    Version(u32),
    #[error("{0}")]
    Invalid(String),
}

/// Id sets shared by the validation helpers.
struct Names<'a> {
    /// Code nodes: the only places that have code to show.
    code: HashSet<&'a str>,
    /// Code nodes, external nodes and action steps.
    ids: HashSet<&'a str>,
    actions: HashSet<&'a str>,
}

fn invalid<T>(msg: String) -> Result<T, FormatError> {
    Err(FormatError::Invalid(msg))
}

impl Names<'_> {
    /// Places that are part of the graph: never an action step.
    fn in_graph(&self, id: &str, field: &str) -> Result<(), FormatError> {
        if self.actions.contains(id) {
            Err(FormatError::Invalid(format!(
                "{field} names action `{id}`; actions are steps, not part of the graph"
            )))
        } else {
            Ok(())
        }
    }

    fn known(&self, id: &str, field: &str) -> Result<(), FormatError> {
        if self.ids.contains(id) {
            Ok(())
        } else {
            Err(FormatError::Invalid(format!(
                "{field} refers to unknown node `{id}`"
            )))
        }
    }

    /// Places that need code: an external node has nothing to show in an editor.
    fn anchor_ref(&self, r: &AnchorRef, field: &str) -> Result<(), FormatError> {
        match r {
            AnchorRef::Node(id) if self.code.contains(id.as_str()) => Ok(()),
            AnchorRef::Node(id) if self.actions.contains(id.as_str()) => Err(FormatError::Invalid(
                format!("{field} names action `{id}`, which has no code"),
            )),
            AnchorRef::Node(id) if self.ids.contains(id.as_str()) => Err(FormatError::Invalid(
                format!("{field} names external node `{id}`, which has no code"),
            )),
            AnchorRef::Node(id) => Err(FormatError::Invalid(format!(
                "{field} refers to unknown node `{id}`"
            ))),
            AnchorRef::Inline(a) => validate_anchor(a, field),
        }
    }
}

impl Investigation {
    /// Parses and validates an investigation file.
    pub fn parse(json: &str) -> Result<Self, FormatError> {
        // Check the version first so a future file gets a useful message
        // rather than an "unknown field" error.
        #[derive(Deserialize)]
        struct VersionOnly {
            #[serde(rename = "chorong")]
            cho: u32,
        }
        let version: VersionOnly =
            serde_json::from_str(json).map_err(|e| FormatError::Json(e.to_string()))?;
        if !(1..=LATEST_FORMAT_VERSION).contains(&version.cho) {
            return Err(FormatError::Version(version.cho));
        }
        let inv: Investigation =
            serde_json::from_str(json).map_err(|e| FormatError::Json(e.to_string()))?;
        inv.validate()?;
        Ok(inv)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("investigation serialises")
    }

    /// Structural checks that need no access to the code.
    pub fn validate(&self) -> Result<(), FormatError> {
        if !(1..=LATEST_FORMAT_VERSION).contains(&self.cho) {
            return Err(FormatError::Version(self.cho));
        }
        if self.cho < 2
            && let Some(field) = self.v2_field()
        {
            return invalid(format!("{field} needs format version 2 (`\"chorong\": 2`)"));
        }
        if self.title.trim().is_empty() {
            return invalid("`title` must not be empty".into());
        }
        if let Err(e) = RepoRef::parse(&self.repo) {
            return invalid(format!("`repo`: {e}"));
        }
        if !is_commit(&self.rev) {
            return invalid(format!(
                "`rev` must be a commit hash (7–64 hex digits), got `{}`",
                self.rev
            ));
        }
        if self.nodes.is_empty() {
            return invalid("an investigation needs at least one node".into());
        }
        let names = self.check_ids()?;
        self.check_edges_steps_groups(&names)?;
        self.check_views(&names)?;
        self.check_untraced_and_points(&names)?;
        self.check_comparison(&names)?;
        self.check_branches(&names)?;
        self.check_kind_needs()
    }

    /// Node, external and action ids: one namespace.
    fn check_ids(&self) -> Result<Names<'_>, FormatError> {
        // Code nodes, then external nodes: one id namespace.
        let mut code = HashSet::new();
        for node in &self.nodes {
            if node.id.is_empty() || !code.insert(node.id.as_str()) {
                return invalid(format!("node id `{}` is empty or duplicated", node.id));
            }
            validate_anchor(&node.anchor, &format!("node `{}`", node.id))?;
            not_blank(node.name.as_deref(), &format!("node `{}`: `name`", node.id))?;
            values_ok(&node.values, &format!("node `{}`", node.id))?;
        }
        let mut ids = code.clone();
        for ext in &self.externals {
            if ext.id.is_empty() || !ids.insert(ext.id.as_str()) {
                return invalid(format!("node id `{}` is empty or duplicated", ext.id));
            }
            if ext.label.trim().is_empty() {
                return invalid(format!("external `{}`: `label` must not be blank", ext.id));
            }
            not_blank(
                ext.name.as_deref(),
                &format!("external `{}`: `name`", ext.id),
            )?;
            values_ok(&ext.values, &format!("external `{}`", ext.id))?;
        }
        // Action steps: in the same namespace, but only steps, groups, views and tour
        // branches may name them; they are never part of the graph.
        if !self.actions.is_empty() && self.kind != Kind::Tour {
            return invalid("`actions` are for `tour` steps".into());
        }
        let mut actions = HashSet::new();
        for a in &self.actions {
            if a.id.is_empty() || !ids.insert(a.id.as_str()) {
                return invalid(format!("node id `{}` is empty or duplicated", a.id));
            }
            actions.insert(a.id.as_str());
            let at = format!("action `{}`", a.id);
            if a.name.trim().is_empty() || a.note.trim().is_empty() {
                return invalid(format!("{at}: `name` and `note` must not be blank"));
            }
            for (i, c) in a.commands.iter().enumerate() {
                if c.text.trim().is_empty() {
                    return invalid(format!("{at} command {}: `text` must not be blank", i + 1));
                }
                not_blank(
                    c.label.as_deref(),
                    &format!("{at} command {}: `label`", i + 1),
                )?;
            }
        }
        Ok(Names { code, ids, actions })
    }

    /// Edges, steps and groups name known graph nodes.
    fn check_edges_steps_groups(&self, names: &Names) -> Result<(), FormatError> {
        for edge in &self.edges {
            names.known(&edge.from, "edge `from`")?;
            names.known(&edge.to, "edge `to`")?;
            names.in_graph(&edge.from, "edge `from`")?;
            names.in_graph(&edge.to, "edge `to`")?;
        }
        for step in &self.steps {
            names.known(step, "`steps`")?;
        }
        for group in &self.groups {
            if group.members.is_empty() {
                return invalid(format!("group `{}` has no members", group.label));
            }
            for m in &group.members {
                names.known(m, &format!("group `{}`", group.label))?;
            }
        }
        Ok(())
    }

    /// Per-step views.
    fn check_views(&self, names: &Names) -> Result<(), FormatError> {
        if !self.views.is_empty() && !matches!(self.kind, Kind::Tour | Kind::Callstack) {
            return invalid("`views` are for `tour` and `callstack` steps".into());
        }
        for (step, view) in &self.views {
            if !self.steps.contains(step) {
                return invalid(format!("`views` names `{step}`, which is not a step"));
            }
            if view.panes.is_empty() {
                return invalid(format!("view `{step}` has no panes"));
            }
            for (i, pane) in view.panes.iter().enumerate() {
                let at = format!("view `{step}` pane {}", i + 1);
                let PaneSpec::Known(pane) = pane else {
                    continue; // reported by check; the step falls back to the default page
                };
                match pane {
                    Pane::Editor { anchor, highlights } => {
                        if anchor.is_none()
                            && highlights.is_empty()
                            && names.actions.contains(step.as_str())
                        {
                            return invalid(format!(
                                "{at}: an action step has no code; give the editor an `anchor`"
                            ));
                        }
                        if anchor.is_some() && !highlights.is_empty() {
                            return invalid(format!(
                                "{at}: give `anchor` or `highlights`, not both"
                            ));
                        }
                        if let Some(a) = anchor {
                            names.anchor_ref(a, &at)?;
                        }
                        for (j, h) in highlights.iter().enumerate() {
                            names.anchor_ref(&h.anchor, &format!("{at} highlight {}", j + 1))?;
                        }
                    }
                    Pane::Context { anchor } => names.anchor_ref(anchor, &at)?,
                    Pane::Outline { path } => {
                        if !is_relative_path(path) {
                            return invalid(format!(
                                "{at}: path `{path}` must be repository-relative"
                            ));
                        }
                    }
                    Pane::Graph { of, depth, .. } => {
                        match of {
                            AnchorRef::Node(id) => {
                                names.known(id, &at)?;
                                names.in_graph(id, &at)?;
                            }
                            AnchorRef::Inline(a) => validate_anchor(a, &at)?,
                        }
                        if depth.is_some_and(|d| !(1..=view::MAX_GRAPH_DEPTH).contains(&d)) {
                            return invalid(format!(
                                "{at}: `depth` must be 1 to {}",
                                view::MAX_GRAPH_DEPTH
                            ));
                        }
                    }
                    Pane::Diff { path, base, head } => {
                        if !is_relative_path(path) {
                            return invalid(format!(
                                "{at}: path `{path}` must be repository-relative"
                            ));
                        }
                        for (field, rev) in [("base", Some(base)), ("head", head.as_ref())] {
                            if let Some(rev) = rev
                                && !diff::is_rev_name(rev)
                            {
                                return invalid(format!(
                                    "{at}: `{field}` `{rev}` must name a branch, tag or commit"
                                ));
                            }
                        }
                    }
                    Pane::FileTree { path: Some(path) } if !is_relative_path(path) => {
                        return invalid(format!("{at}: path `{path}` must be repository-relative"));
                    }
                    Pane::Callstack | Pane::FileTree { .. } => {}
                }
            }
        }
        Ok(())
    }

    /// Untraced notes and summary points.
    fn check_untraced_and_points(&self, names: &Names) -> Result<(), FormatError> {
        for (i, u) in self.untraced.iter().enumerate() {
            let at = format!("`untraced` entry {}", i + 1);
            if u.what.trim().is_empty() {
                return invalid(format!("{at}: `what` must not be blank"));
            }
            if let Some(r) = &u.at {
                names.anchor_ref(r, &at)?;
            }
        }
        for p in &self.points {
            if p.trim().is_empty() {
                return invalid("`points` must not hold blank bullets".into());
            }
        }
        Ok(())
    }

    /// Comparison sides and rows.
    fn check_comparison(&self, names: &Names) -> Result<(), FormatError> {
        let comparison = self.kind == Kind::Comparison;
        if !comparison && (!self.sides.is_empty() || !self.rows.is_empty()) {
            return invalid("`sides` and `rows` are for `comparison`".into());
        }
        let mut side_ids = HashSet::new();
        let mut row_ids = HashSet::new();
        for row in &self.rows {
            if row.id.is_empty() || !row_ids.insert(row.id.as_str()) {
                return invalid(format!("row id `{}` is empty or duplicated", row.id));
            }
        }
        for side in &self.sides {
            if side.id.is_empty() || !side_ids.insert(side.id.as_str()) {
                return invalid(format!("side id `{}` is empty or duplicated", side.id));
            }
            if side.path.is_empty() {
                return invalid(format!("side `{}` has an empty `path`", side.id));
            }
            for id in &side.path {
                names.known(id, &format!("side `{}` path", side.id))?;
            }
            let mut variant_ids = HashSet::new();
            for v in &side.variants {
                let at = format!("side `{}` variant `{}`", side.id, v.id);
                if v.id.is_empty() || !variant_ids.insert(v.id.as_str()) {
                    return invalid(format!("{at}: id is empty or duplicated"));
                }
                for (row, o) in &v.rows {
                    if !row_ids.contains(row.as_str()) {
                        return invalid(format!("{at} overrides unknown row `{row}`"));
                    }
                    one_of(
                        o.step.as_deref(),
                        o.missing.as_deref(),
                        &format!("{at} row `{row}`"),
                    )?;
                    if let Some(s) = &o.step {
                        names.known(s, &format!("{at} row `{row}`"))?;
                    }
                    values_ok(&o.values, &format!("{at} row `{row}`"))?;
                }
            }
        }
        for row in &self.rows {
            let at = format!("row `{}`", row.id);
            if row.name.trim().is_empty() {
                return invalid(format!("{at}: `name` must not be blank"));
            }
            for side in &self.sides {
                if !row.cells.contains_key(&side.id) {
                    return invalid(format!(
                        "{at} has no cell for side `{}` (give a `step` or a `missing` reason)",
                        side.id
                    ));
                }
            }
            for (side, cell) in &row.cells {
                if !side_ids.contains(side.as_str()) {
                    return invalid(format!("{at} has a cell for unknown side `{side}`"));
                }
                let at = format!("{at} side `{side}`");
                one_of(cell.step.as_deref(), cell.missing.as_deref(), &at)?;
                if let Some(s) = &cell.step {
                    names.known(s, &at)?;
                }
                values_ok(&cell.values, &at)?;
            }
        }
        Ok(())
    }

    /// Tour branches.
    fn check_branches(&self, names: &Names) -> Result<(), FormatError> {
        if !self.variants.is_empty() && self.kind != Kind::Tour {
            return invalid(
                "top-level `variants` are tour branches; a comparison's go on its sides".into(),
            );
        }
        let mut branch_ids = HashSet::new();
        for v in &self.variants {
            let at = format!("branch `{}`", v.id);
            if v.id.is_empty() || !branch_ids.insert(v.id.as_str()) {
                return invalid(format!("{at}: id is empty or duplicated"));
            }
            for (step, o) in &v.rows {
                if !self.steps.contains(step) {
                    return invalid(format!("{at} overrides `{step}`, which is not a step"));
                }
                let at = format!("{at} step `{step}`");
                one_of(o.step.as_deref(), o.missing.as_deref(), &at)?;
                if let Some(s) = &o.step {
                    names.known(s, &at)?;
                }
                if o.result.is_some() || o.callout.is_some() {
                    return invalid(format!(
                        "{at}: `result` and `callout` are for comparison rows"
                    ));
                }
                values_ok(&o.values, &at)?;
            }
        }
        Ok(())
    }

    /// What each kind needs to be useful.
    fn check_kind_needs(&self) -> Result<(), FormatError> {
        match self.kind {
            Kind::Callstack | Kind::Tour if self.steps.is_empty() => {
                invalid("`callstack` and `tour` need `steps` (node ids in reading order)".into())
            }
            Kind::Comparison if self.sides.len() < 2 || self.rows.is_empty() => invalid(
                "`comparison` needs two or more `sides` and at least one row in `rows`".into(),
            ),
            Kind::PrOverlay => match &self.pr {
                None => invalid("`pr-overlay` needs `pr` with number, base and head".into()),
                Some(pr) if !is_commit(&pr.base) || !is_commit(&pr.head) => {
                    invalid("`pr.base` and `pr.head` must be commit hashes".into())
                }
                Some(pr) if pr.head != self.rev => invalid(
                    "`pr.head` must equal `rev`: overlay anchors refer to the PR head".into(),
                ),
                Some(_) => Ok(()),
            },
            _ => Ok(()),
        }
    }

    /// The first field that needs format version 2, if any.
    fn v2_field(&self) -> Option<&'static str> {
        let anchor_v2 = |a: &Anchor| a.from.is_some() || a.to.is_some();
        if self.kind == Kind::Comparison {
            return Some("`kind: comparison`");
        }
        [
            (!self.points.is_empty(), "`points`"),
            (!self.externals.is_empty(), "`externals`"),
            (!self.actions.is_empty(), "`actions`"),
            (!self.untraced.is_empty(), "`untraced`"),
            (!self.views.is_empty(), "`views`"),
            (!self.sides.is_empty(), "`sides`"),
            (!self.rows.is_empty(), "`rows`"),
            (!self.variants.is_empty(), "`variants`"),
            (
                self.groups.iter().any(|g| g.phase.is_some()),
                "group `phase`",
            ),
            (self.nodes.iter().any(|n| n.name.is_some()), "node `name`"),
            (
                self.nodes.iter().any(|n| n.evidence.is_some()),
                "node `evidence`",
            ),
            (
                self.nodes.iter().any(|n| !n.values.is_empty()),
                "node `values`",
            ),
            (
                self.nodes.iter().any(|n| anchor_v2(&n.anchor)),
                "anchor `from`/`to`",
            ),
        ]
        .into_iter()
        .find_map(|(used, name)| used.then_some(name))
    }

    /// The action step with this id.
    pub fn action(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.id == id)
    }

    /// The node or external node with this id: `Ok` for code, `Err` for an external.
    pub fn lookup(&self, id: &str) -> Option<Result<&Node, &External>> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .map(Ok)
            .or_else(|| self.externals.iter().find(|e| e.id == id).map(Err))
    }
}

fn is_relative_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.split('/').any(|c| c.is_empty() || c == "." || c == "..")
}

fn validate_anchor(a: &Anchor, at: &str) -> Result<(), FormatError> {
    let invalid = |msg: String| Err(FormatError::Invalid(msg));
    if !is_relative_path(&a.path) {
        return invalid(format!(
            "{at}: path `{}` must be repository-relative",
            a.path
        ));
    }
    if a.symbol.as_deref().is_some_and(|s| s.trim().is_empty()) {
        return invalid(format!("{at}: `symbol` must not be blank"));
    }
    if a.line == Some(0) {
        return invalid(format!("{at}: `line` is one-based"));
    }
    match (&a.from, &a.to) {
        (None, None) => Ok(()),
        (Some(f), Some(t)) if f.is_empty() || t.is_empty() => {
            invalid(format!("{at}: `from` and `to` must not be empty"))
        }
        (Some(_), Some(_)) if a.excerpt.is_some() => {
            invalid(format!("{at}: give `excerpt` or `from` and `to`, not both"))
        }
        (Some(_), Some(_)) => Ok(()),
        _ => invalid(format!("{at}: `from` and `to` go together")),
    }
}

fn not_blank(s: Option<&str>, at: &str) -> Result<(), FormatError> {
    match s {
        Some(s) if s.trim().is_empty() => {
            Err(FormatError::Invalid(format!("{at} must not be blank")))
        }
        _ => Ok(()),
    }
}

fn values_ok(values: &[String], at: &str) -> Result<(), FormatError> {
    if values.iter().any(|v| v.trim().is_empty()) {
        return Err(FormatError::Invalid(format!(
            "{at}: `values` must not hold blank entries"
        )));
    }
    Ok(())
}

fn one_of(step: Option<&str>, missing: Option<&str>, at: &str) -> Result<(), FormatError> {
    match (step, missing) {
        (Some(_), None) => Ok(()),
        (None, Some(m)) if !m.trim().is_empty() => Ok(()),
        _ => Err(FormatError::Invalid(format!(
            "{at}: give a `step` or a non-blank `missing` reason, not both"
        ))),
    }
}

fn is_commit(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> serde_json::Value {
        serde_json::json!({
            "chorong": 1,
            "title": "t",
            "kind": "callstack",
            "repo": "github:acme/api",
            "rev": "3f9c1e2",
            "nodes": [
                { "id": "a", "anchor": { "path": "src/a.rs", "symbol": "a" } },
                { "id": "b", "anchor": { "path": "src/b.rs", "symbol": "B::b" } }
            ],
            "edges": [ { "from": "a", "to": "b", "rel": "calls" } ],
            "steps": ["a", "b"]
        })
    }

    fn parse(v: &serde_json::Value) -> Result<Investigation, FormatError> {
        Investigation::parse(&v.to_string())
    }

    #[test]
    fn minimal_parses_and_round_trips() {
        let inv = parse(&minimal()).unwrap();
        assert_eq!(inv.kind, Kind::Callstack);
        assert_eq!(Investigation::parse(&inv.to_json()).unwrap(), inv);
    }

    #[test]
    fn rejects_structural_errors() {
        type Mutation = Box<dyn Fn(&mut serde_json::Value)>;
        let cases: Vec<(&str, Mutation)> = vec![
            (
                "dangling edge",
                Box::new(|v| v["edges"][0]["to"] = "zz".into()),
            ),
            ("dangling step", Box::new(|v| v["steps"][1] = "zz".into())),
            (
                "duplicate id",
                Box::new(|v| v["nodes"][1]["id"] = "a".into()),
            ),
            (
                "absolute path",
                Box::new(|v| v["nodes"][0]["anchor"]["path"] = "/etc/x".into()),
            ),
            (
                "dotdot path",
                Box::new(|v| v["nodes"][0]["anchor"]["path"] = "a/../b".into()),
            ),
            ("branch rev", Box::new(|v| v["rev"] = "main".into())),
            ("bad repo", Box::new(|v| v["repo"] = "gitlab:a/b".into())),
            ("no steps", Box::new(|v| v["steps"] = serde_json::json!([]))),
            (
                "zero line",
                Box::new(|v| v["nodes"][0]["anchor"]["line"] = 0.into()),
            ),
            (
                "overlay without pr",
                Box::new(|v| v["kind"] = "pr-overlay".into()),
            ),
        ];
        for (name, mutate) in cases {
            let mut v = minimal();
            mutate(&mut v);
            assert!(matches!(parse(&v), Err(FormatError::Invalid(_))), "{name}");
        }
    }

    #[test]
    fn rejects_unknown_fields_and_versions() {
        let mut v = minimal();
        v["nodes"][0]["anchor"]["coords"] = serde_json::json!([1, 2]);
        assert!(matches!(parse(&v), Err(FormatError::Json(_))));
        let mut v = minimal();
        v["chorong"] = 3.into();
        assert_eq!(parse(&v), Err(FormatError::Version(3)));
    }

    fn mixed_tour() -> serde_json::Value {
        let mut v = minimal();
        v["chorong"] = 2.into();
        v["kind"] = "tour".into();
        v["actions"] = serde_json::json!([
            { "id": "run", "name": "Run the migration", "note": "Then read on.",
              "commands": [ { "label": "Apply", "text": "make migrate" }, { "text": "ls" } ] }
        ]);
        v["steps"] = serde_json::json!(["a", "run", "b"]);
        v["views"] = serde_json::json!({
            "a": { "panes": [ { "kind": "diff", "path": "src/a.rs", "base": "main" } ] },
            "run": { "panes": [ { "kind": "editor", "anchor": "b" } ] }
        });
        v
    }

    #[test]
    fn action_steps_and_diff_panes_parse_in_version_2_only() {
        let inv = parse(&mixed_tour()).unwrap();
        assert_eq!(inv.action("run").unwrap().commands.len(), 2);
        assert!(inv.lookup("run").is_none(), "an action is not a node");
        assert_eq!(Investigation::parse(&inv.to_json()).unwrap(), inv);
        let mut v1 = mixed_tour();
        v1["chorong"] = 1.into();
        v1["views"] = serde_json::json!({});
        assert!(matches!(parse(&v1), Err(FormatError::Invalid(m)) if m.contains("`actions`")));
    }

    #[test]
    fn action_steps_and_diff_panes_reject_misuse() {
        type Mutation = Box<dyn Fn(&mut serde_json::Value)>;
        let cases: Vec<(&str, Mutation)> = vec![
            (
                "edge to an action",
                Box::new(|v| v["edges"][0]["to"] = "run".into()),
            ),
            ("not a tour", Box::new(|v| v["kind"] = "callstack".into())),
            (
                "blank note",
                Box::new(|v| v["actions"][0]["note"] = " ".into()),
            ),
            (
                "blank command",
                Box::new(|v| v["actions"][0]["commands"][1]["text"] = "".into()),
            ),
            (
                "id shared with a node",
                Box::new(|v| v["actions"][0]["id"] = "a".into()),
            ),
            (
                "editor at an action",
                Box::new(|v| v["views"]["run"]["panes"][0]["anchor"] = "run".into()),
            ),
            (
                "editor with no anchor on an action",
                Box::new(|v| v["views"]["run"]["panes"][0] = serde_json::json!({"kind": "editor"})),
            ),
            (
                "graph of an action",
                Box::new(|v| {
                    v["views"]["run"]["panes"][0] =
                        serde_json::json!({"kind": "graph", "of": "run"})
                }),
            ),
            (
                "diff base as an option",
                Box::new(|v| v["views"]["a"]["panes"][0]["base"] = "--output=x".into()),
            ),
            (
                "diff head range",
                Box::new(|v| v["views"]["a"]["panes"][0]["head"] = "a..b".into()),
            ),
            (
                "diff absolute path",
                Box::new(|v| v["views"]["a"]["panes"][0]["path"] = "/etc/passwd".into()),
            ),
        ];
        for (name, mutate) in cases {
            let mut v = mixed_tour();
            mutate(&mut v);
            assert!(
                matches!(parse(&v), Err(FormatError::Invalid(_))),
                "{name}: {:?}",
                parse(&v)
            );
        }
        let mut v = mixed_tour();
        v["actions"][0]["commands"][0]["run"] = true.into();
        assert!(
            matches!(parse(&v), Err(FormatError::Json(_))),
            "unknown field"
        );
    }

    #[test]
    fn pr_overlay_head_must_match_rev() {
        let mut v = minimal();
        v["kind"] = "pr-overlay".into();
        v["pr"] = serde_json::json!({ "number": 7, "base": "aaaaaaa", "head": "bbbbbbb" });
        assert!(matches!(parse(&v), Err(FormatError::Invalid(_))));
        v["pr"]["head"] = "3f9c1e2".into();
        assert!(parse(&v).is_ok());
    }
}

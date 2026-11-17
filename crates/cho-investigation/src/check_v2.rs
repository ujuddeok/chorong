//! Checking the version 2 parts of an investigation: views, steps (numbers, depth,
//! "called by"), external nodes, the untraced list, comparisons and tour branches.
//!
//! Everything here is either **derived** from the code and the check report (labelled
//! verified or claimed by Chorong) or **written** by the author and passed through as the
//! author's claim. Chorong never upgrades a written claim.

use crate::check::{CheckReport, NodeReport, NodeStatus, SourceReader, check_node};
use crate::compare::{Cell, Phase, RowResult, Variant};
use crate::view::{AnchorRef, Arrangement, Direction, Pane, PaneSpec, View};
use crate::{Anchor, Evidence, ExternalKind, Investigation, Kind, Relation, Verification};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ops::Range;
use std::path::Path;

/// The version 2 sections of a [`CheckReport`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct V2Report {
    /// `tour` and `callstack`: every step, with its number, name, depth and callers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepReport>,
    /// Each written view, resolved. Steps without one get the default page.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<ViewReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub externals: Vec<ExternalReport>,
    /// Action steps: instructions, never verified or broken.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub untraced: Vec<UntracedReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<ComparisonReport>,
    /// Tour branches: the steps each one changes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<BranchReport>,
    /// Everything check reports about the version 2 parts, in file order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
}

impl V2Report {
    /// Broken anchors in views and in the untraced list, and diff panes that can't be shown.
    pub fn broken_anchors(&self) -> usize {
        let views: usize = self
            .views
            .iter()
            .filter(|v| v.fallback.is_none())
            .flat_map(|v| &v.panes)
            .map(PaneReport::broken_anchors)
            .sum();
        let untraced = self
            .untraced
            .iter()
            .filter(|u| {
                u.at.as_ref()
                    .is_some_and(|a| a.status == NodeStatus::Broken)
            })
            .count();
        views + untraced
    }
}

/// An anchor resolved for a pane, highlight or untraced entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnchorReport {
    /// The node id, when the anchor was written as one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Display label: the node's label, else the symbol, else the file name.
    pub label: String,
    pub path: String,
    pub status: NodeStatus,
    /// UTF-8 byte range shown (the excerpt or `from`…`to` range, else the region).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range<usize>>,
    /// The declaration, line or file the anchor covers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Range<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_range: Option<Range<usize>>,
    /// One-based line of `range.start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
}

/// Where the caret lands for an anchor's parts: an excerpt's or range's start (when it is
/// not the whole region), else the declared name's start, else the range's start.
pub fn caret_of(
    range: Option<&Range<usize>>,
    region: Option<&Range<usize>>,
    name_start: Option<usize>,
) -> Option<usize> {
    match range {
        Some(r) if Some(r) != region => Some(r.start),
        r => name_start.or(r.map(|r| r.start)),
    }
}

impl AnchorReport {
    /// Where the caret lands: an excerpt's start, else the declared name, else the region.
    pub fn caret(&self) -> Option<usize> {
        caret_of(
            self.range.as_ref(),
            self.region.as_ref(),
            self.name_range.as_ref().map(|r| r.start),
        )
    }

    /// The declared name's start, else the range's start.
    pub fn name_or_range_start(&self) -> Option<usize> {
        self.name_range
            .as_ref()
            .or(self.range.as_ref())
            .map(|r| r.start)
    }

    fn from_node(node: Option<&str>, label: String, r: &NodeReport) -> Self {
        Self {
            node: node.map(str::to_string),
            label,
            path: r.path.clone(),
            status: r.status.clone(),
            range: r.range.clone(),
            region: r.region.clone(),
            name_range: r.name_range.clone(),
            line: r.line,
            messages: r.messages.clone(),
        }
    }
}

/// One highlighted chunk of an editor pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HighlightReport {
    /// One-based, in pane order: the number on the chunk's note strip.
    pub number: usize,
    /// The label on the chunk's strip (an older file's lone `note`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Commentary under the label in the step's list of chunks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub anchor: AnchorReport,
}

/// A pane, resolved. A broken anchor stays as a placeholder pane with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PaneReport {
    Editor {
        /// The file shown: the first highlight's.
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        highlights: Vec<HighlightReport>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        messages: Vec<String>,
    },
    Context {
        anchor: AnchorReport,
    },
    Outline {
        anchor: AnchorReport,
    },
    Graph {
        /// The node the graph is around, when written as a node id.
        #[serde(skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        /// True when `node` is an external node (no code to anchor).
        external: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        anchor: Option<AnchorReport>,
        direction: Direction,
        depth: u32,
    },
    Callstack,
    /// One file between two revisions.
    Diff {
        path: String,
        base: crate::diff::RevReport,
        head: crate::diff::RevReport,
        /// Why the diff can't be shown (an unresolved revision, or the file on neither
        /// side); the pane then shows a card saying so.
        #[serde(skip_serializing_if = "Option::is_none")]
        problem: Option<String>,
    },
    FileTree {
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
}

impl PaneReport {
    fn broken_anchors(&self) -> usize {
        let broken = |a: &AnchorReport| usize::from(a.status == NodeStatus::Broken);
        match self {
            PaneReport::Editor { highlights, .. } => {
                highlights.iter().map(|h| broken(&h.anchor)).sum()
            }
            PaneReport::Context { anchor } | PaneReport::Outline { anchor } => broken(anchor),
            PaneReport::Graph {
                anchor: Some(a), ..
            } => broken(a),
            PaneReport::Diff { problem, .. } => usize::from(problem.is_some()),
            _ => 0,
        }
    }
}

/// What one step's focus page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewReport {
    /// The step (node) id.
    pub step: String,
    /// The arrangement used.
    pub arrangement: String,
    /// True when this is the default page (no view written, or a fallback).
    pub default: bool,
    /// Why the written view is not shown (an unknown pane kind or arrangement).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    pub panes: Vec<PaneReport>,
}

/// How deep a step is: call edges from the first step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Depth {
    /// 0 for the first step.
    pub n: usize,
    /// Verified only when every call edge on the path is verified.
    pub verification: Verification,
}

/// A `calls` or `dispatches` edge into a step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Caller {
    pub id: String,
    pub rel: Relation,
    pub verification: Verification,
}

/// One step of a `tour` or `callstack`, with what Chorong derives for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepReport {
    pub index: usize,
    pub id: String,
    /// "S1", "S2"… for setup steps; "1", "2"… for the rest.
    pub number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    /// The short step name, as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// True for an external node: not in the code.
    pub external: bool,
    /// True for an action step: an instruction, not code.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub action: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_evidence: Option<Evidence>,
    /// Runtime values, as the author saw them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Derived from call edges; `None` when no call edges reach the step from the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<Depth>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub called_by: Vec<Caller>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalReport {
    pub id: String,
    pub what: ExternalKind,
    pub label: String,
    /// Always `not-in-code`: never verified.
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_evidence: Option<Evidence>,
}

/// An action step: what the reader is asked to do. Never verified or broken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionReport {
    pub id: String,
    pub name: String,
    /// Always `action`.
    pub status: &'static str,
    /// How many commands it offers to copy.
    pub commands: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UntracedReport {
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<AnchorReport>,
}

/// Whether a row's result is Chorong's or the author's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResultSource {
    /// Chorong derived it: a side is missing, or every side anchors the same declaration and
    /// the author wrote no result and no values.
    Derived,
    /// The author's claim.
    Claimed,
    /// No result: not derivable and not written.
    None,
}

/// One side's entry in a row, resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CellReport {
    pub side: String,
    /// The variant this cell comes from; `None` for the side's base path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    /// True when `step` is an external node.
    pub external: bool,
    /// The step's anchor, resolved, when it is code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowReport {
    pub id: String,
    pub name: String,
    /// "S1", "S2"… for setup rows; "1", "2"… for the rest.
    pub number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    /// One per side, in side order.
    pub cells: Vec<CellReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<RowResult>,
    pub result_source: ResultSource,
    /// Every side anchors the same declaration (a fact; the result may still be
    /// `differs`, by values).
    pub same_code: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VariantInfo {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SideReport {
    pub id: String,
    pub label: String,
    pub path: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<VariantInfo>,
}

/// The rows one variant changes, resolved with that variant chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VariantRows {
    pub side: String,
    pub variant: String,
    pub rows: Vec<RowReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComparisonReport {
    pub sides: Vec<SideReport>,
    /// Every row with no variant chosen.
    pub rows: Vec<RowReport>,
    /// For each variant, the rows it overrides.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<VariantRows>,
}

/// One step as a tour branch sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchStep {
    pub index: usize,
    /// The step id in `steps`.
    pub step: String,
    /// The node shown under the branch; `None` when not reached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub reached: bool,
    /// Why the step is not reached under this branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchReport {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The steps this branch changes, in step order.
    pub overrides: Vec<BranchStep>,
}

// ---- Checking ----

/// Checks the version 2 parts. `base` holds the node and edge reports.
pub fn check(inv: &Investigation, base: &CheckReport, reader: &dyn SourceReader) -> V2Report {
    let mut out = V2Report::default();
    let mut ctx = Ctx {
        inv,
        base,
        reader,
        messages: vec![],
    };

    if matches!(inv.kind, Kind::Tour | Kind::Callstack) {
        out.steps = step_reports(inv, base, &mut ctx.messages);
        // In step order; a step listed twice has one view.
        let mut seen = std::collections::HashSet::new();
        for step in &inv.steps {
            if let Some(view) = inv.views.get(step)
                && seen.insert(step.as_str())
            {
                let report = ctx.view(step, Some(view));
                out.views.push(report);
            }
        }
    }
    out.externals = inv
        .externals
        .iter()
        .map(|e| ExternalReport {
            id: e.id.clone(),
            what: e.what,
            label: e.label.clone(),
            status: "not-in-code",
            author_evidence: e.evidence,
        })
        .collect();
    out.actions = inv
        .actions
        .iter()
        .map(|a| ActionReport {
            id: a.id.clone(),
            name: a.name.clone(),
            status: "action",
            commands: a.commands.len(),
        })
        .collect();
    out.untraced = inv
        .untraced
        .iter()
        .map(|u| UntracedReport {
            what: u.what.clone(),
            why: u.why.clone(),
            at: u.at.as_ref().map(|r| ctx.anchor(r)),
        })
        .collect();
    if inv.kind == Kind::Comparison {
        out.comparison = Some(comparison(inv, base, &mut ctx.messages));
    }
    out.branches = inv
        .variants
        .iter()
        .map(|v| BranchReport {
            id: v.id.clone(),
            label: v.label.clone(),
            note: v.note.clone(),
            overrides: branch_steps(inv, Some(v))
                .into_iter()
                .filter(|s| v.rows.contains_key(&s.step))
                .collect(),
        })
        .collect();
    out.messages = ctx.messages;
    out
}

struct Ctx<'a> {
    inv: &'a Investigation,
    base: &'a CheckReport,
    reader: &'a dyn SourceReader,
    messages: Vec<String>,
}

impl Ctx<'_> {
    /// Resolves a node id (from the base report) or an inline anchor (read now).
    fn anchor(&self, r: &AnchorRef) -> AnchorReport {
        match r {
            AnchorRef::Node(id) => node_anchor(self.inv, self.base, id),
            AnchorRef::Inline(a) => {
                let report = check_node("", a, self.reader);
                AnchorReport::from_node(None, anchor_label(a), &report)
            }
        }
    }

    /// The page for `step`: the written view, or the default page.
    fn view(&mut self, step: &str, view: Option<&View>) -> ViewReport {
        let fallback = view.and_then(View::fallback_reason);
        if let Some(reason) = &fallback {
            self.messages.push(format!("view `{step}`: {reason}"));
        }
        match view {
            Some(view) if fallback.is_none() => {
                let panes = view
                    .panes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| match p {
                        PaneSpec::Known(p) => Some(self.pane(step, i, p)),
                        PaneSpec::Unknown { .. } => None,
                    })
                    .collect();
                ViewReport {
                    step: step.into(),
                    arrangement: view.arrangement().as_str().into(),
                    default: false,
                    fallback: None,
                    panes,
                }
            }
            _ => {
                let mut page = default_page(self.inv, self.base, step);
                page.fallback = fallback;
                page
            }
        }
    }

    fn pane(&mut self, step: &str, index: usize, pane: &Pane) -> PaneReport {
        let at = format!("view `{step}` pane {}", index + 1);
        match pane {
            Pane::Editor { anchor, highlights } => {
                let mut list: Vec<HighlightReport> = if highlights.is_empty() {
                    let r = anchor.clone().unwrap_or(AnchorRef::Node(step.to_string()));
                    vec![HighlightReport {
                        number: 1,
                        name: None,
                        note: None,
                        anchor: self.anchor(&r),
                    }]
                } else {
                    highlights
                        .iter()
                        .enumerate()
                        .map(|(i, h)| HighlightReport {
                            number: i + 1,
                            name: h.label_and_note().0,
                            note: h.label_and_note().1,
                            anchor: self.anchor(&h.anchor),
                        })
                        .collect()
                };
                let path = list.first().map(|h| h.anchor.path.clone());
                let mut messages = vec![];
                if list.iter().any(|h| Some(&h.anchor.path) != path.as_ref()) {
                    let m = format!(
                        "{at}: highlights are in more than one file; an editor pane shows one file (`{}`)",
                        path.as_deref().unwrap_or_default()
                    );
                    self.messages.push(m.clone());
                    messages.push(m);
                }
                for h in &mut list {
                    if h.anchor.status == NodeStatus::Broken {
                        self.messages.push(format!(
                            "{at} highlight {}: {}",
                            h.number,
                            h.anchor.messages.first().map_or("broken", String::as_str)
                        ));
                    }
                }
                PaneReport::Editor {
                    path,
                    highlights: list,
                    messages,
                }
            }
            Pane::Context { anchor } => PaneReport::Context {
                anchor: self.noted(&at, self.anchor(anchor)),
            },
            Pane::Outline { path } => {
                let a = Anchor {
                    path: path.clone(),
                    symbol: None,
                    excerpt: None,
                    line: None,
                    from: None,
                    to: None,
                };
                PaneReport::Outline {
                    anchor: self.noted(&at, self.anchor(&AnchorRef::Inline(a))),
                }
            }
            Pane::Graph {
                of,
                direction,
                depth,
            } => {
                let external = of
                    .node_id()
                    .is_some_and(|id| self.inv.externals.iter().any(|e| e.id == id));
                let anchor = (!external).then(|| self.anchor(of));
                let anchor = anchor.map(|a| self.noted(&at, a));
                PaneReport::Graph {
                    node: of.node_id().map(str::to_string),
                    external,
                    anchor,
                    direction: direction.unwrap_or_default(),
                    depth: depth.unwrap_or(1),
                }
            }
            Pane::Callstack => PaneReport::Callstack,
            Pane::Diff { path, base, head } => {
                let texts = match self.reader.root() {
                    Some(root) => crate::diff::read(root, path, base, head.as_deref()),
                    None => {
                        let side = |rev: Option<&String>| crate::diff::RevReport {
                            rev: rev.cloned(),
                            commit: None,
                            has_file: false,
                            error: Some("no repository to read it from".into()),
                        };
                        crate::diff::DiffTexts {
                            base: side(Some(base)),
                            head: side(head.as_ref()),
                            base_bytes: None,
                            head_bytes: None,
                        }
                    }
                };
                let problem = texts.problem(path);
                if let Some(p) = &problem {
                    self.messages.push(format!("{at}: {p}"));
                }
                PaneReport::Diff {
                    path: path.clone(),
                    base: texts.base,
                    head: texts.head,
                    problem,
                }
            }
            Pane::FileTree { path } => PaneReport::FileTree { path: path.clone() },
        }
    }

    /// Records a broken pane anchor in the messages.
    fn noted(&mut self, at: &str, a: AnchorReport) -> AnchorReport {
        if a.status == NodeStatus::Broken {
            self.messages.push(format!(
                "{at}: {}",
                a.messages.first().map_or("broken", String::as_str)
            ));
        }
        a
    }
}

/// The default page for a step: an editor at the step's anchor plus Context. An external
/// or action step has no code, so its page has no panes.
pub fn default_page(inv: &Investigation, base: &CheckReport, step: &str) -> ViewReport {
    let panes = match inv.lookup(step) {
        Some(Ok(_)) => {
            let a = node_anchor(inv, base, step);
            vec![
                PaneReport::Editor {
                    path: Some(a.path.clone()),
                    highlights: vec![HighlightReport {
                        number: 1,
                        name: None,
                        note: None,
                        anchor: a.clone(),
                    }],
                    messages: vec![],
                },
                PaneReport::Context { anchor: a },
            ]
        }
        _ => vec![],
    };
    ViewReport {
        step: step.into(),
        arrangement: Arrangement::default_for(panes.len()).as_str().into(),
        default: true,
        fallback: None,
        panes,
    }
}

fn node_anchor(inv: &Investigation, base: &CheckReport, id: &str) -> AnchorReport {
    match (inv.nodes.iter().find(|n| n.id == id), base.node(id)) {
        (Some(node), Some(r)) => {
            AnchorReport::from_node(Some(id), crate::session::node_title(node), r)
        }
        _ => AnchorReport {
            node: Some(id.into()),
            label: id.into(),
            path: String::new(),
            status: NodeStatus::Broken,
            range: None,
            region: None,
            name_range: None,
            line: None,
            messages: vec![format!("`{id}` is not a code node")],
        },
    }
}

fn anchor_label(a: &Anchor) -> String {
    a.symbol.clone().unwrap_or_else(|| {
        Path::new(&a.path)
            .file_name()
            .map_or(a.path.clone(), |f| f.to_string_lossy().into_owned())
    })
}

// ---- Steps: numbers, depth, called by ----

/// The phase of each id, from groups. Conflicts are reported.
fn phases(inv: &Investigation, messages: &mut Vec<String>) -> HashMap<String, Phase> {
    let mut out: HashMap<String, Phase> = HashMap::new();
    for g in &inv.groups {
        let Some(p) = g.phase else { continue };
        for m in &g.members {
            match out.get(m) {
                Some(q) if *q != p => messages.push(format!(
                    "`{m}` is in both a setup group and a call group; numbered as {}",
                    phase_word(*q)
                )),
                Some(_) => {}
                None => {
                    out.insert(m.clone(), p);
                }
            }
        }
    }
    out
}

fn phase_word(p: Phase) -> &'static str {
    match p {
        Phase::Setup => "setup",
        Phase::Call => "call",
    }
}

/// Numbers a sequence of phases: setup items S1, S2…, the rest 1, 2….
pub fn numbers(phases: impl Iterator<Item = Option<Phase>>) -> Vec<String> {
    let (mut setup, mut call) = (0, 0);
    phases
        .map(|p| match p {
            Some(Phase::Setup) => {
                setup += 1;
                format!("S{setup}")
            }
            _ => {
                call += 1;
                call.to_string()
            }
        })
        .collect()
}

fn step_reports(
    inv: &Investigation,
    base: &CheckReport,
    messages: &mut Vec<String>,
) -> Vec<StepReport> {
    let phase_of = phases(inv, messages);
    let nums = numbers(inv.steps.iter().map(|s| phase_of.get(s).copied()));
    let call_like = |r: Relation| matches!(r, Relation::Calls | Relation::Dispatches);
    let verification = |i: usize| {
        base.edges
            .get(i)
            .map_or(Verification::Claimed, |e| e.verification)
    };
    let depths = inv.steps.first().map(|root| depths(inv, base, root));
    inv.steps
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let (name, evidence, values, external) = match inv.lookup(id) {
                Some(Ok(n)) => (n.name.clone(), n.evidence, n.values.clone(), false),
                Some(Err(e)) => (e.name.clone(), e.evidence, e.values.clone(), true),
                None => (inv.action(id).map(|a| a.name.clone()), None, vec![], false),
            };
            let called_by = inv
                .edges
                .iter()
                .enumerate()
                .filter(|(_, e)| &e.to == id && call_like(e.rel))
                .map(|(i, e)| Caller {
                    id: e.from.clone(),
                    rel: e.rel,
                    verification: verification(i),
                })
                .collect();
            StepReport {
                index,
                id: id.clone(),
                number: nums[index].clone(),
                phase: phase_of.get(id).copied(),
                name,
                external,
                action: inv.action(id).is_some(),
                author_evidence: evidence,
                values,
                depth: depths.as_ref().and_then(|d| d.get(id.as_str()).copied()),
                called_by,
            }
        })
        .collect()
}

/// Shortest call-edge distance from `root` to every node it reaches. Verified only when a
/// shortest path uses verified edges alone.
fn depths<'a>(
    inv: &'a Investigation,
    base: &CheckReport,
    root: &'a str,
) -> HashMap<&'a str, Depth> {
    let bfs = |verified_only: bool| -> HashMap<&'a str, usize> {
        let mut dist = HashMap::from([(root, 0usize)]);
        let mut queue = VecDeque::from([root]);
        while let Some(at) = queue.pop_front() {
            let d = dist[at];
            for (i, e) in inv.edges.iter().enumerate() {
                if e.from != at || !matches!(e.rel, Relation::Calls | Relation::Dispatches) {
                    continue;
                }
                let v = base
                    .edges
                    .get(i)
                    .map_or(Verification::Claimed, |r| r.verification);
                if v == Verification::Broken || (verified_only && v != Verification::Verified) {
                    continue;
                }
                if !dist.contains_key(e.to.as_str()) {
                    dist.insert(e.to.as_str(), d + 1);
                    queue.push_back(e.to.as_str());
                }
            }
        }
        dist
    };
    let verified = bfs(true);
    // The first step anchors the count: verified only when it resolves to code.
    let root_ok = base
        .node(root)
        .is_some_and(|n| n.status == NodeStatus::Resolved);
    bfs(false)
        .into_iter()
        .map(|(id, n)| {
            let verification = if root_ok && verified.get(id) == Some(&n) {
                Verification::Verified
            } else {
                Verification::Claimed
            };
            (id, Depth { n, verification })
        })
        .collect()
}

// ---- Comparisons ----

fn comparison(
    inv: &Investigation,
    base: &CheckReport,
    messages: &mut Vec<String>,
) -> ComparisonReport {
    let rows = resolve_rows(inv, base, &BTreeMap::new());
    // Path order is about the base paths only; variants may bring in other steps.
    let mut rows_out = rows;
    for side in &inv.sides {
        let mut last: Option<(usize, &str)> = None;
        for (row, report) in inv.rows.iter().zip(rows_out.iter_mut()) {
            let Some(step) = row.cells.get(&side.id).and_then(|c| c.step.as_deref()) else {
                continue;
            };
            match side.path.iter().position(|p| p == step) {
                None => report
                    .messages
                    .push(format!("side `{}`: `{step}` is not on its path", side.id)),
                Some(pos) => {
                    if let Some((prev, prev_row)) = last
                        && pos < prev
                    {
                        report.messages.push(format!(
                            "side `{}`: `{step}` comes before row `{prev_row}`'s step on the path",
                            side.id
                        ));
                    }
                    last = Some((pos, row.id.as_str()));
                }
            }
        }
    }
    let variants = inv
        .sides
        .iter()
        .flat_map(|side| side.variants.iter().map(move |v| (side, v)))
        .map(|(side, v)| {
            let choice = BTreeMap::from([(side.id.clone(), v.id.clone())]);
            VariantRows {
                side: side.id.clone(),
                variant: v.id.clone(),
                rows: resolve_rows(inv, base, &choice)
                    .into_iter()
                    .filter(|r| v.rows.contains_key(&r.id))
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    for r in rows_out
        .iter()
        .chain(variants.iter().flat_map(|v| v.rows.iter()))
    {
        for m in &r.messages {
            messages.push(format!("row `{}`: {m}", r.id));
        }
    }
    ComparisonReport {
        sides: inv
            .sides
            .iter()
            .map(|s| SideReport {
                id: s.id.clone(),
                label: s.label.clone(),
                path: s.path.clone(),
                variants: s
                    .variants
                    .iter()
                    .map(|v| VariantInfo {
                        id: v.id.clone(),
                        label: v.label.clone(),
                        note: v.note.clone(),
                    })
                    .collect(),
            })
            .collect(),
        rows: rows_out,
        variants,
    }
}

/// Every row of a comparison, with `choices` (side id to variant id) applied.
pub fn resolve_rows(
    inv: &Investigation,
    base: &CheckReport,
    choices: &BTreeMap<String, String>,
) -> Vec<RowReport> {
    let nums = numbers(inv.rows.iter().map(|r| r.phase));
    inv.rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut result = row.result;
            let mut callout = row.callout.clone();
            let cells: Vec<CellReport> = inv
                .sides
                .iter()
                .map(|side| {
                    let variant = choices
                        .get(&side.id)
                        .and_then(|v| side.variants.iter().find(|x| &x.id == v));
                    let over = variant.and_then(|v| v.rows.get(&row.id).map(|o| (v, o)));
                    let (cell, from_variant) = match over {
                        Some((v, o)) => {
                            // A variant cell that is missing doesn't inherit the base
                            // row's written result: the row is derived `missing`.
                            if o.result.is_some() || o.missing.is_some() {
                                result = o.result;
                            }
                            if o.callout.is_some() {
                                callout = o.callout.clone();
                            }
                            (o.cell(), Some(v.id.clone()))
                        }
                        None => (row.cells.get(&side.id).cloned().unwrap_or_default(), None),
                    };
                    cell_report(inv, base, &side.id, from_variant, &cell)
                })
                .collect();
            let mut messages = vec![];
            let (result, result_source, same_code) = row_result(&cells, result, &mut messages);
            RowReport {
                id: row.id.clone(),
                name: row.name.clone(),
                number: nums[i].clone(),
                phase: row.phase,
                cells,
                result,
                result_source,
                same_code,
                callout,
                note: row.note.clone(),
                messages,
            }
        })
        .collect()
}

fn cell_report(
    inv: &Investigation,
    base: &CheckReport,
    side: &str,
    variant: Option<String>,
    cell: &Cell,
) -> CellReport {
    let external = cell
        .step
        .as_deref()
        .is_some_and(|s| matches!(inv.lookup(s), Some(Err(_))));
    CellReport {
        side: side.into(),
        variant,
        step: cell.step.clone(),
        missing: cell.missing.clone(),
        external,
        anchor: cell
            .step
            .as_deref()
            .filter(|_| !external)
            .map(|s| node_anchor(inv, base, s)),
        values: cell.values.clone(),
        note: cell.note.clone(),
    }
}

/// A row's result. `missing` (a side has no step) is structural and always wins. Otherwise
/// the author's result is taken, as a claim. Only with no written result and no runtime
/// values does every side anchoring the same declaration derive `same`: two sides often run
/// the same code with different values, and then "differs" is the author's point.
fn row_result(
    cells: &[CellReport],
    written: Option<RowResult>,
    messages: &mut Vec<String>,
) -> (Option<RowResult>, ResultSource, bool) {
    let word = |r: RowResult| match r {
        RowResult::Same => "same",
        RowResult::Differs => "differs",
        RowResult::Missing => "missing",
    };
    let places: Vec<_> = cells
        .iter()
        .map(|c| {
            c.anchor
                .as_ref()
                .filter(|a| a.status == NodeStatus::Resolved)
                .map(|a| (a.path.as_str(), a.region.clone()))
        })
        .collect();
    let same_code = !places.is_empty()
        && places.iter().all(Option::is_some)
        && places.windows(2).all(|w| w[0] == w[1]);
    let missing = cells.iter().any(|c| c.missing.is_some());
    let has_values = cells.iter().any(|c| !c.values.is_empty());
    let (result, source) = match written {
        _ if missing => {
            if let Some(w) = written.filter(|w| *w != RowResult::Missing) {
                messages.push(format!(
                    "written result `{}`, but a side has no step; showing `missing`",
                    word(w)
                ));
            }
            (Some(RowResult::Missing), ResultSource::Derived)
        }
        Some(RowResult::Missing) => {
            messages.push("written result `missing`, but every side has a step".into());
            (Some(RowResult::Missing), ResultSource::Claimed)
        }
        Some(w) => {
            if same_code && w == RowResult::Differs {
                messages.push("same code; differs by values".into());
            }
            (Some(w), ResultSource::Claimed)
        }
        None if same_code && !has_values => (Some(RowResult::Same), ResultSource::Derived),
        None => {
            let resolved = places.iter().all(Option::is_some);
            messages.push(if same_code {
                "no result: same code with runtime values, so give `result`".into()
            } else if resolved {
                "no result: the sides anchor different code, so give `result`".into()
            } else {
                "no result: not every side's step resolved to code, so give `result`".into()
            });
            (None, ResultSource::None)
        }
    };
    (result, source, same_code)
}

// ---- Tour branches ----

/// Every step under `branch` (`None` for the main path).
pub fn branch_steps(inv: &Investigation, branch: Option<&Variant>) -> Vec<BranchStep> {
    inv.steps
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let over = branch.and_then(|b| b.rows.get(id));
            match over {
                Some(o) => BranchStep {
                    index,
                    step: id.clone(),
                    node: o.step.clone(),
                    reached: o.step.is_some(),
                    missing: o.missing.clone(),
                    values: o.values.clone(),
                    note: o.note.clone(),
                },
                None => BranchStep {
                    index,
                    step: id.clone(),
                    node: Some(id.clone()),
                    reached: true,
                    missing: None,
                    values: vec![],
                    note: None,
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_steps_are_numbered_apart_from_call_steps() {
        let p = [
            Some(Phase::Setup),
            Some(Phase::Setup),
            Some(Phase::Call),
            None,
        ];
        assert_eq!(numbers(p.into_iter()), ["S1", "S2", "1", "2"]);
        assert_eq!(numbers([None, None].into_iter()), ["1", "2"]);
    }
}

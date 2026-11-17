//! Investigations in the viewer: the seam between the viewer and `cho-investigation`.
//!
//! Everything the panes need and nothing GPUI: reading an investigation (the backend's
//! [`Source`] and bounded [`source::read`]), checking it against the working tree or the
//! pinned commit ([`check_at`]), the rows the Tour and Call stack panes show ([`Step`]), the
//! picker's listing (the backend's `listing` plus recents from its [`RecentsStore`]), the file
//! stamp live reload compares, and notes as inert text ([`note_blocks`]).
//!
//! Investigation files are untrusted (design/investigations-viewer.md, "Security"): nothing in
//! one runs anything, paths stay inside the repository (the format refuses `/` and `..`, and
//! anchored files are read with `cho_core::fs::read_source_in` or from git), URL fetches are
//! the backend's bounded https fetch, and labels come from Chorong. Positions are UTF-8 byte
//! offsets.

use cho_investigation::check::{
    CheckReport, EdgeOracle, NodeStatus, WorktreeReader, check_with_reader,
};
use cho_investigation::drift::CommitReader;
pub use cho_investigation::drift::Drift;
pub use cho_investigation::recents::{Recent, RecentsStore};
use cho_investigation::source;
pub use cho_investigation::source::Source;
use cho_investigation::{Investigation, Kind};
use std::path::{Path, PathBuf};

/// The text a source is remembered by in layouts and links: its path or URL.
pub fn source_key(source: &Source) -> String {
    source.to_string()
}

/// What a row shows as its source: the host for a URL, the path (relative to `root` when
/// inside it) for a file.
pub fn origin(source: &Source, root: Option<&Path>) -> String {
    match source {
        Source::Url(_) => source.host().unwrap_or_default(),
        Source::Path(p) => root
            .and_then(|r| p.strip_prefix(r).ok())
            .map(|r| r.display().to_string())
            .unwrap_or_else(|| p.display().to_string()),
    }
}

/// Reads and parses an investigation: a file with the safe read (inside `root` when it is
/// there), a URL with the backend's bounded https fetch. Never on the UI thread.
pub fn read(source: &Source, root: Option<&Path>) -> Result<Investigation, String> {
    read_with_host(source, root).map(|(inv, _)| inv)
}

/// [`read`], with the host a URL was finally fetched from (after redirects), which is the
/// host the reader is shown.
pub fn read_with_host(
    source: &Source,
    root: Option<&Path>,
) -> Result<(Investigation, Option<String>), String> {
    let fetched = source::read(source, root).map_err(|e| e.to_string())?;
    let host = fetched
        .final_url
        .as_deref()
        .and_then(source::host_of)
        .or_else(|| source.host());
    let inv = source::parse_bytes(&fetched.bytes).map_err(|e| match source {
        Source::Path(p) => format!("{}: {e}", p.display()),
        Source::Url(_) => format!("{}: {e}", host.clone().unwrap_or_default()),
    })?;
    Ok((inv, host))
}

/// What live reload compares: the file's length and modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

/// The file's stamp (`None` for a URL, which is not watched, or a file that is gone). One
/// `stat`, no read; a change is then confirmed by reading and parsing.
pub fn stamp(source: &Source) -> Option<Stamp> {
    let Source::Path(p) = source else { return None };
    let m = std::fs::metadata(p).ok()?;
    Some(Stamp {
        len: m.len(),
        modified: m.modified().ok(),
    })
}

/// Which code the labels describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeAt {
    /// The files as they are in the working folder.
    WorkingTree,
    /// The pinned commit, read from git (read-only). The full commit id.
    Pinned(String),
}

/// The checkout against the investigation's `rev` (two or three `git rev-parse` calls).
pub fn drift(root: &Path, rev: &str) -> Drift {
    cho_investigation::drift::drift(root, rev)
}

/// The full id of the pinned commit, when this repository has it.
pub fn pinned_commit(root: &Path, rev: &str) -> Option<String> {
    cho_investigation::drift::resolve_commit(root, rev)
}

/// Checks `inv` against the chosen code. The working tree uses `oracle` (the semantic index,
/// built from the working tree); the pinned commit is read from git with no index for that
/// commit here, so its edges can be claimed at best.
pub fn check_at(
    inv: &Investigation,
    root: &Path,
    at: &CodeAt,
    oracle: Option<&dyn EdgeOracle>,
) -> CheckReport {
    match at {
        CodeAt::WorkingTree => check_with_reader(inv, &WorktreeReader { root }, oracle),
        CodeAt::Pinned(rev) => match CommitReader::new(root, rev) {
            Ok(reader) => check_with_reader(inv, &reader, None),
            // The commit went away meanwhile: every anchor reads as broken, with the reason.
            Err(e) => check_with_reader(inv, &Unreadable(e), None),
        },
    }
}

struct Unreadable(String);

impl cho_investigation::check::SourceReader for Unreadable {
    fn read(&self, _: &str) -> Result<Vec<u8>, String> {
        Err(self.0.clone())
    }
}

/// A verification label, as the Call stack pane shows it. It differs by shape as well as
/// colour (design/layout.md, "Call stacks and graphs").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Label {
    Verified,
    Claimed,
    Broken,
}

impl Label {
    pub fn word(self) -> &'static str {
        match self {
            Label::Verified => "verified",
            Label::Claimed => "claimed",
            Label::Broken => "broken",
        }
    }

    fn of(status: &str) -> Label {
        match status {
            "verified" => Label::Verified,
            "broken" => Label::Broken,
            _ => Label::Claimed,
        }
    }
}

/// One stop of a tour or frame of a call stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub id: String,
    /// One-based position.
    pub n: usize,
    /// The node's label, else its symbol, else its file name.
    pub title: String,
    pub symbol: Option<String>,
    /// Repository-relative path.
    pub path: String,
    pub label: Label,
    /// Why the label is what it is, when it is not verified (shown in place of the location
    /// for a broken step, and as a tooltip otherwise).
    pub reason: Option<String>,
    /// Where the anchor resolved (an excerpt, or the declaration), and the whole region it
    /// covers. `None` when broken.
    pub range: Option<std::ops::Range<usize>>,
    pub region: Option<std::ops::Range<usize>>,
    /// Start of the declared name, for a symbol anchor.
    pub name_start: Option<usize>,
    /// One-based line of `range.start`.
    pub line: Option<usize>,
    /// Markdown, shown inert.
    pub note: Option<String>,
    /// What the step shows as its number: `3`, or `S1` for a setup step.
    pub number: String,
    /// What format version 2 adds (short name, phase, evidence, runtime values, external
    /// node, view); empty for version 1.
    pub extras: crate::tour_pages::StepExtras,
}

impl Step {
    /// A node that is not in the code: no location and never a verification chip.
    pub fn is_external(&self) -> bool {
        self.extras.external.is_some()
    }

    /// An action step: an instruction with commands to copy, not code.
    pub fn is_action(&self) -> bool {
        self.extras.action.is_some()
    }

    /// No code to open and no verification label: an external node or an action.
    pub fn no_code(&self) -> bool {
        self.is_external() || self.is_action()
    }

    /// The name lists show: the short name when the step has one.
    pub fn list_name(&self) -> &str {
        self.extras.name.as_deref().unwrap_or(&self.title)
    }

    pub fn file_name(&self) -> &str {
        file_name(&self.path)
    }

    /// Where the caret lands: an excerpt's start, else the declared name, else the region.
    pub fn caret(&self) -> Option<usize> {
        self.range.as_ref()?;
        cho_investigation::check_v2::caret_of(
            self.range.as_ref(),
            self.region.as_ref(),
            self.name_start,
        )
    }

    /// `buffer.rs · SourceBuffer::new`, or `buffer.rs:42` for a line anchor.
    pub fn location(&self) -> String {
        match (&self.symbol, self.line) {
            (Some(s), _) => format!("{} \u{b7} {s}", self.file_name()),
            (None, Some(l)) => format!("{}:{l}", self.file_name()),
            (None, None) => self.file_name().to_string(),
        }
    }
}

/// The last component of a `/`-separated path.
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Where the code was read, in reasons: `at 04e63f8` or `in the working tree`.
pub fn at_words(at: &CodeAt) -> String {
    match at {
        CodeAt::WorkingTree => "in the working tree".into(),
        CodeAt::Pinned(rev) => format!("at {}", cho_git::short(rev)),
    }
}

/// The steps of `inv` in reading order, labelled from `report`.
///
/// A step whose anchor is broken is broken. Otherwise, in a call stack, the first frame is
/// verified when its anchor names exactly one declaration, and each later frame takes the
/// label of the edge from the frame before it into it: verified only when the index confirms
/// it, claimed when the file has no such edge. An ambiguous anchor is claimed at best. A tour
/// stop's label is its anchor's: verified when it resolves uniquely.
#[cfg(test)]
pub fn steps(inv: &Investigation, report: &CheckReport, at: &CodeAt) -> Vec<Step> {
    steps_on(inv, report, at, None)
}

/// The steps of `inv` under a tour branch (`None` for the main path): a step the branch
/// replaces shows the node put in its place (keeping its slot's id, so places and pages still
/// find it); a step it does not reach is marked so (`StepExtras::not_reached`), never dropped.
/// Version 2's extras are applied by [`with_extras`] and then [`apply_branch`].
pub fn steps_on(
    inv: &Investigation,
    report: &CheckReport,
    at: &CodeAt,
    branch: Option<&cho_investigation::compare::Variant>,
) -> Vec<Step> {
    let words = at_words(at);
    cho_investigation::check_v2::branch_steps(inv, branch)
        .into_iter()
        .filter_map(|b| {
            let i = b.index;
            let slot = &b.step;
            // Not reached: the slot's own node, for its name, muted by `apply_branch`.
            let id = b.node.as_ref().unwrap_or(slot);
            let mut step = step_of(inv, report, &words, i, id)?;
            step.id = slot.clone();
            Some(step)
        })
        .collect()
}

/// A step with no anchor and no label of Chorong's (an action or an external node): shown
/// without one.
fn unanchored_step(
    id: &str,
    i: usize,
    title: String,
    note: Option<String>,
    extras: crate::tour_pages::StepExtras,
) -> Step {
    Step {
        id: id.to_string(),
        n: i + 1,
        title,
        symbol: None,
        path: String::new(),
        label: Label::Claimed,
        reason: None,
        range: None,
        region: None,
        name_start: None,
        line: None,
        note,
        number: (i + 1).to_string(),
        extras,
    }
}

/// An instruction: no anchor, with its card.
fn action_step(a: &cho_investigation::Action, id: &str, i: usize) -> Step {
    let extras = crate::tour_pages::StepExtras {
        action: Some(action_card(a)),
        ..Default::default()
    };
    unanchored_step(id, i, a.name.clone(), Some(a.note.clone()), extras)
}

/// A node that is not in the code.
fn external_step(ext: &cho_investigation::External, id: &str, i: usize) -> Step {
    let extras = crate::tour_pages::StepExtras {
        external: Some(external_word(&ext.what).to_string()),
        ..Default::default()
    };
    unanchored_step(id, i, ext.label.clone(), ext.note.clone(), extras)
}

/// A node in the code, labelled from its check (and, in a call stack, the edge into it).
fn code_step(
    inv: &Investigation,
    report: &CheckReport,
    words: &str,
    i: usize,
    id: &String,
    node: &cho_investigation::Node,
) -> Option<Step> {
    let checked = report.nodes.iter().find(|n| &n.id == id)?;
    let first_message = checked.messages.first().cloned();
    let (mut label, mut reason) = match checked.status {
        NodeStatus::Broken => (
            Label::Broken,
            Some(format!(
                "{} {words}",
                first_message
                    .clone()
                    .unwrap_or_else(|| "anchor not found".into())
            )),
        ),
        NodeStatus::Ambiguous => (Label::Claimed, first_message),
        NodeStatus::Resolved => (Label::Verified, None),
    };
    if inv.kind == Kind::Callstack && i > 0 && label != Label::Broken {
        let prev = &inv.steps[i - 1];
        let edge = report.edges.iter().find(|e| &e.from == prev && &e.to == id);
        let edge_label = edge.map_or(Label::Claimed, |e| Label::of(e.status));
        if edge_label != Label::Verified {
            label = Label::Claimed;
            reason.get_or_insert_with(|| match edge {
                None => format!("no edge from `{prev}` in the file"),
                Some(_) => "the index does not confirm this call".into(),
            });
        }
    }
    Some(Step {
        id: id.clone(),
        n: i + 1,
        title: cho_investigation::session::node_title(node),
        symbol: node.anchor.symbol.clone(),
        path: node.anchor.path.clone(),
        label,
        reason,
        range: checked.range.clone(),
        region: checked.region.clone(),
        name_start: checked.name_range.as_ref().map(|r| r.start),
        line: checked.line,
        note: node.note.clone(),
        number: (i + 1).to_string(),
        extras: Default::default(),
    })
}

/// Step `i` of `inv`, showing node `id`.
fn step_of(
    inv: &Investigation,
    report: &CheckReport,
    words: &str,
    i: usize,
    id: &String,
) -> Option<Step> {
    if let Some(a) = inv.action(id) {
        return Some(action_step(a, id, i));
    }
    match inv.lookup(id)? {
        Ok(node) => code_step(inv, report, words, i, id, node),
        Err(ext) => Some(external_step(ext, id, i)),
    }
}

/// The steps as the panes show them under the reader's choices: a comparison's rows with
/// each side's chosen variant (`resolve_rows`, as `InvestigationSession::comparison_rows`
/// does), else the tour's or call stack's steps on the chosen branch, with what version 2
/// adds. The summary bullets and "could not be traced" list come with them.
pub fn derive_steps(
    inv: &Investigation,
    report: &CheckReport,
    at: &CodeAt,
    choice: &crate::comparison::Choice,
) -> (Vec<Step>, crate::tour_pages::Overview) {
    let (extras, overview) = v2_of(inv, report);
    if inv.kind == Kind::Comparison {
        let rows = cho_investigation::check_v2::resolve_rows(inv, report, &choice.variants);
        return (crate::comparison::row_steps(inv, &rows), overview);
    }
    let branch = choice
        .branch
        .as_ref()
        .and_then(|b| inv.variants.iter().find(|v| &v.id == b));
    let steps = with_extras(steps_on(inv, report, at, branch), &extras);
    (apply_branch(steps, inv, branch), overview)
}

/// Marks what a tour branch changes, after [`with_extras`]: a step it does not reach keeps its
/// place, muted, with the reason; a step it replaces drops the original's view and name and
/// takes the branch's values and note.
pub fn apply_branch(
    mut steps: Vec<Step>,
    inv: &Investigation,
    branch: Option<&cho_investigation::compare::Variant>,
) -> Vec<Step> {
    let Some(branch) = branch else { return steps };
    for s in &mut steps {
        let Some(o) = branch.rows.get(&s.id) else {
            continue;
        };
        match &o.step {
            None => {
                s.extras.not_reached = Some(
                    o.missing
                        .clone()
                        .unwrap_or_else(|| "not reached on this path".into()),
                );
                s.extras.view = None;
            }
            Some(node) => {
                s.extras.view = None;
                s.extras.name = match inv.lookup(node) {
                    Some(Ok(n)) => n.name.clone(),
                    Some(Err(e)) => e.name.clone(),
                    None => None,
                };
            }
        }
        if !o.values.is_empty() {
            s.extras.runtime = o.values.clone();
        }
        if let Some(n) = &o.note {
            s.note = Some(n.clone());
        }
    }
    steps
}

/// Whether step `ix` can be stepped to: it is reached on the chosen path.
pub fn reached(steps: &[Step], ix: usize) -> bool {
    steps
        .get(ix)
        .is_some_and(|s| s.extras.not_reached.is_none())
}

/// Like [`step_to`], over reached steps only: `delta` reached steps from `current`, stopping
/// at the last reached step at either end. `current` itself need not be reached.
pub fn step_to_reached(steps: &[Step], current: usize, delta: isize) -> usize {
    let len = steps.len();
    if len == 0 {
        return 0;
    }
    let mut at = current.min(len - 1);
    let dir: isize = if delta < 0 { -1 } else { 1 };
    for _ in 0..delta.unsigned_abs() {
        let mut next = at;
        loop {
            let Some(n) = next.checked_add_signed(dir).filter(|n| *n < len) else {
                // No reached step further on: stay.
                return if reached(steps, at) {
                    at
                } else {
                    first_reached(steps, current, -dir).unwrap_or(at)
                };
            };
            next = n;
            if reached(steps, next) {
                break;
            }
        }
        at = next;
    }
    at
}

/// The first reached step from `from` going `dir` (inclusive).
fn first_reached(steps: &[Step], from: usize, dir: isize) -> Option<usize> {
    let mut i = from;
    loop {
        if reached(steps, i) {
            return Some(i);
        }
        i = i.checked_add_signed(dir).filter(|n| *n < steps.len())?;
    }
}

/// The first and last reached steps.
pub fn reached_ends(steps: &[Step]) -> Option<(usize, usize)> {
    let first = steps.iter().position(|s| s.extras.not_reached.is_none())?;
    let last = steps.iter().rposition(|s| s.extras.not_reached.is_none())?;
    Some((first, last))
}

/// Adds what format version 2 says about each step (by id) and numbers the steps by phase:
/// setup steps S1, S2…, the rest 1, 2….
pub fn with_extras(
    mut steps: Vec<Step>,
    extras: &std::collections::HashMap<String, crate::tour_pages::StepExtras>,
) -> Vec<Step> {
    for s in &mut steps {
        if let Some(x) = extras.get(&s.id) {
            s.extras = x.clone();
        }
    }
    let phases: Vec<_> = steps.iter().map(|s| s.extras.phase).collect();
    for (s, n) in steps
        .iter_mut()
        .zip(cho_investigation::check_v2::numbers(phases.iter().copied()))
    {
        s.number = n;
    }
    steps
}

/// An action step as its card shows it.
pub fn action_card(a: &cho_investigation::Action) -> std::sync::Arc<crate::tour_pages::ActionCard> {
    std::sync::Arc::new(crate::tour_pages::ActionCard {
        name: a.name.clone(),
        note: a.note.clone(),
        commands: a
            .commands
            .iter()
            .map(|c| crate::tour_pages::CommandLine {
                label: c.label.clone(),
                text: c.text.clone(),
            })
            .collect(),
    })
}

/// What an external node is, in words.
pub fn external_word(what: &cho_investigation::ExternalKind) -> &'static str {
    use cho_investigation::ExternalKind as K;
    match what {
        K::Model => "a model",
        K::Database => "a database",
        K::Service => "a service",
        K::Vendor => "a vendor",
        K::Platform => "the platform",
        K::Limit => "a limit",
        K::Other => "not code",
    }
}

/// What format version 2 adds, per step id and for the whole file, from the check report
/// (`report.v2`): Chorong's numbers, the author's names, evidence and values, external nodes,
/// and each step's view resolved into panes. Version 1 files add nothing.
pub fn v2_of(
    inv: &Investigation,
    report: &CheckReport,
) -> (
    std::collections::HashMap<String, crate::tour_pages::StepExtras>,
    crate::tour_pages::Overview,
) {
    use crate::tour_pages::{Overview, StepExtras};
    let Some(v2) = &report.v2 else {
        return Default::default();
    };
    let mut out = std::collections::HashMap::new();
    for s in &v2.steps {
        let external = s.external.then(|| {
            inv.lookup(&s.id)
                .and_then(|r| r.err())
                .map_or("not code", |e| external_word(&e.what))
                .to_string()
        });
        let view = v2
            .views
            .iter()
            .find(|v| v.step == s.id && !v.default)
            .map(view_of);
        out.insert(
            s.id.clone(),
            StepExtras {
                name: s.name.clone(),
                phase: s.phase,
                evidence: s.author_evidence,
                runtime: s.values.clone(),
                external,
                action: inv.action(&s.id).map(action_card),
                view,
                ..Default::default()
            },
        );
    }
    let untraced = v2
        .untraced
        .iter()
        .map(|u| {
            let mut t = u.what.clone();
            if let Some(why) = &u.why {
                t.push_str(": ");
                t.push_str(why);
            }
            if let Some(at) = &u.at {
                t.push_str(&format!(" ({})", at.label));
            }
            t
        })
        .collect();
    (
        out,
        Overview {
            bullets: inv.points.clone(),
            untraced,
        },
    )
}

/// A written view, resolved: broken anchors become cards that say why.
fn view_of(v: &cho_investigation::check_v2::ViewReport) -> crate::tour_pages::View {
    use crate::tour_pages::{Arrangement, Highlight, PaneSpec, View};
    use cho_investigation::check::NodeStatus;
    use cho_investigation::check_v2::{AnchorReport, PaneReport};
    let broken = |a: &AnchorReport| PaneSpec::Broken {
        asked: format!("{} in {}", a.label, a.path),
        reason: a
            .messages
            .first()
            .cloned()
            .unwrap_or_else(|| "anchor not found".into()),
    };
    let panes = v
        .panes
        .iter()
        .map(|p| match p {
            PaneReport::Editor {
                path, highlights, ..
            } => {
                let good: Vec<_> = highlights
                    .iter()
                    .filter(|h| h.anchor.status != NodeStatus::Broken)
                    .collect();
                let Some(first) = good.first() else {
                    return match highlights.first() {
                        Some(h) => broken(&h.anchor),
                        None => PaneSpec::Broken {
                            asked: path.clone().unwrap_or_default(),
                            reason: "no anchor".into(),
                        },
                    };
                };
                let range = |a: &AnchorReport| a.range.clone().or_else(|| a.region.clone());
                // One chunk without a note is the plain anchor tint, with no strip.
                let plain = highlights.len() == 1 && first.name.is_none() && first.note.is_none();
                PaneSpec::Editor {
                    path: first.anchor.path.clone(),
                    // An excerpt's or range's start, else the declared name (as `Step::caret`).
                    caret: first.anchor.caret(),
                    region: first.anchor.region.clone(),
                    highlights: if plain {
                        Vec::new()
                    } else {
                        good.iter()
                            .filter_map(|h| {
                                Some(Highlight {
                                    range: range(&h.anchor)?,
                                    region: h.anchor.region.clone(),
                                    label: h.name.clone(),
                                    note: h.note.clone(),
                                })
                            })
                            .collect()
                    },
                    at: first.anchor.label.clone(),
                }
            }
            PaneReport::Context { anchor } if anchor.status == NodeStatus::Broken => broken(anchor),
            PaneReport::Context { anchor } => PaneSpec::Context {
                symbol: Some(anchor.label.clone()),
                path: Some(anchor.path.clone()),
                at: anchor.name_or_range_start(),
            },
            PaneReport::Outline { anchor } => PaneSpec::Outline {
                path: anchor.path.clone(),
            },
            PaneReport::Graph {
                node,
                anchor,
                direction,
                depth,
                ..
            } => PaneSpec::Graph {
                of: node
                    .clone()
                    .or_else(|| anchor.as_ref().map(|a| a.label.clone()))
                    .unwrap_or_default(),
                direction: format!("{direction:?}").to_lowercase(),
                depth: *depth,
                path: anchor.as_ref().map(|a| a.path.clone()),
                at: anchor.as_ref().and_then(|a| a.name_or_range_start()),
            },
            PaneReport::Callstack => PaneSpec::CallStack,
            PaneReport::Diff {
                path,
                base,
                head,
                problem,
            } => {
                let diff = crate::tour_pages::DiffAt {
                    path: path.clone(),
                    base: base.rev.clone().unwrap_or_default(),
                    head: head.rev.clone(),
                };
                match problem {
                    // Never guessed: the card says what could not be resolved.
                    Some(reason) => PaneSpec::DiffUnresolved {
                        diff,
                        reason: reason.clone(),
                    },
                    None => PaneSpec::Diff(diff),
                }
            }
            PaneReport::FileTree { .. } => PaneSpec::FileTree,
        })
        .collect();
    View {
        panes,
        arrangement: Arrangement::parse(&v.arrangement).unwrap_or(Arrangement::MainSide),
    }
}

/// How many steps carry each label: (verified, claimed, broken). External nodes carry none.
pub fn counts(steps: &[Step]) -> (usize, usize, usize) {
    let n = |l| {
        steps
            .iter()
            .filter(|s| s.label == l && !s.no_code())
            .count()
    };
    (n(Label::Verified), n(Label::Claimed), n(Label::Broken))
}

/// A row of a step list: a phase's heading, or a step (zero-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListItem {
    Phase(crate::tour_pages::Phase),
    Step(usize),
}

/// The rows of a step list. When any step has a phase, each run of steps in one phase starts
/// with its heading (Setup, Call); otherwise the rows are the steps.
pub fn list_items(steps: &[Step]) -> Vec<ListItem> {
    let phased = steps.iter().any(|s| s.extras.phase.is_some());
    let mut out = Vec::with_capacity(steps.len() + 2);
    let mut last = None;
    for (i, s) in steps.iter().enumerate() {
        let p = s.extras.phase.unwrap_or(crate::tour_pages::Phase::Call);
        if phased && last != Some(p) {
            out.push(ListItem::Phase(p));
            last = Some(p);
        }
        out.push(ListItem::Step(i));
    }
    out
}

/// The list row of step `ix`.
pub fn item_of(steps: &[Step], ix: usize) -> usize {
    list_items(steps)
        .iter()
        .position(|i| *i == ListItem::Step(ix))
        .unwrap_or(ix)
}

/// The tour's files in first-visit order, each with its stop numbers (stops the chosen branch
/// does not reach are left out).
pub fn tour_files(steps: &[Step]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for s in steps.iter().filter(|s| s.extras.not_reached.is_none()) {
        match out.iter_mut().find(|(p, _)| *p == s.path) {
            Some((_, ns)) => ns.push(s.n),
            None => out.push((s.path.clone(), vec![s.n])),
        }
    }
    out
}

/// Where a tour stop stands, for its badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopState {
    Done,
    Current,
    ToDo,
    Broken,
}

/// The badge of stop `ix` (zero-based) when `current` is the current stop. A broken stop shows
/// as broken whatever its place.
pub fn stop_state(step: &Step, ix: usize, current: usize) -> StopState {
    if step.label == Label::Broken {
        StopState::Broken
    } else if ix == current {
        StopState::Current
    } else if ix < current {
        StopState::Done
    } else {
        StopState::ToDo
    }
}

// ---- Notes: inert markdown ----

/// A piece of a note's line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    /// Text under `**strong**` and/or `*emphasis*` ([`crate::note_md`]); at least one is set.
    Styled {
        text: String,
        bold: bool,
        italic: bool,
    },
    Code(String),
    /// A link: shown with its real target, opened only on click and only when `kind` allows.
    Link {
        text: String,
        target: String,
        kind: LinkKind,
    },
}

/// What a click on a link may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `chorong://`: followed through the viewer's own link handling (its scheme and folder checks).
    Chorong,
    /// `https://`: opened in the browser.
    Web,
    /// Anything else (relative, `http:`, `file:`, `javascript:`…): shown, never opened.
    Inert,
}

impl LinkKind {
    pub fn of(target: &str) -> LinkKind {
        if target.starts_with("chorong://") {
            LinkKind::Chorong
        } else if target.starts_with("https://") && !target.contains(char::is_whitespace) {
            LinkKind::Web
        } else {
            LinkKind::Inert
        }
    }
}

/// A note as blocks of inline pieces. Only `` `code` ``, `[text](target)` and emphasis are
/// read; every other character, raw HTML and image syntax included, stays as text. Paragraphs
/// split at blank lines; list items and headings start blocks of their own.
pub fn note_blocks(md: &str) -> Vec<Vec<Inline>> {
    let mut blocks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in md.lines() {
        let t = line.trim();
        let starts_block = t.starts_with("- ")
            || t.starts_with("* ")
            || t.starts_with('#')
            || t.split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if t.is_empty() || starts_block {
            if !cur.is_empty() {
                blocks.push(std::mem::take(&mut cur));
            }
            if t.is_empty() {
                continue;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(t);
    }
    if !cur.is_empty() {
        blocks.push(cur);
    }
    blocks.iter().map(|b| crate::note_md::inlines(b)).collect()
}

/// `[label](target)` at the start of `s`: the label, the target and the bytes used.
pub(crate) fn link_at(s: &str) -> Option<(&str, &str, usize)> {
    let close = s.find("](")?;
    let label = &s[1..close];
    if label.contains(['[', ']']) {
        return None;
    }
    let after = &s[close + 2..];
    let end = after.find(')')?;
    let target = after[..end].trim();
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    Some((label, target, close + 2 + end + 1))
}

// ---- Listing, recents and place ----

/// Chorong's store of recents and places, in its config directory (never in a worktree).
pub fn store() -> Option<RecentsStore> {
    cho_settings::config_dir().map(RecentsStore::new)
}

pub fn now_secs() -> u64 {
    cho_investigation::recents::now_secs()
}

/// A row of Open Investigation…
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub source: Source,
    pub title: String,
    pub kind: String,
    /// Short commit.
    pub rev: String,
    /// Host or path.
    pub origin: String,
    /// Broken anchors in the working tree, once checked.
    pub broken: Option<usize>,
    /// A recent whose file is gone.
    pub missing: bool,
    /// Why it cannot be opened (it does not parse).
    pub error: Option<String>,
    /// From the repository's `.chorong/investigations/`, not from recents.
    pub in_repo: bool,
}

impl Listed {
    fn new(source: Source, root: &Path, in_repo: bool) -> Listed {
        Listed {
            origin: origin(&source, Some(root)),
            title: String::new(),
            source,
            kind: String::new(),
            rev: String::new(),
            broken: None,
            missing: false,
            error: None,
            in_repo,
        }
    }

    /// The right-hand text: `tour · 3f9c1e2 · .chorong/investigations/a.json · 1 broken`.
    pub fn hint(&self) -> String {
        if self.missing {
            return format!("missing \u{b7} {}", self.origin);
        }
        if let Some(e) = &self.error {
            return format!("cannot open: {e} \u{b7} {}", self.origin);
        }
        let mut parts = vec![self.kind.clone(), self.rev.clone(), self.origin.clone()];
        match self.broken {
            Some(0) => parts.push("all anchors found".into()),
            Some(n) => parts.push(format!("{n} broken")),
            None => {}
        }
        parts.retain(|p| !p.is_empty());
        parts.join(" \u{b7} ")
    }

    /// Fills the row from the parsed file, checked against `root` for the broken count.
    fn fill(mut self, root: &Path) -> Listed {
        match read(&self.source, Some(root)) {
            Ok(inv) => {
                let report = check_with_reader(&inv, &WorktreeReader { root }, None);
                self.broken = Some(report.broken_count());
                self.title = inv.title.clone();
                self.kind = kind_word(&inv.kind).into();
                self.rev = cho_git::short(&inv.rev).to_string();
            }
            Err(e) => {
                self.title = self.origin.rsplit('/').next().unwrap_or("").to_string();
                self.error = Some(e);
            }
        }
        self
    }
}

pub fn kind_word(kind: &Kind) -> &'static str {
    match kind {
        Kind::Callstack => "call stack",
        Kind::Tour => "tour",
        Kind::Diagram => "diagram",
        Kind::PrOverlay => "pr overlay",
        Kind::Comparison => "comparison",
    }
}

/// The repository's `.chorong/investigations/*.json` (the backend's listing: regular files,
/// sorted by name), then the recents not already listed. Local files are parsed and checked
/// against `root`; a URL is not fetched. Runs off the UI thread.
pub fn list(root: &Path, recents: &[Recent]) -> Vec<Listed> {
    let mut rows: Vec<Listed> = cho_investigation::listing::list(root)
        .map(|l| l.entries)
        .unwrap_or_default()
        .into_iter()
        .map(|e| {
            let source = match e.source {
                Source::Path(p) if p.is_relative() => Source::Path(root.join(p)),
                other => other,
            };
            Listed::new(source, root, true).fill(root)
        })
        .collect();
    let same = |a: &Source, b: &Source| match (a, b) {
        (Source::Path(a), Source::Path(b)) => {
            a == b
                || a.canonicalize()
                    .ok()
                    .is_some_and(|a| Some(a) == b.canonicalize().ok())
        }
        (a, b) => a == b,
    };
    for r in recents {
        if rows.iter().any(|row| same(&row.source, &r.source)) {
            continue;
        }
        let row = Listed::new(r.source.clone(), root, false);
        rows.push(match &r.source {
            Source::Path(p) if !p.is_file() => Listed {
                title: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                missing: true,
                ..row
            },
            Source::Path(_) => row.fill(root),
            Source::Url(u) => Listed {
                title: u.rsplit('/').next().unwrap_or(u).to_string(),
                ..row
            },
        });
    }
    rows
}

/// The worktree an investigation's anchors are relative to: the window's own root when the
/// investigation's repository is this one (or a local file inside it), else the worktree Chorong
/// knows for its `repo` at its `rev`.
pub fn worktree_for(
    inv: &Investigation,
    file: Option<&Path>,
    known: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(dir) = file.and_then(Path::parent) {
        for d in dir.ancestors() {
            if d.join(".git").exists() {
                return Some(d.to_path_buf());
            }
        }
    }
    let repo = cho_core::RepoRef::parse(&inv.repo).ok()?;
    let candidates = cho_git::candidates_for(&repo, known);
    cho_git::choose_worktree(&candidates, Some(&inv.rev), None).map(|c| c.path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, Investigation) {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/q.rs",
            "struct Q;\nimpl Q {\n    fn push(&self) {\n        helper();\n    }\n}\nfn helper() {}\n",
        );
        let inv = Investigation::parse(
            &serde_json::json!({
                "chorong": 1, "title": "Push", "kind": "callstack", "repo": "local:/x", "rev": "abcdef1",
                "nodes": [
                    { "id": "push", "anchor": { "path": "src/q.rs", "symbol": "Q::push" }, "note": "Starts `here`." },
                    { "id": "helper", "anchor": { "path": "src/q.rs", "symbol": "helper" } },
                    { "id": "gone", "anchor": { "path": "src/q.rs", "symbol": "nope" }, "label": "Old entry" },
                    { "id": "loose", "anchor": { "path": "src/q.rs", "line": 7 } }
                ],
                "edges": [ { "from": "push", "to": "helper", "rel": "calls" } ],
                "steps": ["push", "helper", "gone", "loose"]
            })
            .to_string(),
        )
        .unwrap();
        (dir, inv)
    }

    /// Format version 2 action steps and diff panes, as the panes get them: the action is a
    /// stop with its card and no label; a diff pane whose base resolves is a diff, one whose
    /// base doesn't is a card with the reason (never another revision).
    #[test]
    fn action_steps_carry_their_card_and_diff_panes_resolve_or_say_why() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/q.rs", "fn push() {}\n");
        cho_core::test_repo::init(dir.path());
        cho_core::test_repo::commit_all(dir.path(), "one");
        let inv = Investigation::parse(
            &serde_json::json!({
                "chorong": 2, "title": "Mixed", "kind": "tour", "repo": "local:/x", "rev": "abcdef1",
                "nodes": [ { "id": "push", "anchor": { "path": "src/q.rs", "symbol": "push" } } ],
                "actions": [ { "id": "run", "name": "Run it", "note": "Then **read**.",
                               "commands": [ { "label": "Go", "text": "make run \\\n  --fast 'a b'\n" } ] } ],
                "steps": ["push", "run"],
                "views": {
                    "push": { "panes": [
                        { "kind": "diff", "path": "src/q.rs", "base": "main" },
                        { "kind": "diff", "path": "src/q.rs", "base": "nope" } ] }
                }
            })
            .to_string(),
        )
        .unwrap();
        let at = CodeAt::WorkingTree;
        let report = check_at(&inv, dir.path(), &at, None);
        let (extras, _) = v2_of(&inv, &report);
        let s = with_extras(steps(&inv, &report, &at), &extras);
        assert!(s[1].is_action() && s[1].no_code() && !s[1].is_external());
        let card = s[1].extras.action.as_ref().unwrap();
        // Exactly as written in the file: Copy gives these bytes.
        assert_eq!(
            card.commands[0].clipboard(),
            "make run \\\n  --fast 'a b'\n"
        );
        assert_eq!(
            card.commands[0].clipboard(),
            inv.actions[0].commands[0].text
        );
        assert_eq!(s[1].list_name(), "Run it");
        assert_eq!(counts(&s), (1, 0, 0), "an action carries no label");
        let view = s[0].extras.view.as_ref().unwrap();
        use crate::tour_pages::PaneSpec;
        assert!(
            matches!(&view.panes[0], PaneSpec::Diff(d) if d.base == "main" && d.head.is_none())
        );
        assert!(
            matches!(&view.panes[1], PaneSpec::DiffUnresolved { reason, .. } if reason.contains("nope"))
        );
    }

    struct Yes;
    impl EdgeOracle for Yes {
        fn confirms(
            &self,
            _: cho_investigation::Relation,
            _: &cho_investigation::check::Endpoint<'_>,
            _: &cho_investigation::check::Endpoint<'_>,
        ) -> bool {
            true
        }
    }

    #[test]
    fn frames_are_labelled_by_anchor_and_incoming_edge() {
        let (dir, inv) = fixture();
        let at = CodeAt::WorkingTree;
        let report = check_at(&inv, dir.path(), &at, None);
        let s = steps(&inv, &report, &at);
        let labels: Vec<_> = s.iter().map(|s| s.label).collect();
        // No index: the call into `helper` is claimed; `gone` is broken; `loose` has no edge.
        assert_eq!(
            labels,
            [
                Label::Verified,
                Label::Claimed,
                Label::Broken,
                Label::Claimed
            ]
        );
        assert_eq!(s[2].title, "Old entry");
        assert!(
            s[2].reason
                .as_deref()
                .unwrap()
                .ends_with("in the working tree")
        );
        assert_eq!(s[3].location(), "q.rs:7");
        assert_eq!(counts(&s), (1, 2, 1));
        let report = check_at(&inv, dir.path(), &at, Some(&Yes));
        let s = steps(&inv, &report, &at);
        assert_eq!(s[1].label, Label::Verified);
        assert_eq!(s[0].location(), "q.rs \u{b7} Q::push");
    }

    /// Format version 2 on top of a version 1 file's steps, as the viewer gets it from the
    /// format's types: phases, short names, evidence, values and an external node.
    #[test]
    fn v2_extras_number_steps_by_phase_and_externals_carry_no_label() {
        use crate::tour_pages::{Evidence, Phase, StepExtras};
        let (dir, inv) = fixture();
        let at = CodeAt::WorkingTree;
        let report = check_at(&inv, dir.path(), &at, None);
        let mut extras = std::collections::HashMap::new();
        let ids: Vec<String> = inv.steps.clone();
        extras.insert(
            ids[0].clone(),
            StepExtras {
                phase: Some(Phase::Setup),
                name: Some("Queue made".into()),
                evidence: Some(Evidence::Read),
                ..Default::default()
            },
        );
        for id in &ids[1..] {
            extras.insert(
                id.clone(),
                StepExtras {
                    phase: Some(Phase::Call),
                    ..Default::default()
                },
            );
        }
        extras.get_mut(&ids[2]).unwrap().external = Some("a vendor queue".into());
        extras.get_mut(&ids[3]).unwrap().runtime = vec!["full = false".into()];
        let s = with_extras(steps(&inv, &report, &at), &extras);
        let numbers: Vec<_> = s.iter().map(|s| s.number.as_str()).collect();
        assert_eq!(numbers, ["S1", "1", "2", "3"]);
        assert_eq!(s[0].list_name(), "Queue made");
        assert_eq!(s[1].list_name(), s[1].title, "no short name: the title");
        assert!(s[2].is_external());
        // The external step counts under no label; Chorong's labels are unchanged by evidence.
        assert_eq!(counts(&s), (1, 2, 0));
        assert_eq!(s[0].label, Label::Verified);
        assert_eq!(
            list_items(&s),
            [
                ListItem::Phase(Phase::Setup),
                ListItem::Step(0),
                ListItem::Phase(Phase::Call),
                ListItem::Step(1),
                ListItem::Step(2),
                ListItem::Step(3),
            ]
        );
        assert_eq!(item_of(&s, 1), 3);
        // Without phases the list is the steps.
        let plain = steps(&inv, &report, &at);
        assert_eq!(list_items(&plain).len(), plain.len());
        assert_eq!(item_of(&plain, 2), 2);
    }

    #[test]
    fn a_tour_stop_is_its_anchor_and_broken_stops_are_kept() {
        let (dir, mut inv) = fixture();
        inv.kind = Kind::Tour;
        let at = CodeAt::WorkingTree;
        let s = steps(&inv, &check_at(&inv, dir.path(), &at, None), &at);
        assert_eq!(s.len(), 4);
        assert_eq!(s[1].label, Label::Verified);
        assert_eq!(stop_state(&s[2], 2, 0), StopState::Broken);
        assert_eq!(stop_state(&s[0], 0, 1), StopState::Done);
        assert_eq!(stop_state(&s[1], 1, 1), StopState::Current);
        assert_eq!(stop_state(&s[3], 3, 1), StopState::ToDo);
        assert_eq!(tour_files(&s), [("src/q.rs".to_string(), vec![1, 2, 3, 4])]);
    }

    #[test]
    fn rows_show_a_urls_host_and_a_files_path_in_the_repository() {
        let u = Source::from_link_src("https://example.com/x/inv.json?a=1").unwrap();
        assert_eq!(origin(&u, None), "example.com");
        let f = Source::from_link_src("/r/.chorong/investigations/a.json").unwrap();
        assert_eq!(
            origin(&f, Some(Path::new("/r"))),
            ".chorong/investigations/a.json"
        );
        assert_eq!(source_key(&f), "/r/.chorong/investigations/a.json");
        for bad in [
            "relative.json",
            "http://example.com/a.json",
            "file:///etc/passwd",
        ] {
            assert!(Source::from_link_src(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn notes_are_inert() {
        let b = note_blocks(
            "Calls `push`, see [docs](https://example.com/d) and [here](chorong://open?repo=x).\n\n<img src=\"https://evil/x.png\"> ![alt](https://evil/y.png) [run](javascript:alert(1)) [rel](../a)\n- item",
        );
        assert_eq!(b.len(), 3);
        assert_eq!(b[0][1], Inline::Code("push".into()));
        assert!(
            matches!(&b[0][3], Inline::Link { kind: LinkKind::Web, target, .. } if target == "https://example.com/d")
        );
        assert!(matches!(
            &b[0][5],
            Inline::Link {
                kind: LinkKind::Chorong,
                ..
            }
        ));
        // Raw HTML and the image stay text; other schemes are shown but inert.
        let Inline::Text(t) = &b[1][0] else { panic!() };
        assert!(t.starts_with("<img src=") && t.contains("![alt](https://evil/y.png)"));
        assert!(b[1].iter().all(|i| !matches!(
            i,
            Inline::Link {
                kind: LinkKind::Web | LinkKind::Chorong,
                ..
            }
        )));
        assert!(b[1].iter().any(
            |i| matches!(i, Inline::Link { kind: LinkKind::Inert, target, .. } if target == "../a")
        ));
        assert_eq!(b[2], [Inline::Text("- item".into())]);
    }

    #[test]
    fn the_listing_puts_the_repository_first_then_recents_and_marks_missing() {
        let (dir, inv) = fixture();
        let root = dir.path();
        write(root, ".chorong/investigations/b.json", &inv.to_json());
        write(root, ".chorong/investigations/a.json", "{ not json");
        write(root, ".chorong/investigations/notes.txt", "x");
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "c.json", &inv.to_json());
        let recent = |src: String| Recent {
            source: Source::from_link_src(&src).unwrap(),
            opened_at: 1,
        };
        let recents = vec![
            recent(
                root.join(".chorong/investigations/b.json")
                    .display()
                    .to_string(),
            ),
            recent(outside.path().join("c.json").display().to_string()),
            recent("/nowhere/gone.json".into()),
            recent("https://example.com/i.json".into()),
        ];
        let rows = list(root, &recents);
        let titles: Vec<_> = rows.iter().map(|r| (r.in_repo, r.title.as_str())).collect();
        assert_eq!(
            titles,
            [
                (true, "a.json"),
                (true, "Push"),
                (false, "Push"),
                (false, "gone.json"),
                (false, "i.json")
            ]
        );
        assert!(rows[0].error.is_some());
        assert_eq!(rows[1].broken, Some(1));
        assert_eq!(
            rows[1].hint(),
            "call stack \u{b7} abcdef1 \u{b7} .chorong/investigations/b.json \u{b7} 1 broken"
        );
        assert!(rows[3].missing && rows[3].hint().starts_with("missing"));
        assert_eq!(rows[4].origin, "example.com");
        assert_eq!(rows[4].broken, None);
    }
}

#[cfg(test)]
mod cost {
    /// The cost of one live-reload look (`stamp`: one `stat`), as docs/public/settings.md
    /// quotes it. Run with `cargo test -p chorong-app stamp_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn stamp_cost() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.json");
        std::fs::write(&file, "{}").unwrap();
        let source = super::Source::Path(file);
        let n = 20_000;
        let started = std::time::Instant::now();
        for _ in 0..n {
            assert!(super::stamp(&source).is_some());
        }
        let each = started.elapsed() / n;
        println!("STAMP_COST each={each:?}");
    }
}

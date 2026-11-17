//! An open investigation, with no UI: the file, its check report and the current step.
//!
//! Loading is read, then parse and validate, then check. Nothing here runs in the
//! background: the app decides when to call [`InvestigationSession::check_for_update`],
//! which only reports; [`InvestigationSession::reload`] swaps in a new version only when it
//! parses.

use crate::check::{
    CheckReport, EdgeOracle, NodeReport, NodeStatus, SourceReader, check_with_reader,
};
use crate::check_v2::{
    BranchStep, Caller, ComparisonReport, Depth, RowReport, V2Report, ViewReport, branch_steps,
    default_page, resolve_rows,
};
use crate::source::{self, ContentState, Fetched, Source, SourceError};
use crate::{Anchor, Evidence, External, Investigation, Node, Relation, Variant, Verification};
use serde::Serialize;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// What the anchored files are read from, and what confirms edges. Passed to
/// [`InvestigationSession::open`] and [`InvestigationSession::reload`].
pub struct Checker<'a> {
    pub reader: &'a dyn SourceReader,
    /// Must describe the same code as `reader` (see [`check_with_reader`]).
    pub oracle: Option<&'a dyn EdgeOracle>,
}

/// One step of a `tour` or `callstack`, resolved.
#[derive(Debug, Clone, Serialize)]
pub struct StepView<'a> {
    /// Position in `steps`, zero-based.
    pub index: usize,
    /// The node shown: the step's own id, or the one a tour branch puts in its place.
    pub id: &'a str,
    /// The id written in `steps`.
    pub step_id: &'a str,
    /// The code node; `None` for an external node or a step the branch doesn't reach.
    #[serde(skip)]
    pub node: Option<&'a Node>,
    /// Version 2: the external node, when the step isn't code.
    #[serde(skip)]
    pub external: Option<&'a External>,
    pub anchor: Option<&'a Anchor>,
    /// Display label: the node's `label`, else its symbol, else its file name.
    pub title: String,
    /// Resolved, ambiguous or broken; `None` for an external node or a step not reached.
    pub status: Option<NodeStatus>,
    /// UTF-8 byte range the step points at (the excerpt, else the region); `None` when broken.
    pub range: Option<Range<usize>>,
    /// The whole declaration, line or file the anchor covers.
    pub region: Option<Range<usize>>,
    /// One-based line of `range.start`.
    pub line: Option<usize>,
    pub note: Option<&'a str>,
    /// Why the anchor is ambiguous or broken.
    pub messages: &'a [String],
    /// "S1", "S2"… for setup steps, "1", "2"… for the rest (version 1: "1", "2"…).
    pub number: String,
    /// Version 2: the short step name.
    pub name: Option<&'a str>,
    /// Version 2: runtime values (the branch's, when it overrides the step), as written.
    pub values: &'a [String],
    /// Version 2: the author's evidence, as written.
    pub author_evidence: Option<Evidence>,
    /// False when the chosen tour branch doesn't reach this step.
    pub reached: bool,
    /// Why the step is not reached under the chosen branch.
    pub not_reached: Option<&'a str>,
    /// Version 2: derived from call edges from the first step.
    pub depth: Option<Depth>,
    /// Version 2: `calls` and `dispatches` edges into this step, with Chorong's labels.
    pub called_by: &'a [Caller],
}

/// One edge, with Chorong's label.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeView<'a> {
    /// Index in the investigation's `edges`.
    pub index: usize,
    pub from: &'a str,
    pub to: &'a str,
    pub rel: Relation,
    pub verification: Verification,
    pub note: Option<&'a str>,
}

/// What [`InvestigationSession::check_for_update`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// The file has the same length and content hash.
    Unchanged,
    /// The content changed. Offer **Reload**; nothing has been swapped in.
    UpdateAvailable,
    /// The file can no longer be read (deleted, too large, replaced by a directory…).
    Unreadable(String),
    /// URL sources are not watched; they are fetched again only when reopened.
    NotWatched,
}

/// What [`InvestigationSession::reload`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// Nothing changed on disk.
    Unchanged,
    /// The new version is in. `kept_step` is false when the current step's id is gone and
    /// the first step is now current.
    Reloaded { kept_step: bool },
}

/// An open investigation.
#[derive(Debug, Clone)]
pub struct InvestigationSession {
    source: Source,
    /// Repository root, for safe reads of files inside it.
    root: Option<PathBuf>,
    investigation: Investigation,
    report: CheckReport,
    state: ContentState,
    /// For a URL source, the URL after redirects.
    final_url: Option<String>,
    current: usize,
    /// Tour branch chosen, by id.
    branch: Option<String>,
    /// Comparison variants chosen: side id to variant id.
    variants: BTreeMap<String, String>,
}

impl InvestigationSession {
    /// Reads `source` (bounded; see [`source::read`]), parses, validates and checks it.
    /// `root` is the open repository: files inside it are read with the safe in-repo read.
    pub fn open(
        source: Source,
        root: Option<&Path>,
        checker: &Checker<'_>,
    ) -> Result<Self, SourceError> {
        let fetched = source::read(&source, root)?;
        Self::from_fetched(source, root, fetched, checker)
    }

    /// Like [`Self::open`] for bytes already read with [`source::read`], e.g. fetched on a
    /// worker thread so the UI thread never waits on the network.
    pub fn from_fetched(
        source: Source,
        root: Option<&Path>,
        fetched: Fetched,
        checker: &Checker<'_>,
    ) -> Result<Self, SourceError> {
        let investigation = source::parse_bytes(&fetched.bytes)?;
        let report = check_with_reader(&investigation, checker.reader, checker.oracle);
        Ok(Self {
            source,
            root: root.map(Path::to_path_buf),
            investigation,
            report,
            state: fetched.state,
            final_url: fetched.final_url,
            current: 0,
            branch: None,
            variants: BTreeMap::new(),
        })
    }

    pub fn source(&self) -> &Source {
        &self.source
    }
    pub fn investigation(&self) -> &Investigation {
        &self.investigation
    }
    pub fn report(&self) -> &CheckReport {
        &self.report
    }
    /// The host to show for a URL source: where the content came from after redirects.
    /// `None` for a file. [`Source::host`] is the host the link named.
    pub fn final_host(&self) -> Option<String> {
        self.final_url.as_deref().and_then(source::host_of)
    }

    /// Length and hash of the content shown.
    pub fn content_state(&self) -> ContentState {
        self.state
    }

    /// Re-checks against other code (e.g. after choosing the pinned commit over the
    /// working tree), keeping the current step.
    pub fn recheck(&mut self, checker: &Checker<'_>) {
        self.report = check_with_reader(&self.investigation, checker.reader, checker.oracle);
    }

    // --- Steps and navigation ---

    pub fn step_count(&self) -> usize {
        self.investigation.steps.len()
    }

    /// Index of the current step (0 when there are no steps).
    pub fn current_index(&self) -> usize {
        self.current
    }

    /// The current step's node id; `None` for a diagram or overlay without steps.
    pub fn current_id(&self) -> Option<&str> {
        self.investigation
            .steps
            .get(self.current)
            .map(String::as_str)
    }

    pub fn current_step(&self) -> Option<StepView<'_>> {
        self.step(self.current)
    }

    /// Moves to the next step. False (and no move) at the last one.
    pub fn next_step(&mut self) -> bool {
        self.go_to_index(self.current + 1)
    }

    /// Moves to the previous step. False (and no move) at the first one.
    pub fn previous_step(&mut self) -> bool {
        self.current > 0 && self.go_to_index(self.current - 1)
    }

    pub fn first_step(&mut self) -> bool {
        self.go_to_index(0)
    }

    pub fn last_step(&mut self) -> bool {
        self.step_count() > 0 && self.go_to_index(self.step_count() - 1)
    }

    /// Moves to step `index`. False if out of range.
    pub fn go_to_index(&mut self, index: usize) -> bool {
        if index < self.step_count() {
            self.current = index;
            true
        } else {
            false
        }
    }

    /// Moves to the step for node `id`. If the id appears more than once in `steps`, the
    /// occurrence nearest the current step wins. False if no step has that id.
    pub fn go_to_step(&mut self, id: &str) -> bool {
        match nearest(&self.investigation.steps, id, self.current) {
            Some(i) => self.go_to_index(i),
            None => false,
        }
    }

    /// Step `index`, resolved against the check report and the chosen tour branch.
    pub fn step(&self, index: usize) -> Option<StepView<'_>> {
        const NONE: &[String] = &[];
        const NO_CALLERS: &[Caller] = &[];
        let inv = &self.investigation;
        let step_id = inv.steps.get(index)?;
        let derived = self.v2().and_then(|v| v.steps.get(index));
        let over = self
            .branch_variant()
            .and_then(|b| b.rows.get(step_id.as_str()));
        let (id, not_reached) = match over {
            Some(o) => (o.step.as_deref(), o.missing.as_deref()),
            None => (Some(step_id.as_str()), None),
        };
        let number = derived.map_or_else(|| (index + 1).to_string(), |d| d.number.clone());
        let mut view = StepView {
            index,
            id: id.unwrap_or(step_id),
            step_id,
            node: None,
            external: None,
            anchor: None,
            title: step_id.clone(),
            status: None,
            range: None,
            region: None,
            line: None,
            note: None,
            messages: NONE,
            number,
            name: None,
            values: over.map_or(NONE, |o| o.values.as_slice()),
            author_evidence: None,
            reached: id.is_some(),
            not_reached,
            depth: None,
            called_by: NO_CALLERS,
        };
        let Some(id) = id else {
            view.note = over.and_then(|o| o.note.as_deref());
            return Some(view);
        };
        if id == step_id
            && let Some(d) = derived
        {
            view.depth = d.depth;
            view.called_by = &d.called_by;
        }
        match inv.lookup(id)? {
            Ok(node) => {
                let report = self.node_report(id)?;
                view.node = Some(node);
                view.anchor = Some(&node.anchor);
                view.title = node_title(node);
                view.status = Some(report.status.clone());
                view.range = report.range.clone();
                view.region = report.region.clone();
                view.line = report.line;
                view.note = node.note.as_deref();
                view.messages = &report.messages;
                view.name = node.name.as_deref();
                view.author_evidence = node.evidence;
                if over.is_none() {
                    view.values = &node.values;
                }
            }
            Err(ext) => {
                view.external = Some(ext);
                view.title = ext.label.clone();
                view.note = ext.note.as_deref();
                view.name = ext.name.as_deref();
                view.author_evidence = ext.evidence;
                if over.is_none() {
                    view.values = &ext.values;
                }
            }
        }
        if let Some(o) = over.filter(|o| o.note.is_some()) {
            view.note = o.note.as_deref();
        }
        Some(view)
    }

    /// What step `index`'s focus page shows: its written view, else the default page
    /// (editor at the step plus Context). A step the branch replaces shows the default
    /// page of the node put in its place; a step not reached shows no panes.
    pub fn step_page(&self, index: usize) -> Option<ViewReport> {
        let step = self.step(index)?;
        if !step.reached {
            return Some(ViewReport {
                step: step.step_id.into(),
                arrangement: "single".into(),
                default: true,
                fallback: None,
                panes: vec![],
            });
        }
        if step.id == step.step_id
            && let Some(v) = self
                .v2()
                .and_then(|v| v.views.iter().find(|v| v.step == step.id))
        {
            return Some(v.clone());
        }
        Some(default_page(&self.investigation, &self.report, step.id))
    }

    /// The version 2 parts of the check report; `None` for a version 1 file.
    pub fn v2(&self) -> Option<&V2Report> {
        self.report.v2.as_ref()
    }

    // --- Tour branches ---

    /// The tour's branches, as written.
    pub fn branches(&self) -> &[Variant] {
        &self.investigation.variants
    }

    /// The branch chosen; `None` for the main path.
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    fn branch_variant(&self) -> Option<&Variant> {
        let id = self.branch.as_deref()?;
        self.investigation.variants.iter().find(|v| v.id == id)
    }

    /// Chooses a branch (`None` for the main path). False, and no change, for an unknown id.
    pub fn set_branch(&mut self, id: Option<&str>) -> bool {
        match id {
            None => {
                self.branch = None;
                true
            }
            Some(id) if self.investigation.variants.iter().any(|v| v.id == id) => {
                self.branch = Some(id.into());
                true
            }
            Some(_) => false,
        }
    }

    /// Every step under the chosen branch: which node it shows and whether it is reached.
    pub fn branch_steps(&self) -> Vec<BranchStep> {
        branch_steps(&self.investigation, self.branch_variant())
    }

    // --- Comparisons ---

    /// The comparison as checked, with no variant chosen.
    pub fn comparison(&self) -> Option<&ComparisonReport> {
        self.v2().and_then(|v| v.comparison.as_ref())
    }

    /// Every comparison row, resolved per side with the chosen variants. Empty unless the
    /// investigation is a comparison.
    pub fn comparison_rows(&self) -> Vec<RowReport> {
        if self.comparison().is_none() {
            return vec![];
        }
        resolve_rows(&self.investigation, &self.report, &self.variants)
    }

    /// The variant chosen for `side`; `None` for its base path.
    pub fn variant(&self, side: &str) -> Option<&str> {
        self.variants.get(side).map(String::as_str)
    }

    /// Chooses a variant of `side` (`None` for its base path). False, and no change, for an
    /// unknown side or variant.
    pub fn set_variant(&mut self, side: &str, variant: Option<&str>) -> bool {
        let Some(s) = self.investigation.sides.iter().find(|s| s.id == side) else {
            return false;
        };
        match variant {
            None => {
                self.variants.remove(side);
                true
            }
            Some(v) if s.variants.iter().any(|x| x.id == v) => {
                self.variants.insert(side.into(), v.into());
                true
            }
            Some(_) => false,
        }
    }

    /// Every step, in order.
    pub fn steps(&self) -> Vec<StepView<'_>> {
        (0..self.step_count())
            .filter_map(|i| self.step(i))
            .collect()
    }

    pub fn node_report(&self, id: &str) -> Option<&NodeReport> {
        self.report.nodes.iter().find(|n| n.id == id)
    }

    // --- Edges ---

    /// Edges between nodes `a` and `b`, in either direction, in file order.
    pub fn edges_between(&self, a: &str, b: &str) -> Vec<EdgeView<'_>> {
        self.investigation
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| (e.from == a && e.to == b) || (e.from == b && e.to == a))
            .map(|(i, e)| EdgeView {
                index: i,
                from: &e.from,
                to: &e.to,
                rel: e.rel,
                verification: self
                    .report
                    .edges
                    .get(i)
                    .map_or(Verification::Claimed, |r| r.verification),
                note: e.note.as_deref(),
            })
            .collect()
    }

    /// For a call stack: the edges linking step `index - 1` to step `index`, in either
    /// direction. Empty for the first step, or when the file declares no such edge.
    pub fn edges_into_step(&self, index: usize) -> Vec<EdgeView<'_>> {
        let steps = &self.investigation.steps;
        match (
            index.checked_sub(1).and_then(|p| steps.get(p)),
            steps.get(index),
        ) {
            (Some(prev), Some(cur)) => self.edges_between(prev, cur),
            _ => vec![],
        }
    }

    // --- Live reload ---

    /// Re-reads the file and compares length and content hash with what is shown. Only a
    /// file source is re-read; nothing is fetched, checked or swapped in.
    pub fn check_for_update(&self) -> UpdateCheck {
        if !self.source.is_file() {
            return UpdateCheck::NotWatched;
        }
        match source::read(&self.source, self.root.as_deref()) {
            Ok(f) if f.state == self.state => UpdateCheck::Unchanged,
            Ok(_) => UpdateCheck::UpdateAvailable,
            Err(e) => UpdateCheck::Unreadable(e.to_string()),
        }
    }

    /// Takes the new version of the file if it parses and validates, then checks it. On any
    /// error the current version stays. The current step is kept if its id still exists
    /// (nearest occurrence to the old position), else the first step becomes current. URL
    /// sources are fetched again here, since the reader asked for it.
    pub fn reload(&mut self, checker: &Checker<'_>) -> Result<ReloadOutcome, SourceError> {
        let fetched = source::read(&self.source, self.root.as_deref())?;
        if fetched.state == self.state {
            return Ok(ReloadOutcome::Unchanged);
        }
        let investigation = source::parse_bytes(&fetched.bytes)?;
        let old_id = self.current_id().map(str::to_string);
        let kept = old_id
            .as_deref()
            .and_then(|id| nearest(&investigation.steps, id, self.current));
        self.report = check_with_reader(&investigation, checker.reader, checker.oracle);
        self.investigation = investigation;
        self.state = fetched.state;
        self.final_url = fetched.final_url;
        self.current = kept.unwrap_or(0);
        if let Some(b) = self.branch.take() {
            self.set_branch(Some(&b));
        }
        for (side, v) in std::mem::take(&mut self.variants) {
            self.set_variant(&side, Some(&v));
        }
        Ok(ReloadOutcome::Reloaded {
            kept_step: kept.is_some() || old_id.is_none(),
        })
    }
}

fn nearest(steps: &[String], id: &str, around: usize) -> Option<usize> {
    steps
        .iter()
        .enumerate()
        .filter(|(_, s)| *s == id)
        .min_by_key(|(i, _)| i.abs_diff(around))
        .map(|(i, _)| i)
}

/// The node's `label`, else its symbol, else its file name.
pub fn node_title(node: &Node) -> String {
    if let Some(l) = &node.label {
        return l.clone();
    }
    if let Some(s) = &node.anchor.symbol {
        return s.clone();
    }
    node.anchor
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&node.anchor.path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::WorktreeReader;
    use std::fs;

    struct Fixture {
        root: PathBuf,
        file: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).ok();
        }
    }

    fn fixture(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("cho-session-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".chorong/investigations")).unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "fn route() {\n    handle();\n}\nfn handle() {\n    enqueue();\n}\nfn enqueue() {}\n",
        )
        .unwrap();
        let file = root.join(".chorong/investigations/stack.json");
        fs::write(&file, stack(&["route", "handle", "enqueue"]).to_string()).unwrap();
        Fixture { root, file }
    }

    fn stack(steps: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "chorong": 1, "title": "Stack", "kind": "callstack", "repo": "local:/x", "rev": "abcdef1",
            "nodes": [
                { "id": "route", "anchor": { "path": "src/lib.rs", "symbol": "route" }, "note": "start" },
                { "id": "handle", "anchor": { "path": "src/lib.rs", "symbol": "handle", "excerpt": "enqueue()" }, "label": "Handle" },
                { "id": "enqueue", "anchor": { "path": "src/lib.rs", "symbol": "enqueue" } },
                { "id": "gone", "anchor": { "path": "src/lib.rs", "symbol": "missing" } }
            ],
            "edges": [
                { "from": "route", "to": "handle", "rel": "calls" },
                { "from": "handle", "to": "enqueue", "rel": "calls", "note": "queued" },
                { "from": "enqueue", "to": "gone", "rel": "calls" }
            ],
            "steps": steps
        })
    }

    fn open(f: &Fixture) -> InvestigationSession {
        let reader = WorktreeReader { root: &f.root };
        InvestigationSession::open(
            Source::Path(f.file.clone()),
            Some(&f.root),
            &Checker {
                reader: &reader,
                oracle: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn steps_resolve_and_navigate() {
        let f = fixture("nav");
        let mut s = open(&f);
        assert_eq!(s.step_count(), 3);
        let first = s.current_step().unwrap();
        assert_eq!(
            (first.id, first.title.as_str(), first.note),
            ("route", "route", Some("start"))
        );
        assert_eq!(first.status, Some(NodeStatus::Resolved));
        assert_eq!(first.number, "1");
        assert_eq!(first.range, Some(0..28));
        assert!(!s.previous_step());
        assert!(s.next_step());
        let handle = s.current_step().unwrap();
        assert_eq!(handle.title, "Handle");
        // The excerpt narrows the range to `enqueue()` inside `handle`.
        let text = cho_core::fs::read_source_text(&f.root.join("src/lib.rs")).unwrap();
        assert_eq!(&text[handle.range.clone().unwrap()], "enqueue()");
        assert_eq!(handle.line, Some(5));
        assert!(s.last_step());
        assert!(!s.next_step());
        assert_eq!(s.current_id(), Some("enqueue"));
        assert!(s.first_step());
        assert!(s.go_to_step("handle"));
        assert_eq!(s.current_index(), 1);
        assert!(!s.go_to_step("gone"));
        assert!(!s.go_to_index(9));
        assert_eq!(s.steps().len(), 3);
    }

    #[test]
    fn edges_per_step_and_per_pair() {
        let f = fixture("edges");
        let s = open(&f);
        assert!(s.edges_into_step(0).is_empty());
        let into = s.edges_into_step(2);
        assert_eq!(into.len(), 1);
        assert_eq!(
            (into[0].from, into[0].to, into[0].note),
            ("handle", "enqueue", Some("queued"))
        );
        assert_eq!(into[0].verification, Verification::Claimed);
        let broken = s.edges_between("gone", "enqueue");
        assert_eq!(broken[0].verification, Verification::Broken);
        assert_eq!(broken[0].index, 2);
        assert!(s.edges_between("route", "enqueue").is_empty());
    }

    #[test]
    fn update_check_and_reload() {
        let f = fixture("reload");
        let mut s = open(&f);
        s.go_to_step("handle");
        assert_eq!(s.check_for_update(), UpdateCheck::Unchanged);

        // Same length, different content: still noticed (not only mtime).
        let original = cho_core::fs::read_source_text(&f.file).unwrap();
        let swapped = original.replace("\"Stack\"", "\"Stalk\"");
        assert_eq!(swapped.len(), original.len());
        fs::write(&f.file, &swapped).unwrap();
        assert_eq!(s.check_for_update(), UpdateCheck::UpdateAvailable);
        // Reporting never swaps anything in.
        assert_eq!(s.investigation().title, "Stack");

        let reader = WorktreeReader { root: &f.root };
        let checker = Checker {
            reader: &reader,
            oracle: None,
        };
        assert_eq!(
            s.reload(&checker).unwrap(),
            ReloadOutcome::Reloaded { kept_step: true }
        );
        assert_eq!(s.investigation().title, "Stalk");
        assert_eq!(s.current_id(), Some("handle"));
        assert_eq!(s.reload(&checker).unwrap(), ReloadOutcome::Unchanged);

        // A half-written file keeps the current version.
        fs::write(&f.file, &swapped[..swapped.len() / 2]).unwrap();
        assert_eq!(s.check_for_update(), UpdateCheck::UpdateAvailable);
        assert!(s.reload(&checker).is_err());
        assert_eq!(s.investigation().title, "Stalk");

        // The step moves when its id moves, and falls back to the first when it's gone.
        fs::write(&f.file, stack(&["enqueue", "handle"]).to_string()).unwrap();
        assert_eq!(
            s.reload(&checker).unwrap(),
            ReloadOutcome::Reloaded { kept_step: true }
        );
        assert_eq!(s.current_index(), 1);
        fs::write(&f.file, stack(&["route", "enqueue"]).to_string()).unwrap();
        assert_eq!(
            s.reload(&checker).unwrap(),
            ReloadOutcome::Reloaded { kept_step: false }
        );
        assert_eq!(s.current_id(), Some("route"));

        fs::remove_file(&f.file).unwrap();
        assert!(matches!(s.check_for_update(), UpdateCheck::Unreadable(_)));
    }

    #[test]
    fn urls_are_not_watched() {
        let f = fixture("url");
        let reader = WorktreeReader { root: &f.root };
        let bytes = stack(&["route"]).to_string().into_bytes();
        let fetched = Fetched {
            state: ContentState::of(&bytes),
            bytes,
            final_url: Some("https://cdn.example.net/s.json".into()),
        };
        let s = InvestigationSession::from_fetched(
            Source::url("https://example.com/s.json").unwrap(),
            None,
            fetched,
            &Checker {
                reader: &reader,
                oracle: None,
            },
        )
        .unwrap();
        assert_eq!(s.check_for_update(), UpdateCheck::NotWatched);
        assert_eq!(s.source().host().as_deref(), Some("example.com"));
        assert_eq!(s.final_host().as_deref(), Some("cdn.example.net"));
    }
}

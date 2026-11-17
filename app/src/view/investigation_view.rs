//! An open investigation in the viewer: loading it, asking which code to use when the checkout
//! has moved since its commit (drift), checking it, stepping through it, the note above the
//! code, live reload, and the Open Investigation… picker. The two panes that show it are
//! `tour_pane` (left dock) and `callstack_pane` (bottom bar).
//!
//! One investigation is active per window. Everything about the file itself (sources, labels,
//! listing, recents, inert notes) is in `crate::investigation`; this module is the GPUI side.
//! Reading and checking run on the background executor, with a generation guard.

use super::shell_host::{Redraw, pane_of};
use super::*;
use crate::investigation::{
    self as inv, CodeAt, Drift, Inline, Label, LinkKind, Source, Stamp, Step,
};
use cho_investigation::{Investigation, Kind};
use cho_layout::{DockId, PaneContent, PaneKind};

/// How often live reload looks at the open file's stamp (one `stat`), besides window
/// activation.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// The bottom bar's height when a comparison first opens in it (logical pixels).
const COMPARISON_BAR_H: f32 = 420.;

/// The investigation a window shows.
pub(crate) struct InvState {
    pub source: Source,
    pub inv: Arc<Investigation>,
    /// The worktree the anchors are relative to.
    pub root: PathBuf,
    pub drift: Drift,
    /// The pinned commit's full id, when this repository has it.
    pub pinned: Option<String>,
    /// For a URL source, the host it was fetched from (after redirects), shown in the pane.
    pub host: Option<String>,
    /// Which code the labels describe; `None` while the reader is asked (drift).
    pub at: Option<CodeAt>,
    /// The steps, labelled; empty until checked. For a comparison, one per row
    /// (`crate::comparison::row_steps`).
    pub steps: Arc<Vec<Step>>,
    /// The last check, kept so choosing a branch or a variant redraws the steps without
    /// checking again.
    pub report: Option<Arc<cho_investigation::check::CheckReport>>,
    /// The tour branch and each comparison side's variant, remembered per investigation.
    pub choice: crate::comparison::Choice,
    /// Format version 2's summary bullets and "could not be traced" list.
    pub overview: Arc<crate::tour_pages::Overview>,
    /// The Tour pane's header area shows the overview in full.
    pub overview_open: bool,
    /// The tour, opened as focus pages.
    pub pages: Option<super::tour_pages_view::TourPages>,
    pub checking: bool,
    /// The open step and the list's selection (zero-based).
    pub current: usize,
    pub selected: usize,
    /// The file's stamp when it was read, and a newer version that parses (Updated · Reload).
    pub stamp: Option<Stamp>,
    pub updated: Option<(Arc<Investigation>, Option<Stamp>)>,
    /// The tour filters the file tree (the Tour files chip).
    pub tree_filter: bool,
    /// The Comparison pane's groups, filter and scroll.
    pub cmp: super::comparison_pane::CmpView,
    pub scroll: UniformListScrollHandle,
    /// The Tour or Call stack list's stretch at its ends.
    pub over: crate::overscroll::Overscroll,
    pub generation: Generation,
}

impl InvState {
    pub fn kind(&self) -> &Kind {
        &self.inv.kind
    }

    /// `3/6`, once checked.
    pub fn position(&self) -> Option<String> {
        (!self.steps.is_empty()).then(|| format!("{}/{}", self.current + 1, self.steps.len()))
    }
}

/// Where a loaded investigation's anchors are read: its worktree, how the checkout relates to
/// its commit, and that commit's full id when present.
struct Placed {
    root: Option<PathBuf>,
    drift: Drift,
    pinned: Option<String>,
    host: Option<String>,
}

/// Where a step opens in an editor: the file, the caret, and what is tinted.
pub(crate) struct EditorAt {
    pub path: String,
    pub caret: Option<usize>,
    pub region: Option<std::ops::Range<usize>>,
    pub highlights: Vec<crate::tour_pages::Highlight>,
    /// On a comparison row's page: the row and which of its cells this editor shows.
    pub cell: Option<(Arc<crate::comparison::RowInfo>, usize)>,
    /// A diff pane: the file between two revisions, with the comparison view's marks.
    pub diff: Option<crate::tour_pages::DiffAt>,
}

/// What the note strip above an editor shows: the step a tour or call stack opened there.
#[derive(Clone)]
pub(crate) struct StepMark {
    /// The file (absolute), so the tint applies only while it is on screen.
    pub path: PathBuf,
    /// Short commit when the step was opened at the pinned commit.
    pub at_rev: Option<String>,
    /// The lines the anchor covers (tinted).
    pub region: Option<std::ops::Range<usize>>,
    /// What the anchor pins inside its declaration (an `excerpt`, or `from`..`to`): its lines
    /// get a stronger tint. `None` when the anchor is the whole declaration, or when the step
    /// has highlights (the anchor then only names the stop; each highlight pins its own).
    pub pinned: Option<std::ops::Range<usize>>,
    pub n: usize,
    pub total: usize,
    pub title: String,
    pub note: Option<Arc<Vec<Vec<Inline>>>>,
    /// Why the anchor is broken; then nothing is tinted.
    pub broken: Option<String>,
    /// What the anchor asked for: `Worker::drain in src/worker.rs`.
    pub anchor: String,
    pub callstack: bool,
    /// What the step shows as its number (`3`, `S1`).
    pub number: String,
    /// The author's evidence and runtime values: claims, styled as such.
    pub evidence: Option<crate::tour_pages::Evidence>,
    pub runtime: Vec<String>,
    /// Several highlighted chunks, each tinted with a numbered note strip above its first
    /// line; empty when the anchor's own region is tinted.
    pub highlights: Arc<Vec<crate::tour_pages::Highlight>>,
    /// A step that is not in the code: what it is (`a database`).
    pub external: Option<String>,
    /// An action step: its card replaces the code until the editor opens a file.
    pub action: Option<Arc<crate::tour_pages::ActionCard>>,
    /// A comparison side: the row (its result) and the cell this editor shows.
    pub cell: Option<(Arc<crate::comparison::RowInfo>, usize)>,
}

impl StepMark {
    /// The byte ranges tinted in the editor: each highlight's declaration (or pinned range),
    /// else the anchor's region.
    pub fn tinted(&self) -> Vec<std::ops::Range<usize>> {
        if self.highlights.is_empty() {
            self.region.clone().into_iter().collect()
        } else {
            self.highlights.iter().map(|h| h.area()).collect()
        }
    }

    /// The ranges tinted darker: each highlight's excerpt or `from`..`to` range, else the
    /// anchor's own.
    pub fn pinned_ranges(&self) -> Vec<std::ops::Range<usize>> {
        if self.highlights.is_empty() {
            self.pinned.clone().into_iter().collect()
        } else {
            self.highlights.iter().filter_map(|h| h.pinned()).collect()
        }
    }
}

impl StepMark {
    /// Whether the mark belongs to `doc`.
    pub fn applies_to(&self, doc: &Document) -> bool {
        doc.path == self.path && doc.at_rev == self.at_rev
    }
}

impl Chorong {
    // ---- Opening ----

    /// Opens an investigation in this window: read, parse, find its worktree, then ask about
    /// drift or check. A file for another repository goes to that repository's window.
    pub(super) fn open_investigation(&mut self, source: Source, cx: &mut Context<Self>) {
        let window_root = self.root.clone();
        let src = source.clone();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let (parsed, host) = inv::read_with_host(&src, window_root.as_deref())?;
                    let file = match &src {
                        Source::Path(p) => Some(p.clone()),
                        Source::Url(_) => None,
                    };
                    let known = cho_sources::recent::list();
                    let root = inv::worktree_for(&parsed, file.as_deref(), &known);
                    let drift_root = root.clone().or(window_root.clone());
                    let (drift, pinned) = match drift_root.as_deref() {
                        Some(r) => (
                            inv::drift(r, &parsed.rev),
                            inv::pinned_commit(r, &parsed.rev),
                        ),
                        None => (
                            Drift::NotGitRepo {
                                reason: "no repository".into(),
                            },
                            None,
                        ),
                    };
                    Ok::<_, String>((parsed, root, drift, pinned, host, inv::stamp(&src)))
                })
                .await;
            this.update(cx, |this, cx| match loaded {
                Err(e) => this.notify_text(e, cx),
                Ok((parsed, root, drift, pinned, host, stamp)) => this.investigation_loaded(
                    source,
                    parsed,
                    Placed {
                        root,
                        drift,
                        pinned,
                        host,
                    },
                    stamp,
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    fn investigation_loaded(
        &mut self,
        source: Source,
        parsed: Investigation,
        placed: Placed,
        stamp: Option<Stamp>,
        cx: &mut Context<Self>,
    ) {
        let Placed {
            root,
            drift,
            pinned,
            host,
        } = placed;
        if !matches!(parsed.kind, Kind::Tour | Kind::Callstack | Kind::Comparison) {
            self.notify_text(
                format!(
                    "{} is a {}: this version shows tours, call stacks and comparisons only",
                    parsed.title,
                    inv::kind_word(&parsed.kind)
                ),
                cx,
            );
            return;
        }
        let same = |a: &std::path::Path, b: &std::path::Path| {
            a.canonicalize().ok() == b.canonicalize().ok()
        };
        let root = match (root, self.root.clone()) {
            (Some(r), Some(mine)) if !same(&r, &mine) && !source.is_file() => {
                // A URL is fetched by the window it lands in; it is not handed on (a second
                // fetch, and a link would come back here).
                self.notify_text(
                    format!(
                        "{} is about {}, open in {}. Open that folder, then open the link again.",
                        parsed.title,
                        parsed.repo,
                        r.display()
                    ),
                    cx,
                );
                return;
            }
            (Some(r), Some(mine)) if !same(&r, &mine) => {
                // Its repository is open elsewhere (or not yet): that window shows it.
                let link = cho_core::ChorongLink::Investigation {
                    src: inv::source_key(&source),
                }
                .to_url();
                crate::send_link(link, cx);
                return;
            }
            (Some(r), _) => r,
            (None, Some(mine)) => mine,
            (None, None) => {
                self.notify_text(
                    format!(
                        "{} is not open in Chorong on this machine. Open a clone of it once, then open the investigation again.",
                        parsed.repo
                    ),
                    cx,
                );
                return;
            }
        };
        // Outside git there is no commit to compare: the working tree, without asking.
        let at = (!matches!(drift, Drift::Differs { .. })).then_some(CodeAt::WorkingTree);
        if crate::devlog::on() {
            eprintln!(
                "INV opened kind={} drift={}",
                inv::kind_word(&parsed.kind),
                if at.is_some() { "same" } else { "ask" }
            );
        }
        let mut generation = self
            .inv
            .as_ref()
            .map_or(Generation::default(), |s| s.generation);
        generation.next();
        let tree_filter = parsed.kind == Kind::Tour;
        let choice = crate::comparison::valid_choice(
            &parsed,
            crate::comparison::load_choice(&inv::source_key(&source)),
        );
        self.inv = Some(InvState {
            source: source.clone(),
            inv: Arc::new(parsed),
            root,
            drift,
            pinned,
            host,
            at,
            steps: Arc::default(),
            report: None,
            choice,
            overview: Arc::default(),
            overview_open: false,
            pages: None,
            checking: false,
            current: 0,
            selected: 0,
            stamp,
            updated: None,
            tree_filter,
            cmp: Default::default(),
            scroll: UniformListScrollHandle::new(),
            over: Default::default(),
            generation,
        });
        if let (Some(repo), Some(store)) = (self.repo_root(), inv::store())
            && let Err(e) = store.record_opened(&repo, &source, inv::now_secs())
        {
            self.notify_text(format!("Could not remember the investigation: {e}"), cx);
        }
        self.show_investigation_pane(cx);
        self.start_watch(cx);
        if self.inv.as_ref().is_some_and(|s| s.at.is_some()) {
            self.check_investigation(None, true, cx);
        }
        self.redraw(None, cx);
        self.redraw_step_headers(cx);
    }

    /// This window's repository, canonical, as recents are keyed.
    fn repo_root(&self) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        Some(root.canonicalize().unwrap_or_else(|_| root.clone()))
    }

    /// Shows the pane for the active investigation's kind (a tour above the tree, a call stack
    /// in the bottom bar), making it when absent, and gives it the keyboard.
    fn show_investigation_pane(&mut self, cx: &mut Context<Self>) {
        let Some(st) = &self.inv else { return };
        let (kind, content) = match st.kind() {
            Kind::Tour => (
                PaneKind::Tour,
                PaneContent::Tour {
                    investigation: inv::source_key(&st.source),
                    title: "Tour".into(),
                },
            ),
            Kind::Comparison => (
                PaneKind::Comparison,
                PaneContent::Comparison {
                    investigation: inv::source_key(&st.source),
                    title: "Comparison".into(),
                },
            ),
            _ => (
                PaneKind::CallStack,
                PaneContent::CallStack {
                    investigation: inv::source_key(&st.source),
                    title: "Call stack".into(),
                },
            ),
        };
        let existing = pane_of(self.shell.read(cx).workspace(), kind);
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| {
                let pane = match existing {
                    Some(p) => p,
                    None => {
                        // A tour sits above the file tree (layout.md, "Tours").
                        let above_tree = (kind == PaneKind::Tour && !ws.in_focus())
                            .then(|| pane_of(ws, PaneKind::FileTree))
                            .flatten()
                            .and_then(|t| ws.group_of(t))
                            .map(|group| cho_layout::Target::Beside {
                                group,
                                edge: cho_layout::Edge::Top,
                            });
                        ws.open(content, above_tree)?
                    }
                };
                let dock = match kind {
                    PaneKind::Tour => DockId::Left,
                    _ => DockId::Bottom,
                };
                if !ws.in_focus() && !ws.arrangement().dock(dock).visible {
                    let _ = ws.toggle_dock(dock);
                }
                // A comparison is a table: the bottom bar is made tall enough for a few rows.
                if kind == PaneKind::Comparison
                    && existing.is_none()
                    && ws.arrangement().dock(dock).size < COMPARISON_BAR_H
                {
                    let _ = ws.resize_dock(dock, COMPARISON_BAR_H);
                }
                ws.activate(pane)
            });
        });
        self.focus_to(KeyFocus::Steps);
    }

    /// Checks the investigation against the chosen code in the background, then opens the
    /// remembered step (or `keep`, a step id to stay on after a reload), else the first.
    /// With `reopen` false (the index changed) only the labels are refreshed: the reader's
    /// place in the code is left alone.
    fn check_investigation(&mut self, keep: Option<String>, reopen: bool, cx: &mut Context<Self>) {
        let Some(st) = &mut self.inv else { return };
        let Some(at) = st.at.clone() else { return };
        st.generation.next();
        st.checking = true;
        let generation = st.generation;
        let (investigation, root, src) = (st.inv.clone(), st.root.clone(), st.source.clone());
        let choice = st.choice.clone();
        // The index describes the working tree: it may confirm calls in a working-tree check
        // while it is current (as the status bar says), whatever commit the investigation was
        // made at. The pinned commit is read from git, which no index here describes.
        let oracle = match (&self.index, &at) {
            (IndexState::Ready(sem, IndexStatus::Current), CodeAt::WorkingTree) => {
                Some(sem.clone())
            }
            _ => None,
        };
        cx.spawn(async move |this, cx| {
            let steps = cx
                .background_spawn(async move {
                    let report = inv::check_at(
                        &investigation,
                        &root,
                        &at,
                        oracle
                            .as_deref()
                            .map(|s| s as &dyn cho_investigation::check::EdgeOracle),
                    );
                    let (steps, overview) =
                        inv::derive_steps(&investigation, &report, &at, &choice);
                    (steps, overview, report)
                })
                .await;
            let (steps, overview, report) = steps;
            this.update(cx, |this, cx| {
                let Some(st) = &mut this.inv else { return };
                if !st.generation.is(generation) {
                    return;
                }
                let place = keep.or_else(|| inv::store().and_then(|s| s.place(&src)));
                let ix = place
                    .and_then(|id| steps.iter().position(|s| s.id == id))
                    .filter(|ix| inv::reached(&steps, *ix))
                    .or_else(|| inv::reached_ends(&steps).map(|(first, _)| first))
                    .unwrap_or(0);
                st.steps = Arc::new(steps);
                st.report = Some(Arc::new(report));
                st.overview = Arc::new(overview);
                st.checking = false;
                // A comparison opens on its pane; Enter opens a row.
                let reopen = reopen && *st.kind() != Kind::Comparison;
                if reopen {
                    this.step_open(ix, cx);
                } else {
                    st.current = ix;
                    st.selected = ix;
                    this.redraw(Some(Redraw::Steps), cx);
                    this.redraw_step_headers(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The index changed (loaded, rebuilt): an investigation checked against the working tree
    /// is checked again, staying on its step, so calls the index confirms become verified.
    pub(super) fn investigation_index_changed(&mut self, cx: &mut Context<Self>) {
        self.tour_graphs_stale();
        let Some(st) = &self.inv else { return };
        if st.at != Some(CodeAt::WorkingTree) {
            return;
        }
        let keep = st.steps.get(st.current).map(|s| s.id.clone());
        self.check_investigation(keep, false, cx);
    }

    /// Reopens the investigation a saved layout's Tour or Call stack pane names, once per
    /// window start.
    pub(super) fn restore_investigation(&mut self, cx: &mut Context<Self>) {
        if self.inv.is_some() {
            return;
        }
        let ws = self.shell.read(cx).workspace();
        let src = ws.panes().values().find_map(|c| match c {
            PaneContent::Tour { investigation, .. }
            | PaneContent::CallStack { investigation, .. }
            | PaneContent::Comparison { investigation, .. } => Some(investigation.clone()),
            _ => None,
        });
        // Only a file is reopened by itself: a URL is fetched only when asked.
        if let Some(Ok(source @ Source::Path(_))) = src.as_deref().map(Source::from_link_src) {
            self.open_investigation(source, cx);
        }
    }

    /// Close Investigation: forgets it and closes its panes (Tour, Call stack, Comparison),
    /// which have nothing to show without it.
    pub(super) fn close_investigation(&mut self, cx: &mut Context<Self>) {
        if self.inv.take().is_none() {
            self.notify_text("No investigation is open", cx);
            return;
        }
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| {
                let panes: Vec<PaneId> = ws
                    .panes()
                    .iter()
                    .filter(|(_, c)| {
                        matches!(
                            c.kind(),
                            PaneKind::Tour | PaneKind::CallStack | PaneKind::Comparison
                        )
                    })
                    .map(|(p, _)| *p)
                    .collect();
                for p in panes {
                    // One that will not close (parked, already gone) leaves the rest closing.
                    let _ = ws.close(p);
                }
                Ok(())
            });
        });
        for ed in self.editors.values_mut() {
            ed.step_mark = None;
            if let Some(f) = &mut ed.folds {
                f.set_notes(Vec::new());
            }
        }
        self.key_focus.leave(KeyFocus::Steps);
        self.redraw(None, cx);
    }

    // ---- Drift ----

    /// The pane headers (`3/6`, the Tour files chip) are drawn by the shell, not the bodies.
    pub(super) fn redraw_step_headers(&mut self, cx: &mut Context<Self>) {
        self.shell.update(cx, |_, cx| cx.notify());
    }

    /// The reader chose which code to use.
    pub(super) fn choose_code(&mut self, pinned: bool, cx: &mut Context<Self>) {
        let Some(st) = &mut self.inv else { return };
        st.at = match (&st.pinned, pinned) {
            (Some(r), true) => Some(CodeAt::Pinned(r.clone())),
            _ => Some(CodeAt::WorkingTree),
        };
        self.check_investigation(None, true, cx);
        self.redraw(None, cx);
    }

    /// The question while the code is not chosen: the text and whether the pinned commit can
    /// be read.
    pub(super) fn drift_question(&self) -> Option<(String, bool)> {
        let st = self.inv.as_ref()?;
        if st.at.is_some() {
            return None;
        }
        let Drift::Differs { head, .. } = &st.drift else {
            return None;
        };
        let rev = cho_git::short(&st.inv.rev);
        let here = head
            .as_deref()
            .map_or("no commit".into(), |h| cho_git::short(h).to_string());
        Some(match &st.pinned {
            Some(_) => (
                format!(
                    "Made at {rev}; this checkout is at {here}. Which code should the labels describe?"
                ),
                true,
            ),
            None => (
                format!(
                    "Made at {rev}, which is not in this repository (the checkout is at {here}). Fetch it to see the pinned code, or use the working tree."
                ),
                false,
            ),
        })
    }

    // ---- Stepping ----

    /// Opens step `ix` in the active editor: the file at the anchor, the covered lines tinted,
    /// the note above. A broken step opens its file if it exists, untinted, with the reason.
    pub(super) fn step_open(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(st) = &mut self.inv else { return };
        if st.at.is_none() {
            self.notify_text("Choose which code to use first", cx);
            return;
        }
        let Some(step) = st.steps.get(ix).cloned() else {
            return;
        };
        if let Some(why) = &step.extras.not_reached {
            st.selected = ix;
            let text = format!("Step {} is not reached on this path: {why}", step.number);
            self.notify_text(text, cx);
            self.redraw(Some(Redraw::Steps), cx);
            return;
        }
        let comparison = *st.kind() == Kind::Comparison;
        st.current = ix;
        st.selected = ix;
        st.scroll
            .scroll_to_item(inv::item_of(&st.steps, ix), ScrollStrategy::Center);
        let src = st.source.clone();
        if let Some(store) = inv::store() {
            // Best effort: losing the place is not worth interrupting the reader.
            let _ = store.set_place(&src, &step.id, inv::now_secs());
        }
        // On tour focus pages the step is its page.
        if self.tour_page_show(ix, cx) {
            return;
        }
        // A comparison row opens as a focus page with one editor per side.
        if comparison {
            self.open_tour_pages_with(false, cx);
            self.redraw(Some(Redraw::Steps), cx);
            self.redraw_step_headers(cx);
            return;
        }
        let highlights = first_editor_highlights(&step);
        let editor = EditorAt {
            path: step.path.clone(),
            // With highlights, the anchor only names the stop: open on the first one.
            caret: highlights
                .first()
                .map(|h| h.range.start)
                .or_else(|| step.caret()),
            region: step.region.clone(),
            highlights,
            cell: None,
            diff: None,
        };
        self.show_step_in_active(&step, editor, cx);
        let pane = self.active_editor;
        self.dirty_pane = Some(pane);
        self.redraw(Some(Redraw::Steps), cx);
        self.redraw(Some(Redraw::Editor(pane)), cx);
        self.redraw_step_headers(cx);
        self.redraw_chrome(cx);
        cx.notify();
    }

    /// Shows `step` in the active editor at `editor`: the file at the anchor, the covered lines
    /// (or each highlight) tinted, the note above. A broken step opens its file if it exists,
    /// untinted, with the reason.
    pub(super) fn show_step_in_active(
        &mut self,
        step: &Step,
        editor: EditorAt,
        cx: &mut Context<Self>,
    ) {
        let Some(st) = &self.inv else { return };
        let (root, at, total) = (st.root.clone(), st.at.clone(), st.steps.len());
        let callstack = *st.kind() == Kind::Callstack;
        let file = root.join(&editor.path);
        let pinned = match &at {
            Some(CodeAt::Pinned(rev)) => Some(rev.clone()),
            _ => None,
        };
        // A comparison side's editor is made only for an anchor that resolved.
        let broken =
            step.label == Label::Broken && editor.path == step.path && editor.cell.is_none();
        let side = editor
            .cell
            .as_ref()
            .and_then(|(r, k)| r.cells.get(*k).cloned());
        let mut mark = StepMark {
            path: file.clone(),
            at_rev: pinned.as_deref().map(|r| cho_git::short(r).to_string()),
            region: editor.region.clone().filter(|_| !broken),
            pinned: step.range.clone().filter(|r| {
                !broken
                    && editor.highlights.is_empty()
                    && editor.path == step.path
                    && editor.region == step.region
                    && Some(r) != step.region.as_ref()
            }),
            n: step.n,
            total,
            title: match &side {
                Some(c) => format!(
                    "{} \u{b7} {}",
                    c.heading(),
                    c.text.clone().unwrap_or_default()
                ),
                None => step.list_name().to_string(),
            },
            note: side
                .as_ref()
                .map_or(step.note.as_deref(), |c| c.note.as_deref())
                .map(|n| Arc::new(inv::note_blocks(n))),
            broken: broken.then(|| step.reason.clone().unwrap_or_default()),
            anchor: match &step.symbol {
                Some(sym) => format!("{sym} in {}", step.path),
                None => step.path.clone(),
            },
            callstack,
            number: step.number.clone(),
            evidence: step.extras.evidence,
            runtime: side
                .as_ref()
                .map_or_else(|| step.extras.runtime.clone(), |c| c.values.clone()),
            highlights: Arc::new(editor.highlights),
            external: step.extras.external.clone(),
            // Only the step's own editor (no file): an editor its view adds shows code.
            action: step
                .extras
                .action
                .clone()
                .filter(|_| editor.path.is_empty()),
            cell: editor.cell.clone(),
        };
        if crate::devlog::on() {
            eprintln!(
                "INV step={}/{total} id={} label={} at={} highlights={}",
                step.n,
                step.id,
                step.label.word(),
                if pinned.is_some() {
                    "pinned"
                } else {
                    "worktree"
                },
                mark.highlights.len()
            );
            if let Some(c) = &side {
                eprintln!("INV side={} path={}", c.letter, editor.path);
            }
        }
        if step.is_action() && !editor.path.is_empty() {
            // Code an action's page asks you to watch: the card beside it says what to do.
            mark.note = None;
        }
        if let Some(d) = editor.diff.clone() {
            // A diff pane: the file's change between two revisions, not the step's anchor.
            mark.title = format!("Changes in {} \u{b7} {}", d.path, d.sides());
            mark.region = None;
            mark.broken = None;
            mark.highlights = Arc::new(Vec::new());
            mark.note = None;
            self.ed_mut().step_mark = Some(mark);
            self.open_diff(d, cx);
            self.sync_note_rows();
            return;
        }
        // The last jump's outline goes now, not when the step's file has loaded.
        self.ed_mut().landed = None;
        self.ed_mut().step_mark = Some(mark);
        let target = Target {
            byte: editor.caret,
            step: true,
            ..Target::default()
        };
        if step.is_external() || (step.is_action() && editor.path.is_empty()) {
            // Nothing to open: the editor shows the stop's card over its file (Back returns
            // to the file).
            self.remember_caret();
            self.sync_note_rows();
            return;
        }
        match pinned {
            Some(rev) => self.open_at_commit(file, editor.path.clone(), rev, target, cx),
            None if file.is_file() => self.open(file, target, true, cx),
            None => {}
        }
        self.sync_note_rows();
        // The strips just added sit above the caret's line: keep it in the middle.
        if self
            .ed()
            .doc
            .as_ref()
            .zip(self.ed().step_mark.as_ref())
            .is_some_and(|(d, m)| m.applies_to(d))
        {
            self.center_caret();
        }
    }

    /// The step's file at the pinned commit, read from git (read-only), in the active editor.
    fn open_at_commit(
        &mut self,
        file: PathBuf,
        rel: String,
        rev: String,
        target: Target,
        cx: &mut Context<Self>,
    ) {
        let short = cho_git::short(&rev).to_string();
        if self
            .ed()
            .doc
            .as_ref()
            .is_some_and(|d| d.path == file && d.at_rev.as_deref() == Some(short.as_str()))
        {
            self.remember_caret();
            self.apply_target(&target, cx);
            self.record_history();
            return;
        }
        let Some(top) = self.inv.as_ref().map(|s| s.root.clone()) else {
            return;
        };
        let display = self.display_of(&file);
        let theme = self.theme.clone();
        let json = self.features.on(Feature::JsonFolds);
        self.remember_caret();
        let generation = self.ed_mut().generation.next();
        let pane = self.active_editor;
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let bytes = cho_git::read_file_at(&top, &rev, &rel)?;
                    let index_ok = mismatch::rev_index_matches(&top, &rev);
                    let doc = Document::from_bytes(file, display, bytes, Some(short), &theme, json)
                        .map_err(|e| e.to_string())?;
                    Ok::<_, String>((doc, index_ok))
                })
                .await;
            this.update(cx, |this, cx| {
                if !this.editor_is(pane, generation) {
                    return;
                }
                match loaded {
                    Ok((doc, index_ok)) => {
                        this.with_pane(pane, |this| {
                            this.install(doc);
                            this.ed_mut().rev_index_ok = index_ok;
                            this.apply_target(&target, cx);
                            this.record_history();
                        });
                        this.sync_editor_pane(pane, cx);
                        this.refresh_context(cx);
                    }
                    Err(e) => this.notify_text(e, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the folds inside the open step's region (or each of its highlighted chunks), so
    /// its code reads without a click.
    pub(super) fn reveal_step_region(&mut self) {
        let ed = self.ed_mut();
        let (Some(doc), Some(folds), Some(mark)) = (&ed.doc, &mut ed.folds, &ed.step_mark) else {
            return;
        };
        if !mark.applies_to(doc) {
            return;
        }
        for region in mark.tinted() {
            let (first, last) = (
                doc.buffer.line_of(region.start),
                doc.buffer
                    .line_of(region.end.saturating_sub(1).max(region.start)),
            );
            for line in first..=last.min(first + 2000) {
                folds.reveal(line);
            }
        }
    }

    /// Alt-] / Alt-[ / Alt-Home / Alt-End: go to a step and open it, from anywhere.
    pub(super) fn go_step(&mut self, to: StepTo, cx: &mut Context<Self>) {
        let Some(st) = &self.inv else {
            self.notify_text(
                "No tour or call stack is open: use Open Investigation\u{2026}",
                cx,
            );
            return;
        };
        // Steps the chosen branch does not reach are skipped.
        let Some((first, last)) = inv::reached_ends(&st.steps) else {
            return;
        };
        let ix = match to {
            StepTo::Next => inv::step_to_reached(&st.steps, st.current, 1),
            StepTo::Prev => inv::step_to_reached(&st.steps, st.current, -1),
            StepTo::First => first,
            StepTo::Last => last,
        };
        self.step_open(ix, cx);
    }

    /// j / k, Down / Up in a step pane: moves the selection.
    pub(super) fn step_select(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(st) = &mut self.inv else { return };
        st.selected = if *st.kind() == Kind::Comparison {
            // Over the rows the pane shows (a group may be collapsed, or a filter on).
            let shown = super::comparison_pane::shown_rows(&super::comparison_pane::cmp_items(
                &st.steps, &st.cmp,
            ));
            let to = super::comparison_pane::step_shown(&shown, st.selected, delta)
                .unwrap_or(st.selected);
            st.cmp.scroll.scroll_to_item(
                super::comparison_pane::cmp_items(&st.steps, &st.cmp)
                    .iter()
                    .position(|i| *i == super::comparison_pane::CmpItem::Row(to))
                    .unwrap_or(0),
            );
            to
        } else {
            inv::step_to_reached(&st.steps, st.selected, delta)
        };
        st.scroll
            .scroll_to_item(inv::item_of(&st.steps, st.selected), ScrollStrategy::Center);
        self.redraw(Some(Redraw::Steps), cx);
        self.skip_refresh = true;
    }

    /// Enter in a step pane.
    pub(super) fn step_open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.inv.as_ref().map(|s| s.selected) {
            self.step_open(ix, cx);
            self.focus_to(KeyFocus::Steps);
        }
    }

    /// Esc in a step pane: the keyboard goes back to the code.
    pub(super) fn steps_leave(&mut self, cx: &mut Context<Self>) {
        self.key_focus.leave(KeyFocus::Steps);
        self.redraw(Some(Redraw::Steps), cx);
        self.skip_refresh = true;
    }

    /// A click on a step row: opens it, and the pane keeps the keyboard. In the Comparison
    /// pane a click selects the row (Enter opens it), unless its pages are open.
    pub(super) fn step_clicked(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.focus_to(KeyFocus::Steps);
        if let Some(st) = &mut self.inv
            && *st.kind() == Kind::Comparison
            && st.pages.is_none()
        {
            st.selected = ix;
            self.redraw(Some(Redraw::Steps), cx);
            return;
        }
        self.step_open(ix, cx);
    }

    /// The status-bar field: `Tour 3 of 6`; for a comparison, the row by its own number
    /// (`Comparison row S1`), as the table and the strip show it.
    pub(super) fn step_status(&self) -> Option<String> {
        let st = self.inv.as_ref()?;
        if *st.kind() == Kind::Comparison {
            let row = st.steps.get(st.current)?;
            return Some(format!(
                "Comparison row {} \u{b7} {} rows",
                row.number,
                st.steps.len()
            ));
        }
        let what = match st.kind() {
            Kind::Tour => "Tour",
            Kind::Comparison => "Comparison row",
            _ => "Call stack",
        };
        (!st.steps.is_empty()).then(|| format!("{what} {} of {}", st.current + 1, st.steps.len()))
    }

    // ---- Live reload ----

    /// Polls the open file's stamp while an investigation is open (and the setting is on).
    fn start_watch(&mut self, cx: &mut Context<Self>) {
        if self.inv_watch.is_some() {
            return;
        }
        self.inv_watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_EVERY).await;
                let go_on = this
                    .update(cx, |this, cx| {
                        if this.inv.is_none() {
                            this.inv_watch = None;
                            return false;
                        }
                        this.watch_tick(cx);
                        true
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
        }));
    }

    /// One look at the file: a `stat` on the UI thread (timed as the `investigations.watch`
    /// feature); when it changed, the new version is read and parsed in the background and
    /// offered (Updated · Reload) only if it parses. Also called on window activation.
    pub(super) fn watch_tick(&mut self, cx: &mut Context<Self>) {
        if !self.features.on(Feature::InvestigationsWatch) {
            return;
        }
        let Some(st) = &self.inv else { return };
        let now = {
            let _t = self.features.time(Feature::InvestigationsWatch);
            inv::stamp(&st.source)
        };
        let known = st.updated.as_ref().map_or(st.stamp, |(_, s)| *s);
        if now.is_none() || now == known {
            return;
        }
        let (source, current, generation, root) = (
            st.source.clone(),
            st.inv.clone(),
            st.generation,
            st.root.clone(),
        );
        let cost = self.features.probe(Feature::InvestigationsWatch);
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_spawn(async move {
                    let _t = cost.background();
                    inv::read(&source, Some(&root)).ok()
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(st) = &mut this.inv else { return };
                if !st.generation.is(generation) {
                    return;
                }
                match parsed {
                    // Half-written or invalid: keep the current one, look again later.
                    None => {}
                    Some(p) if p == *current => st.stamp = now,
                    Some(p) => {
                        if crate::devlog::on() {
                            eprintln!("INV updated");
                        }
                        st.updated = Some((Arc::new(p), now));
                        this.redraw(Some(Redraw::Steps), cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reload: takes the newer version, keeping the current step if its id still exists.
    pub(super) fn reload_investigation(&mut self, cx: &mut Context<Self>) {
        let Some(st) = &mut self.inv else {
            self.notify_text("No investigation is open", cx);
            return;
        };
        let Some((newer, stamp)) = st.updated.take() else {
            self.notify_text("The investigation has not changed", cx);
            return;
        };
        let keep = st.steps.get(st.current).map(|s| s.id.clone());
        st.inv = newer;
        st.stamp = stamp;
        self.check_investigation(keep, true, cx);
        self.redraw(None, cx);
    }

    // ---- Open Investigation… ----

    /// The picker: the repository's `.chorong/investigations/` first, then recents, then a row to
    /// open any file. Rows are listed (and checked) in the background.
    pub(super) fn open_investigation_picker(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.open_investigation_file(cx);
            return;
        };
        let recents = self
            .repo_root()
            .zip(inv::store())
            .map(|(r, s)| s.recents(&r))
            .unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_spawn(async move { inv::list(&root, &recents) })
                .await;
            this.update(cx, |this, cx| {
                let (mut labels, mut hints, mut picks) = (Vec::new(), Vec::new(), Vec::new());
                for r in rows {
                    labels.push(r.title.clone());
                    hints.push(r.hint());
                    picks.push(if r.missing || r.error.is_some() {
                        Pick::Unavailable(r.hint())
                    } else {
                        Pick::Investigation(r.source)
                    });
                }
                labels.push("Open Investigation File\u{2026}".into());
                hints.push("any .json file".into());
                picks.push(Pick::InvestigationFile);
                this.open_overlay_picker(PickerKind::Investigations, labels, hints, picks);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open Investigation File…: the system's Open dialog for any `.json` file.
    pub(super) fn open_investigation_file(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open Investigation".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    this.open_investigation(Source::Path(path), cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// A link in a note was clicked: `chorong://` goes through the viewer's own link handling,
    /// `https://` to the browser, anything else nowhere.
    pub(super) fn note_link(&mut self, target: &str, kind: LinkKind, cx: &mut Context<Self>) {
        match kind {
            LinkKind::Chorong => crate::send_link(target.to_string(), cx),
            LinkKind::Web => cx.open_url(target),
            LinkKind::Inert => self.notify_text(format!("Not opened: {target}"), cx),
        }
    }

    // ---- Shared rendering ----

    /// The note strip above an editor showing a step: the step number, its note (inert), or
    /// why its anchor is broken.
    /// In place of the code, while the editor's step is a node that is not in the code: the
    /// same card a tour page shows.
    pub(super) fn render_external_stop(
        &self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let mark = self.editors.get(&pane)?.step_mark.as_ref()?;
        if let Some(card) = &mark.action {
            let heading = format!("{} \u{b7} step {} of {}", mark.title, mark.n, mark.total);
            return Some(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_action_card(
                        card,
                        &mark.number,
                        heading,
                        pane.0 as usize,
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        let what = mark.external.clone()?;
        let c = crate::pane_style::colors(&self.theme, cx);
        let note = mark.note.clone();
        Some(
            div()
                .flex_1()
                .min_h_0()
                .child(super::tour_pages_view::external_card(
                    &mark.title,
                    &what,
                    note.map(|n| self.render_note(&n, ("ext-note", pane.0 as usize), cx)),
                    mark.evidence,
                    &mark.runtime,
                    &c,
                ))
                .into_any_element(),
        )
    }

    /// The banner's text for a step: the external stop's line, the broken-anchor message, or
    /// the title line with evidence, runtime values, row result, note and chunk list.
    fn step_note_text(
        &self,
        pane: PaneId,
        mark: &StepMark,
        c: &cho_layout_ui::colors::Colors,
        error: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> Div {
        match &mark.broken {
            _ if mark.external.is_some() => div().font_weight(FontWeight::MEDIUM).child(format!(
                "{} \u{b7} {} {} of {}",
                mark.title,
                if mark.callstack { "frame" } else { "step" },
                mark.n,
                mark.total
            )),
            Some(reason) => div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_color(error)
                        .font_weight(FontWeight::MEDIUM)
                        .child(small_icon("triangle-alert", error))
                        .child("Anchor not found"),
                )
                .child(format!(
                    "{} asked for {} ({}). Nothing was put in its place.",
                    if mark.callstack {
                        "The call stack"
                    } else {
                        "The tour"
                    },
                    mark.anchor,
                    mark.title
                ))
                .child(div().text_color(c.text_muted).child(reason.clone())),
            None => div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .child(div().font_weight(FontWeight::MEDIUM).child(
                            if mark.cell.is_some() {
                                // A comparison row goes by its own number (S1, 1…), as in the
                                // table.
                                format!("{} \u{b7} row {}", mark.title, mark.number)
                            } else {
                                format!(
                                    "{} \u{b7} {} {} of {}",
                                    mark.title,
                                    if mark.callstack { "frame" } else { "step" },
                                    mark.n,
                                    mark.total
                                )
                            },
                        ))
                        .children(mark.evidence.map(|e| evidence_tag(e, c)))
                        .children(mark.runtime.iter().map(|v| runtime_chip(v, c))),
                )
                .children(
                    mark.cell
                        .as_ref()
                        .map(|(row, _)| self.render_row_result(row, c)),
                )
                .children(
                    mark.note
                        .clone()
                        .map(|n| self.render_note(&n, ("note", pane.0 as usize), cx)),
                )
                .children(self.render_chunk_list(pane, mark, c, cx)),
        }
    }

    /// The banner's body for `body` (the text) beside `badge`: scrolling, measured after layout
    /// and clamped to the reader's height, plus the "More" chip when text is below the fold.
    fn measured_note_body(
        &self,
        pane: PaneId,
        badge: Div,
        body: Div,
        c: &cho_layout_ui::colors::Colors,
        cx: &mut Context<Self>,
    ) -> (Div, Option<gpui::Stateful<Div>>) {
        let Some(ed) = self.editors.get(&pane) else {
            return (body, None);
        };
        let (scroll, over) = (ed.note_scroll.clone(), ed.note_over.clone());
        // Measured after layout: when the text's height changes, store it and redraw this
        // pane (panes are cached views, so a window refresh alone would wait for the next
        // event), so the banner fits a shorter step's text on the next frame.
        let fit = ed.note_fit.clone();
        let measure = {
            let fit = fit.clone();
            let this = cx.entity().downgrade();
            gpui::canvas(
                |_, _, _| {},
                move |b, (), _, cx| {
                    let h = f32::from(b.size.height).max(18.) + NOTE_PAD_Y * 2.;
                    if (h - fit.get()).abs() > 0.5 {
                        fit.set(h);
                        let this = this.clone();
                        cx.defer(move |cx| {
                            let _ = this.update(cx, |this, cx| {
                                this.redraw(Some(Redraw::Editor(pane)), cx);
                            });
                        });
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        let inner = div()
            .px_3()
            .py(px(NOTE_PAD_Y))
            .flex()
            .items_start()
            .gap_2()
            .text_size(px(12.5))
            .child(badge)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .child(body)
                    .child(measure),
            );
        let body = crate::scroll::ScrollArea::with(scroll, over).vertical(
            ("step-note", pane.0 as usize),
            self.features.probe_if_on(Feature::Overscroll),
            inner,
        );
        // The reader's height (or the cap), but never more than the text needs.
        let want = self.note_height.unwrap_or(NOTE_FIT_MAX);
        let body = match fit.get() {
            f if f > 0. => body.h(px(want.min(f))),
            _ => body.max_h(px(want)),
        };
        // Text below the fold: a chip that grows the banner to show it all (up to the drag
        // limit; beyond that it scrolls).
        let hidden = fit.get() > want + 0.5 && want < NOTE_MAX;
        let more = hidden.then(|| {
            let all = fit.get().min(NOTE_MAX);
            div()
                .id(("step-banner-more", pane.0 as usize))
                .absolute()
                .right(px(12.))
                .bottom(px(NOTE_HANDLE + 3.))
                .h(px(18.))
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(3.))
                .rounded(px(4.))
                .border_1()
                .border_color(c.border)
                .bg(c.panel)
                .text_size(px(11.))
                .text_color(c.text_muted)
                .cursor_pointer()
                .hover(|d| d.text_color(c.text).border_color(c.text_accent))
                .tooltip(super::tip("Show the whole note (or drag the edge)".into()))
                .child("More")
                .child(small_icon("chevron-down", c.text_muted))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.note_height = Some(all);
                    this.redraw(Some(Redraw::Editor(pane)), cx);
                }))
        });
        (body, more)
    }

    pub(super) fn render_step_note(
        &self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let ed = self.editors.get(&pane)?;
        let mark = ed.step_mark.as_ref()?;
        let on_screen = ed.doc.as_ref().is_some_and(|d| mark.applies_to(d));
        if !on_screen && mark.broken.is_none() && mark.external.is_none() {
            return None;
        }
        if mark.action.is_some() {
            // The card itself carries the stop line.
            return None;
        }
        let c = crate::pane_style::colors(&self.theme, cx);
        let error = c.error.unwrap_or(c.text_accent);
        let badge = step_badge(
            mark.number.clone(),
            if mark.broken.is_some() {
                BadgeLook::Broken
            } else {
                BadgeLook::Current
            },
            &c,
        );
        let body = self.step_note_text(pane, mark, &c, error, cx);
        let (body, more) = self.measured_note_body(pane, badge, body, &c, cx);
        Some(
            div()
                .id(("step-banner", pane.0 as usize))
                .relative()
                .flex_none()
                .flex()
                .flex_col()
                .border_b_1()
                .border_color(c.border)
                .bg(c.accent_faint)
                .on_drag_move::<NoteResize>(cx.listener(
                    move |this, e: &gpui::DragMoveEvent<NoteResize>, _, cx| {
                        if e.drag(cx).0 != pane {
                            return;
                        }
                        let h = f32::from(e.event.position.y - e.bounds.top()) - NOTE_HANDLE / 2.;
                        this.note_height = Some(h.clamp(NOTE_MIN, NOTE_MAX));
                        this.redraw(Some(Redraw::Editor(pane)), cx);
                    },
                ))
                .child(body)
                .child(
                    div()
                        .id(("step-banner-handle", pane.0 as usize))
                        .h(px(NOTE_HANDLE))
                        .w_full()
                        .flex_none()
                        .cursor_row_resize()
                        .hover(|d| d.bg(c.hover))
                        .tooltip(super::tip(
                            "Drag to resize; double-click for the default height".into(),
                        ))
                        .on_drag(NoteResize(pane), |_, _, _, cx| cx.new(|_| NoDragImage))
                        .on_click(cx.listener(|this, e: &gpui::ClickEvent, _, cx| {
                            if e.click_count() > 1 {
                                this.note_height = None;
                                this.redraw(None, cx);
                            }
                        })),
                )
                .children(more)
                .into_any_element(),
        )
    }

    /// Inert markdown: text, code spans, and links that show their target. Text wraps word by
    /// word at the pane's width: a text run in a wrapping row is otherwise one item that runs
    /// past it.
    pub(super) fn render_note(
        &self,
        blocks: &[Vec<Inline>],
        id: (&'static str, usize),
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let mut col = div().flex().flex_col().gap_1();
        for (b, block) in blocks.iter().enumerate() {
            let mut line = div().flex().flex_wrap().items_baseline();
            for (i, piece) in block.iter().enumerate() {
                let el = match piece {
                    Inline::Text(t) => {
                        line = line
                            .children(t.split_inclusive(' ').map(|w| {
                                div().whitespace_nowrap().child(w.replace(' ', "\u{a0}"))
                            }));
                        continue;
                    }
                    Inline::Styled { text, bold, italic } => {
                        let (bold, italic) = (*bold, *italic);
                        line = line.children(text.split_inclusive(' ').map(|w| {
                            div()
                                .whitespace_nowrap()
                                .when(bold, |d| d.font_weight(FontWeight::BOLD))
                                .when(italic, |d| d.italic())
                                .child(w.replace(' ', "\u{a0}"))
                        }));
                        continue;
                    }
                    Inline::Code(t) => div()
                        .px_1()
                        .rounded_sm()
                        .bg(c.hover)
                        .font_family(self.font.clone())
                        .text_size(px(12.))
                        .child(t.clone())
                        .into_any_element(),
                    Inline::Link { text, target, kind } => {
                        let shown = crate::note_md::link_shown(text, target);
                        let el = div()
                            .id((id.0, id.1 * 10_000 + b * 100 + i))
                            .tooltip(tip(target.clone()));
                        match kind {
                            LinkKind::Inert => el.text_color(c.text_muted).child(shown),
                            _ => {
                                let (target, kind) = (target.clone(), *kind);
                                el.text_color(c.text_accent)
                                    .underline()
                                    .cursor_pointer()
                                    .child(shown)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.note_link(&target, kind, cx);
                                    }))
                            }
                        }
                        .into_any_element()
                    }
                };
                line = line.child(el);
            }
            col = col.child(line);
        }
        col.into_any_element()
    }

    /// The drift question, the Updated · Reload notice and the empty and checking states, at
    /// the top of a step pane. `None` when there is nothing to say.
    /// Format version 2's summary bullets and "could not be traced" list, in a step pane's
    /// header area: one line that opens to the lists (shared by the Tour and Call stack
    /// panes). `None` when the file has neither.
    pub(super) fn render_inv_overview(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let st = self.inv.as_ref()?;
        let o = st.overview.clone();
        if o.bullets.is_empty() && o.untraced.is_empty() {
            return None;
        }
        let c = crate::pane_style::colors(&self.theme, cx);
        let open = st.overview_open;
        let mut line = vec![];
        if !o.bullets.is_empty() {
            line.push(format!(
                "Summary \u{b7} {} point{}",
                o.bullets.len(),
                if o.bullets.len() == 1 { "" } else { "s" }
            ));
        }
        if !o.untraced.is_empty() {
            line.push(format!("{} could not be traced", o.untraced.len()));
        }
        let warn = c.warning.unwrap_or(c.text_muted);
        let item = |text: &String, mark: &'static str, colour: gpui::Hsla| {
            div()
                .flex()
                .gap(px(6.))
                .child(div().flex_none().text_color(colour).child(mark))
                .child(div().flex_1().min_w_0().child(super::note_text_in(
                    &crate::note_md::Text::of(text, "\n"),
                    &c,
                )))
        };
        Some(
            div()
                .flex_none()
                .px_3()
                .py(px(4.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .border_b_1()
                .border_color(c.border_variant)
                .text_size(px(11.5))
                .child(
                    div()
                        .id("inv-overview")
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .text_color(c.text_muted)
                        .hover(|d| d.text_color(c.text))
                        .child(small_icon(
                            if open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            },
                            c.icon,
                        ))
                        .child(line.join(" \u{b7} "))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(st) = &mut this.inv {
                                st.overview_open = !st.overview_open;
                            }
                            this.redraw(Some(Redraw::Steps), cx);
                        })),
                )
                .when(open, |d| {
                    d.children(o.bullets.iter().map(|b| item(b, "\u{2022}", c.text_muted)))
                        .when(!o.untraced.is_empty(), |d| {
                            d.child(
                                div()
                                    .pt(px(2.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(warn)
                                    .child("Could not be traced (author's list)"),
                            )
                            .children(o.untraced.iter().map(|u| item(u, "?", warn)))
                        })
                })
                .into_any_element(),
        )
    }

    pub(super) fn render_inv_notices(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let st = self.inv.as_ref()?;
        let c = crate::pane_style::colors(&self.theme, cx);
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .px_2()
                .rounded_sm()
                .border_1()
                .border_color(c.border)
                .when(primary, |d| d.bg(c.element_bg))
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .child(label)
        };
        if let Some((text, can_pin)) = self.drift_question() {
            let rev = cho_git::short(&st.inv.rev).to_string();
            let mut row = div().flex().flex_wrap().gap_2();
            if can_pin {
                row = row.child(
                    button(
                        "inv-pinned",
                        format!("Pinned commit {rev} (read-only)"),
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.choose_code(true, cx))),
                );
            }
            row = row.child(
                button("inv-worktree", "Working tree".into(), !can_pin)
                    .on_click(cx.listener(|this, _, _, cx| this.choose_code(false, cx))),
            );
            return Some(banner_box(&c).child(text).child(row).into_any_element());
        }
        if st.updated.is_some() {
            return Some(
                div()
                    .flex_none()
                    .mx_2()
                    .my_1()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_sm()
                    .bg(c.accent_faint)
                    .text_size(px(12.))
                    .child(div().flex_1().child("Updated on disk"))
                    .child(
                        button("inv-reload", "Reload".into(), true)
                            .on_click(cx.listener(|this, _, _, cx| this.reload_investigation(cx))),
                    )
                    .into_any_element(),
            );
        }
        st.checking.then(|| {
            div()
                .px_3()
                .py_1()
                .text_size(px(11.5))
                .text_color(c.text_muted)
                .child("Checking anchors\u{2026}")
                .into_any_element()
        })
    }

    /// The pane's one-line key hint, from the live keymap.
    pub(super) fn render_step_hint(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let key = |name: &str, fallback: &str| {
            self.key_labels
                .get(name)
                .cloned()
                .unwrap_or_else(|| fallback.to_string())
        };
        let branches = self
            .inv
            .as_ref()
            .is_some_and(|s| !s.inv.variants.is_empty() && *s.kind() != Kind::Comparison);
        let text = format!(
            "j/k \u{b7} Enter open{} \u{b7} Esc code \u{b7} {} {} anywhere \u{b7} ? keys",
            if branches {
                format!(" \u{b7} {} path", key("chorong::NextBranch", "b"))
            } else {
                String::new()
            },
            key("chorong::NextStep", "Alt-]"),
            key("chorong::PrevStep", "Alt-["),
        );
        div()
            .flex_none()
            .px_3()
            .h(px(22.))
            .flex()
            .items_center()
            .border_t_1()
            .border_color(c.border)
            .text_size(px(11.))
            .text_color(c.text_muted)
            .whitespace_nowrap()
            .overflow_hidden()
            .child(text)
            .into_any_element()
    }

    /// Prev and next buttons for a step pane's header (the `3/6` between them is the pane's
    /// qualifier: headers are cached and redrawn when the qualifier changes). Only on the pane
    /// that shows the open investigation's kind.
    pub(super) fn step_header_actions(
        &self,
        pane: PaneKind,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let st = self.inv.as_ref()?;
        let shows = match st.kind() {
            Kind::Tour => PaneKind::Tour,
            Kind::Comparison => PaneKind::Comparison,
            _ => PaneKind::CallStack,
        };
        if pane != shows {
            return None;
        }
        let c = crate::pane_style::colors(&self.theme, cx);
        let arrow = |id: &'static str, icon: &'static str, to: StepTo, tipt: String| {
            div()
                .id(id)
                .w(px(20.))
                .h(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .tooltip(tip(tipt))
                .child(small_icon(icon, c.icon))
                .on_click(cx.listener(move |this, _, _, cx| this.go_step(to, cx)))
        };
        Some(
            div()
                .flex()
                .items_center()
                .child(arrow(
                    "step-prev",
                    "chevron-left",
                    StepTo::Prev,
                    self.tip_text("Previous step", "chorong::PrevStep"),
                ))
                .child(arrow(
                    "step-next",
                    "chevron-right",
                    StepTo::Next,
                    self.tip_text("Next step", "chorong::NextStep"),
                ))
                .into_any_element(),
        )
    }
}

/// Where Alt-] and friends go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepTo {
    Next,
    Prev,
    First,
    Last,
}

/// How a numbered stop badge looks (layout.md, "Tours"): done outlined in success, current
/// filled with the accent, to do outlined in border, broken outlined in error.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BadgeLook {
    Done,
    Current,
    ToDo,
    Broken,
}

impl From<inv::StopState> for BadgeLook {
    fn from(s: inv::StopState) -> Self {
        match s {
            inv::StopState::Done => BadgeLook::Done,
            inv::StopState::Current => BadgeLook::Current,
            inv::StopState::ToDo => BadgeLook::ToDo,
            inv::StopState::Broken => BadgeLook::Broken,
        }
    }
}

impl Chorong {
    /// Under a step's note, when it has several highlighted chunks: a numbered list of them in
    /// the author's order, each with its label, its commentary under it, and its line. A click
    /// scrolls to the chunk; `n` / `N` go through them in file order.
    fn render_chunk_list(
        &self,
        pane: PaneId,
        mark: &StepMark,
        c: &cho_layout_ui::colors::Colors,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if mark.highlights.len() < 2 {
            return None;
        }
        let doc = self.editors.get(&pane).and_then(|e| e.doc.as_ref());
        let rows = mark.highlights.iter().enumerate().map(|(i, h)| {
            let at = h.range.start;
            let label = match &h.label {
                Some(l) => crate::note_md::Text::of(l, " "),
                None => {
                    let mut t = crate::note_md::Text::default();
                    t.push(&format!("Highlight {}", i + 1), Default::default());
                    t
                }
            };
            let note = h.note.as_deref().map(|n| crate::note_md::Text::of(n, "\n"));
            let line = doc.map(|d| format!("line {}", d.buffer.line_of(h.range.start) + 1));
            div()
                .id(("chunk-row", pane.0 as usize * 1000 + i))
                .px(px(6.))
                .py(px(2.))
                .flex()
                .items_start()
                .gap(px(8.))
                .rounded(px(4.))
                .text_size(px(12.))
                .cursor_pointer()
                .hover(|s| s.bg(c.accent_faint))
                .tooltip(super::tip("Go to this highlight".into()))
                .child(
                    div()
                        .flex_none()
                        .w(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c.text_accent)
                        .child((i + 1).to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_color(c.text)
                                .child(super::note_text_in(&label, c)),
                        )
                        .children(note.map(|n| {
                            div()
                                .pt(px(1.))
                                .text_color(c.text_muted)
                                .child(super::note_text_in(&n, c))
                        })),
                )
                .children(line.map(|l| div().flex_none().text_color(c.text_muted).child(l)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_editor(pane, cx);
                    this.with_pane(pane, |this| {
                        this.place_caret(at, super::navigation::Reveal::Center, cx);
                    });
                    this.redraw(Some(Redraw::Editor(pane)), cx);
                }))
        });
        Some(div().pt(px(6.)).flex().flex_col().children(rows))
    }
}

/// The step banner's height when it fits its text, and the range a drag keeps it in.
const NOTE_FIT_MAX: f32 = 140.;
const NOTE_MIN: f32 = 32.;
const NOTE_MAX: f32 = 640.;
/// The strip under the banner that resizes it.
const NOTE_HANDLE: f32 = 5.;
/// The banner text's padding above and below.
const NOTE_PAD_Y: f32 = 6.;

/// Dragging the step banner's bottom edge, in this pane.
#[derive(Clone, Copy, PartialEq, Eq)]
struct NoteResize(PaneId);

/// A drag that draws nothing under the pointer.
struct NoDragImage;

impl Render for NoDragImage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub(crate) fn step_badge(
    n: impl Into<SharedString>,
    look: BadgeLook,
    c: &cho_layout_ui::colors::Colors,
) -> Div {
    let n: SharedString = n.into();
    // A two-character number (S1, 12) widens the badge into a pill.
    let d = div()
        .flex_none()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(if n.len() > 1 { 4. } else { 0. }))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .child(n);
    match look {
        BadgeLook::Current => d.bg(c.text_accent).text_color(c.editor),
        BadgeLook::Done => {
            let s = c.success.unwrap_or(c.text_accent);
            d.border(px(1.5)).border_color(s).text_color(s)
        }
        BadgeLook::ToDo => d
            .border(px(1.5))
            .border_color(c.border)
            .text_color(c.text_muted),
        BadgeLook::Broken => {
            let e = c.error.unwrap_or(c.text);
            d.border(px(1.5)).border_color(e).text_color(e)
        }
    }
}

/// A phase's heading in a step list: `Setup` or `Call`, small caps at the row's foot, the
/// same height as a row (the lists are uniform).
pub(crate) fn phase_heading(
    p: crate::tour_pages::Phase,
    h: f32,
    c: &cho_layout_ui::colors::Colors,
) -> gpui::AnyElement {
    div()
        .w_full()
        .h(px(h))
        .px_3()
        .pb(px(3.))
        .flex()
        .items_end()
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.text_muted)
        .child(match p {
            crate::tour_pages::Phase::Setup => "SETUP",
            crate::tour_pages::Phase::Call => "CALL",
        })
        .into_any_element()
}

/// The author's evidence (`read`, `inferred`): a small neutral tag beside Chorong's own label,
/// which it never changes. Tooltip-free so it reads the same in every pane.
pub(crate) fn evidence_tag(
    e: crate::tour_pages::Evidence,
    c: &cho_layout_ui::colors::Colors,
) -> gpui::AnyElement {
    div()
        .flex_none()
        .px(px(5.))
        .rounded(px(3.))
        .bg(c.hover)
        .text_size(px(10.5))
        .text_color(c.text_muted)
        .child(format!("author: {}", e.word()))
        .into_any_element()
}

/// A runtime value the author saw (`flag = false`): drawn like a claim, a dashed `warning`
/// outline in the code font, never like a verified fact.
pub(crate) fn runtime_chip(v: &str, c: &cho_layout_ui::colors::Colors) -> gpui::AnyElement {
    let warn = c.warning.unwrap_or(c.text_muted);
    div()
        .flex_none()
        .px(px(5.))
        .rounded(px(3.))
        .border_1()
        .border_dashed()
        .border_color(warn)
        .text_size(px(11.))
        .text_color(c.text)
        .child(v.to_string())
        .into_any_element()
}

/// The highlights of a step's first editor pane, when its view gives several.
pub(crate) fn first_editor_highlights(step: &Step) -> Vec<crate::tour_pages::Highlight> {
    step.extras
        .view
        .as_ref()
        .and_then(|v| {
            v.panes.iter().find_map(|p| match p {
                crate::tour_pages::PaneSpec::Editor {
                    path, highlights, ..
                } if *path == step.path => Some(highlights.clone()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

/// A box in the style of the commit-mismatch banner: accent edge, panel background.
pub(crate) fn banner_box(c: &cho_layout_ui::colors::Colors) -> Div {
    div()
        .flex_none()
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .gap_2()
        .border_b_1()
        .border_l_4()
        .border_color(c.text_accent)
        .bg(c.panel)
        .text_size(px(12.))
        .text_color(c.text)
}

/// An empty step pane: what to do to fill it.
pub(crate) fn empty_pane(what: &str, c: &cho_layout_ui::colors::Colors) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .px_3()
        .text_size(px(12.5))
        .text_color(c.text_muted)
        .child(format!("No {what} open."))
        .child("Open Investigation\u{2026} in the command palette lists this repository's.")
        .into_any_element()
}

use cho_layout_ui::widgets::small_icon;

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        CloseInvestigation,
        FirstStep,
        LastStep,
        NextStep,
        OpenInvestigation,
        OpenInvestigationFile,
        PrevStep,
        ReloadInvestigation,
        StepLeave,
        StepNext,
        StepOpen,
        StepPrev,
        ToggleTourFiles
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c(
        "chorong::OpenInvestigation",
        "Open Investigation\u{2026}",
        Layer::Go,
        115,
    ),
    c(
        "chorong::OpenInvestigationFile",
        "Open Investigation File\u{2026}",
        Layer::Go,
        116,
    ),
    c("chorong::NextStep", "Next Step", Layer::Steps, 350),
    c("chorong::PrevStep", "Previous Step", Layer::Steps, 351),
    c("chorong::FirstStep", "First Step", Layer::Steps, 352),
    c("chorong::LastStep", "Last Step", Layer::Steps, 353),
    c(
        "chorong::ToggleTourFiles",
        "Toggle Tour Files",
        Layer::Steps,
        354,
    ),
    c(
        "chorong::ReloadInvestigation",
        "Reload Investigation",
        Layer::Steps,
        355,
    ),
    c(
        "chorong::CloseInvestigation",
        "Close Investigation",
        Layer::Steps,
        356,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, OpenInvestigation, |this, _w, cx| this
        .open_investigation_picker(cx));
    let d = on!(d, cx, OpenInvestigationFile, |this, _w, cx| this
        .open_investigation_file(cx));
    let d = on!(d, cx, CloseInvestigation, |this, _w, cx| this
        .close_investigation(cx));
    let d = on!(d, cx, ReloadInvestigation, |this, _w, cx| this
        .reload_investigation(cx));
    let d = on!(d, cx, ToggleTourFiles, |this, _w, cx| this
        .toggle_tour_files(cx));
    let d = on!(d, cx, NextStep, |this, _w, cx| this
        .go_step(StepTo::Next, cx));
    let d = on!(d, cx, PrevStep, |this, _w, cx| this
        .go_step(StepTo::Prev, cx));
    let d = on!(d, cx, FirstStep, |this, _w, cx| this
        .go_step(StepTo::First, cx));
    let d = on!(d, cx, LastStep, |this, _w, cx| this
        .go_step(StepTo::Last, cx));
    let d = on!(d, cx, StepNext, |this, _w, cx| this.step_select(1, cx));
    let d = on!(d, cx, StepPrev, |this, _w, cx| this.step_select(-1, cx));
    let d = on!(d, cx, StepLeave, |this, _w, cx| this.steps_leave(cx));
    on!(d, cx, StepOpen, |this, _w, cx| this.step_open_selected(cx))
}

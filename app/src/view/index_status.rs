//! The semantic index in the background, call kinds, and the status bar.

use super::*;
use cho_layout_ui::widgets::small_icon;
use cho_semantic::progress;

/// The status bar as a view of its own, so GPUI reuses its last frame while the panes, the
/// shell or the chord popup redraw. It redraws when notified ([`Chorong::redraw_chrome`]): after
/// every change to the viewer or the layout, and on its own for changes that touch only the
/// chrome (a chord in progress, a notice running out).
pub(super) struct StatusBar {
    pub(super) cho: gpui::WeakEntity<Chorong>,
}

impl Render for StatusBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(cho) = self.cho.upgrade() else {
            return div().into_any_element();
        };
        cho.update(cx, |o, cx| {
            let dock_toggles = o
                .shell
                .update(cx, |shell, cx| shell.status_left(window, cx));
            let layout_notice = o.shell.read(cx).notice();
            // A view is laid out as a root: the bar fills the box the root view gave it.
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(o.render_status(dock_toggles, layout_notice, cx))
                .into_any_element()
        })
    }
}

impl Chorong {
    pub(super) fn load_index_in_background(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        // An index already in use keeps answering (labelled as it is) while the cache is re-read.
        if !matches!(self.index, IndexState::Ready(..)) {
            self.index = IndexState::Loading;
        }
        let generation = self.index_loads.next();
        cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async move { load_index(&root) }).await;
            this.update(cx, |this, cx| {
                // A newer load started meanwhile: its read of the cache is the newer one.
                if !this.index_loads.is(generation) {
                    return;
                }
                this.index = loaded.state;
                this.index_id = generation;
                this.packages = Arc::new(loaded.packages);
                this.index_failure = loaded.failure;
                // The pane and the lens may be waiting on the index.
                this.requery_context(cx);
                let panes: Vec<PaneId> = this.editors.keys().copied().collect();
                for pane in panes {
                    this.with_pane(pane, |this| this.refresh_call_kinds(cx));
                }
                this.load_start_facts(cx);
                this.maybe_keep_fresh(cx);
                // An open tour or call stack is checked again: the index can verify calls.
                this.investigation_index_changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Classifies the current file's calls with the index, off the UI thread.
    pub(super) fn refresh_call_kinds(&mut self, cx: &mut Context<Self>) {
        if !self.features.on(Feature::CallKinds) {
            self.ed_mut().call_kinds.clear();
            return;
        }
        let (Some(sem), Some(doc)) = (self.doc_index(), &self.ed().doc) else {
            return;
        };
        if doc.analysis.calls.is_empty() {
            return;
        }
        let cost = self.features.probe(Feature::CallKinds);
        let (sem, packages, path, epoch) = (
            sem.clone(),
            self.packages.clone(),
            doc.display_path.clone(),
            self.ed().epoch,
        );
        let pane = self.active_editor;
        let calls = doc.analysis.calls.clone();
        cx.spawn(async move |this, cx| {
            let kinds = cx
                .background_spawn(async move {
                    let _t = cost.background();
                    callkind::classify_calls(&sem, &packages, &path, &calls)
                })
                .await;
            this.update(cx, |this, cx| {
                // Only the pane that asked, and only while it still shows that document.
                let Some(ed) = this.editors.get_mut(&pane).filter(|e| e.epoch == epoch) else {
                    return;
                };
                ed.call_kinds = kinds;
                this.dirty_pane = Some(pane);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Text and click target of the status-bar index control.
    pub(super) fn index_label(&self) -> (String, bool) {
        // Lightweight mode turns indexing off (docs/public/settings.md, "Lightweight mode").
        if self.features.store().effective().lightweight() {
            return ("Index: off (Lightweight)".into(), false);
        }
        let status = self.index.status_text();
        if let Some(job) = &self.job {
            return (
                format!("Index: {status} \u{b7} {}", self.job_text(job)),
                true,
            );
        }
        if self.index_cancelled {
            return (format!("Index: {status} \u{b7} cancelled"), true);
        }
        match index_click(&self.index, self.index_failure.is_some()).hint() {
            Some(hint) => (format!("Index: {status} \u{b7} {hint}"), true),
            None => (format!("Index: {status}"), true),
        }
    }

    /// Redraws the status bar, and with it the window's root (the chord popup and Layout
    /// mode's hint bar), without redrawing any pane body.
    pub(super) fn redraw_chrome(&self, cx: &mut App) {
        self.status_bar.update(cx, |_, cx| cx.notify());
    }

    /// The status bar, as a cached view (see [`StatusBar`]).
    pub(super) fn status_bar_view(&self) -> AnyView {
        AnyView::from(self.status_bar.clone()).cached(
            gpui::StyleRefinement::default()
                .flex_none()
                .w_full()
                .h(px(24.)),
        )
    }

    /// What a running build is doing: `indexing packages/core (14 of 38) · 2m 10s`, with elapsed
    /// time always shown, or the cancel question or "cancelling…".
    pub(super) fn job_text(&self, job: &Job) -> String {
        let elapsed = job.started.elapsed();
        match job.cancel_state {
            Cancel::Cancelling => {
                return format!(
                    "cancelling\u{2026} \u{b7} {}",
                    progress::format_elapsed(elapsed)
                );
            }
            Cancel::Confirming(_) => {
                return format!(
                    "Cancel indexing? Click again to cancel \u{b7} {}",
                    progress::format_elapsed(elapsed)
                );
            }
            Cancel::No => {}
        }
        let verb = if job.automatic {
            "refreshing"
        } else {
            "indexing"
        };
        let mut text = progress::describe(verb, &job.current, &job.progress, elapsed);
        if job.total > 1 {
            text.push_str(&format!(
                " \u{b7} project {} of {}",
                (job.done + 1).min(job.total),
                job.total
            ));
        }
        text
    }

    pub(super) fn render_status(
        &self,
        dock_toggles: gpui::AnyElement,
        layout_notice: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let item = || div().flex().flex_none().items_center().gap(px(5.));
        let mut left: Vec<gpui::AnyElement> = Vec::new();
        let mut right: Vec<gpui::AnyElement> = Vec::new();
        if let Some(text) = self.open_progress() {
            left.push(
                item()
                    .id("remote-progress")
                    .px_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover))
                    .tooltip(tip("Show the Open Remote form".to_string()))
                    .child(small_icon("loader", c.icon))
                    .child(text)
                    .on_click(cx.listener(|this, _, _, cx| this.show_remote_form(cx)))
                    .into_any_element(),
            );
            left.push(
                item()
                    .id("remote-progress-cancel")
                    .px_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover))
                    .tooltip(tip("Stop this open".to_string()))
                    .child("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_open(cx)))
                    .into_any_element(),
            );
        }
        if let Some((text, running)) = self.prepare_text() {
            left.push(
                item()
                    .id("prepare-status")
                    .child(small_icon(
                        if running { "loader" } else { "package" },
                        c.icon,
                    ))
                    .child(text)
                    .into_any_element(),
            );
            if running {
                left.push(
                    item()
                        .id("prepare-cancel")
                        .px_1()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover))
                        .tooltip(tip(
                            "Stop preparing; the window stays syntax only".to_string()
                        ))
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_prepare(cx)))
                        .into_any_element(),
                );
            }
        }
        if self.ed().loading.is_some() {
            left.push(
                item()
                    .child(small_icon("loader", c.icon))
                    .child("Loading\u{2026}")
                    .into_any_element(),
            );
        }
        if let Some(doc) = &self.ed().doc {
            let name = doc
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            left.push(
                item()
                    .text_color(c.text)
                    .child(small_icon(crate::tree::file_icon(&name), c.icon))
                    .child(doc.language.map(|l| l.name()).unwrap_or("Plain text"))
                    .into_any_element(),
            );
            left.push(
                item()
                    .child(format!("{} lines", doc.lines.len()))
                    .into_any_element(),
            );
            // Data formats (JSON, TOML, XML) are highlighted, not analysed.
            if doc.language.is_some_and(|l| l.has_structure()) {
                left.push(if doc.analysis.has_error {
                    let warn = c.warning.unwrap_or(c.text);
                    item()
                        .text_color(warn)
                        .child(small_icon("triangle-alert", warn))
                        .child("partial parse")
                        .into_any_element()
                } else {
                    item().child("parse complete").into_any_element()
                });
            }
            left.push(
                item()
                    .child(format!("{} declarations", doc.symbols.len()))
                    .into_any_element(),
            );
            if let Some(cmp) = self
                .compare
                .as_ref()
                .filter(|_| self.ed().side == Side::Before || doc.at_rev.is_none())
            {
                left.push(
                    item()
                        .child(match (&doc.at_rev, self.ed().side) {
                            (Some(rev), Side::Before) => {
                                format!(
                                    "Before \u{b7} {rev} \u{b7} {} \u{b7} {}",
                                    if cmp.merge_bases == 0 {
                                        "as given, not the merge base".to_string()
                                    } else {
                                        format!("merge base with {}", cmp.against)
                                    },
                                    if self.ed().rev_index_ok {
                                        format!("index built at {rev}")
                                    } else {
                                        "syntax only".to_string()
                                    }
                                ) + &self.link_label()
                            }
                            _ => format!(
                                "After \u{b7} working folder{}{}",
                                match self.uncommitted {
                                    Some(0) => " \u{b7} clean".to_string(),
                                    Some(1) => {
                                        " \u{b7} 1 file differs from the last commit".to_string()
                                    }
                                    Some(n) =>
                                        format!(" \u{b7} {n} files differ from the last commit"),
                                    None => String::new(),
                                },
                                self.link_label()
                            ),
                        })
                        .into_any_element(),
                );
            } else if self.git_folder && self.remote_header.is_none() && doc.at_rev.is_none() {
                // Not comparing, in a git folder: one click away from Compare Against….
                left.push(
                    item()
                        .id("compare-start")
                        .px_1()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover))
                        .tooltip(tip(self.tip_text(
                            "Compare the whole repository with a branch, tag or commit",
                            "chorong::CompareAgainst",
                        )))
                        .child(small_icon("git-compare", c.icon))
                        .child("Compare\u{2026}")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_compare_picker(cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            } else if let Some(rev) = &doc.at_rev {
                left.push(
                    item()
                        .child(if self.ed().rev_index_ok {
                            format!("At {rev}, read from git \u{b7} index built at {rev}")
                        } else {
                            format!("At {rev}, read from git \u{b7} syntax only")
                        })
                        .into_any_element(),
                );
            }
            if let Some(step) = self.step_status() {
                right.push(item().child(step).into_any_element());
            }
            if let Some((line, col)) = self
                .ed()
                .caret
                .map(|caret| caret_line_col(&doc.buffer, caret))
            {
                right.push(
                    item()
                        .text_color(c.text)
                        .child(format!("Ln {line}, Col {col}"))
                        .into_any_element(),
                );
            }
            if let Some(mode) = self.ed().marks.last_mode() {
                right.push(
                    item()
                        .child(format!("Highlight: {}", mode.label()))
                        .into_any_element(),
                );
            }
            if self.lens {
                right.push(self.lens_chip(&c));
            }
            if self.graph_building {
                right.push(item().child("Building graph\u{2026}").into_any_element());
            }
            right.push(
                item()
                    .child(small_icon("lock", c.icon))
                    .child("Read-only")
                    .into_any_element(),
            );
        }
        let lightweight = self.features.store().effective().lightweight();
        if lightweight {
            // The Lightweight chip: `text.accent` at 16%, feather in `icon.accent`, Medium;
            // before `Read-only`, which is last when a file is open.
            let at = right.len() - usize::from(self.ed().doc.is_some());
            right.insert(
                at,
                item()
                    .px(px(6.))
                    .h(px(18.))
                    .rounded(px(4.))
                    .bg(c.accent_wash)
                    .text_color(c.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(small_icon("feather", c.icon_accent))
                    .child("Lightweight")
                    .into_any_element(),
            );
        }
        let (index_text, clickable) = self.index_label();
        let index_tip = if self.job.is_some() {
            self.tip_text(
                "Cancel indexing (the previous index is kept)",
                "chorong::CancelIndexing",
            )
        } else {
            self.tip_text("Build or refresh the index", "chorong::BuildIndex")
        };
        let host = self.host_button();
        let editor_tip = if host.is_some() {
            crate::remote::header::COPY_NOTE.to_string()
        } else if self.compare.is_some() && self.ed().side == Side::Before {
            self.tip_text(
                "Opens the working copy at the matching line",
                "chorong::OpenInEditor",
            )
        } else {
            self.tip_text("Open in the external editor", "chorong::OpenInEditor")
        };
        let error = self.ed().error.clone();
        let large_note = self
            .job
            .as_ref()
            .filter(|j| j.large && j.cancel_state != Cancel::Cancelling)
            .map(|_| crate::indexing::LARGE_NOTE);
        let notice = self
            .notice
            .as_ref()
            .map(|n| n.text.clone())
            .or(layout_notice);
        let look = if lightweight {
            IndexLook::None
        } else {
            index_look(&self.index, self.job.is_some())
        };
        let index_col = match look {
            IndexLook::Stale => c.warning,
            IndexLook::Failed => c.error,
            _ => None,
        };
        div()
            .flex_none()
            .h(px(24.))
            .pl_1()
            .pr_3()
            .flex()
            .items_center()
            .gap(px(14.))
            .bg(c.status_bar)
            .border_t_1()
            .border_color(c.border)
            .text_size(px(11.5))
            .text_color(c.text_muted)
            .whitespace_nowrap()
            .overflow_hidden()
            .child(dock_toggles)
            .children(left)
            .when(self.root.is_some(), |d| {
                d.child(
                    item()
                        .id("index-control")
                        .px_1()
                        .h(px(20.))
                        .rounded(px(4.))
                        .when_some(index_col, |d, col| d.text_color(col))
                        .child(index_dot(look, &c))
                        .child(index_text)
                        .when(clickable, |d| {
                            d.cursor_pointer()
                                .hover(|s| s.bg(c.hover))
                                .active(|s| s.bg(c.ghost_active))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.index_control(cx);
                                    cx.notify();
                                }))
                        })
                        .tooltip(tip(index_tip)),
                )
            })
            .children(large_note.map(|n| item().child(n)))
            .children(notice.map(|n| {
                item()
                    .text_color(c.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(n)
            }))
            .children(error.map(|e| {
                let col = c.error.unwrap_or(c.text);
                item()
                    .min_w_0()
                    .text_color(col)
                    .child(small_icon("triangle-alert", col))
                    .child(e)
            }))
            .child(div().flex_1())
            .children(right)
            .when(self.ed().doc.is_some(), |d| {
                d.child(
                    item()
                        .id("open-in-editor")
                        .px_1()
                        .h(px(20.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(c.text)
                        .hover(|s| s.bg(c.hover))
                        .active(|s| s.bg(c.ghost_active))
                        .tooltip(tip(editor_tip))
                        .child(small_icon("external-link", c.icon))
                        .child(match host {
                            Some(h) => format!("Open on {h}"),
                            None => format!("Open in {}", self.editor.name()),
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_in_editor(cx);
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_none().child(self.theme.name.clone()))
    }

    /// The Critical Lens chip: `text.accent` at 16%, and the call-kind legend once the index
    /// can tell kinds apart, each word in its own call style.
    fn lens_chip(&self, c: &cho_layout_ui::colors::Colors) -> gpui::AnyElement {
        let has_doc = self.ed().doc.is_some();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .px(px(6.))
            .h(px(18.))
            .rounded(px(4.))
            .bg(c.accent_wash)
            .text_color(c.text)
            .child("Critical Lens")
            .when(has_doc && self.doc_index().is_none(), |d| {
                d.child(
                    div()
                        .text_color(c.text_muted)
                        .child("call kinds need an index"),
                )
            })
            .when(has_doc && self.lens_kinds_known(self.ed()), |d| {
                d.child(div().text_color(c.text_muted).child("calls:"))
                    .child("repo")
                    .child(div().opacity(lens::EXTERNAL_OPACITY).child("external"))
                    .child(div().underline().child("unknown"))
            })
            .into_any_element()
    }
}

// ---- Manual start: the index banner (design/manual-start.md) ----

/// The window's part of manual start: nothing indexes or prepares until the reader clicks.
#[derive(Default)]
pub(crate) struct StartState {
    /// What the banner says, worked out off the UI thread after each index load.
    facts: Option<StartFacts>,
    /// "Keep fresh this session" was clicked: a stale index refreshes without asking until
    /// the window closes.
    pub(super) keep_fresh: bool,
    /// An automatic refresh was tried; it isn't tried again until the reader starts a build
    /// or clicks "Keep fresh" (a failed one is not retried by itself).
    pub(super) auto_tried: bool,
    /// The reader closed the banner of this kind for this window.
    dismissed: Option<BannerKind>,
}

/// What building one project runs ([`cho_semantic::indexer::build_plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemFacts {
    pub label: String,
    /// The indexer, as the reader would name it (`rust-analyzer scip`).
    pub runs: String,
    /// The project's own code the build runs (`build scripts, procedural macros`), if any.
    pub runs_code: Option<String>,
    /// Why this project is skipped, if it is.
    pub blocked: Option<String>,
    /// A local TypeScript project with no `node_modules` here (the folder, relative).
    pub no_node_modules: Option<String>,
}

/// A cache worktree's prepare recipe, for the remote banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteFacts {
    /// The recipe's commands, exactly (`Step::display`).
    pub commands: Vec<String>,
    /// The project's approval covers this commit.
    pub approved: bool,
}

/// Everything the banner says about this folder, read off the UI thread.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct StartFacts {
    pub items: Vec<ItemFacts>,
    /// Set for one of Chorong's cache worktrees (a remote project).
    pub remote: Option<RemoteFacts>,
    /// The last successful build of this folder, all projects together.
    pub last_build: Option<Duration>,
    /// A local folder the reader trusts (a cache worktree: its approval, in `remote`).
    pub trusted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerKind {
    /// No index yet.
    Missing,
    /// The index is out of date.
    Stale,
}

/// What a banner button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartAction {
    /// A local folder: build (asking for trust first when it isn't trusted).
    Build,
    /// A remote project: run its recipe, then index.
    PrepareAndIndex,
    /// A remote project: index without running the recipe.
    IndexOnly,
    /// Rebuild a stale index.
    Refresh,
    /// Refresh now, and again whenever the index is stale, until the window closes.
    KeepFresh,
    Dismiss,
    /// "Don't show again": turns `index.banner` off for every window.
    Hide,
}

/// The banner's words and buttons. The first action is the primary one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartBanner {
    pub kind: BannerKind,
    pub text: String,
    pub details: Vec<String>,
    pub warnings: Vec<String>,
    pub actions: Vec<(String, StartAction)>,
}

/// The window's state, as far as the banner cares.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StartInputs<'a> {
    pub facts: &'a StartFacts,
    pub kind: Option<BannerKind>,
    /// A build or a prepare is running, or the approval panel is up.
    pub busy: bool,
    pub lightweight: bool,
    /// A stale index refreshes by itself here ("Keep fresh" or `index.auto_refresh`).
    pub keep_fresh: bool,
    pub dismissed: Option<BannerKind>,
}

/// What the banner shows, if anything. Pure: the decisions of design/manual-start.md
/// (decisions 1, 2, 4, 5 and 15). Lightweight mode still shows it; it only hides "Keep fresh".
pub(crate) fn start_banner(i: StartInputs<'_>) -> Option<StartBanner> {
    let kind = i.kind?;
    let f = i.facts;
    if f.items.is_empty() || i.busy || i.dismissed == Some(kind) {
        return None;
    }
    if kind == BannerKind::Stale && i.keep_fresh {
        return None;
    }
    let mut details = Vec::new();
    let runnable: Vec<&ItemFacts> = f.items.iter().filter(|it| it.blocked.is_none()).collect();
    if let Some(r) = &f.remote
        && kind == BannerKind::Missing
        && !r.commands.is_empty()
    {
        details.push(format!("Prepare runs: {}", r.commands.join("; ")));
    }
    if !runnable.is_empty() {
        details.push(format!(
            "Indexing runs: {}",
            runnable
                .iter()
                .map(|it| format!("{} ({})", it.runs, it.label))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut why: Vec<&str> = Vec::new();
    for w in runnable.iter().filter_map(|it| it.runs_code.as_deref()) {
        if !why.contains(&w) {
            why.push(w);
        }
    }
    let recipe =
        kind == BannerKind::Missing && f.remote.as_ref().is_some_and(|r| !r.commands.is_empty());
    if recipe {
        details.push("The recipe runs the project's own code (package scripts, builds).".into());
    }
    if !why.is_empty() {
        details.push(format!("The project's own code runs: {}.", why.join("; ")));
    } else if !recipe {
        details.push("None of the project's own code runs.".into());
    }
    if let Some(d) = f.last_build {
        details.push(format!(
            "The last build took {}.",
            progress::format_elapsed(d)
        ));
    }
    match &f.remote {
        Some(r) if !r.approved => details.push(
            "Chorong asks for your approval of the recipe first; it covers prepare and indexing."
                .into(),
        ),
        None if !f.trusted => {
            details.push("Chorong asks whether you trust this folder first.".into());
        }
        _ => {}
    }
    let mut warnings = Vec::new();
    for it in &f.items {
        if let Some(at) = it.no_node_modules.as_ref().filter(|_| f.remote.is_none()) {
            warnings.push(format!(
                "No node_modules in {at}: imports between packages won't resolve until you install the dependencies (Chorong never installs)."
            ));
        } else if let Some(b) = &it.blocked {
            warnings.push(format!("{}: {b}", it.label));
        }
    }
    let not_now = ("Not now".to_string(), StartAction::Dismiss);
    let (text, mut actions) = match (kind, &f.remote) {
        (BannerKind::Missing, Some(r)) if r.commands.is_empty() => (
            "No index yet. Files read without one; an index adds definitions, references and call kinds.",
            vec![("Index".to_string(), StartAction::IndexOnly)],
        ),
        (BannerKind::Missing, Some(_)) => (
            "No index yet. Files read without one; an index adds definitions, references and call kinds.",
            vec![
                (
                    "Prepare and index".to_string(),
                    StartAction::PrepareAndIndex,
                ),
                ("Index only".to_string(), StartAction::IndexOnly),
            ],
        ),
        (BannerKind::Missing, None) => (
            "No index yet. Files read without one; an index adds definitions, references and call kinds.",
            vec![("Build index".to_string(), StartAction::Build)],
        ),
        (BannerKind::Stale, _) => {
            let mut a = vec![("Refresh".to_string(), StartAction::Refresh)];
            if !i.lightweight {
                a.push((
                    "Keep fresh this session".to_string(),
                    StartAction::KeepFresh,
                ));
            }
            ("Index is out of date. It keeps answering, marked stale.", a)
        }
    };
    actions.push(not_now);
    actions.push(("Don't show again".to_string(), StartAction::Hide));
    Some(StartBanner {
        kind,
        text: text.to_string(),
        details,
        warnings,
        actions,
    })
}

/// Reads the banner's facts for `root` (blocks: a folder walk, git, the cache).
fn read_start_facts(root: &std::path::Path, trusted: bool) -> StartFacts {
    let items = indexing::plan(root)
        .into_iter()
        .map(|it| {
            // What the build runs and whether project code runs: the indexer crate's own plan.
            let plan = cho_semantic::indexer::build_plan(&it.project);
            let (runs, runs_code) = (
                plan.runs,
                plan.runs_project_code.then(|| plan.why.to_string()),
            );
            let no_node_modules = match cho_semantic::node_modules::check(&it.project, root) {
                cho_semantic::node_modules::NodeModules::Missing { at } => Some(at),
                _ => None,
            };
            ItemFacts {
                label: it.label,
                runs,
                runs_code,
                blocked: it.blocked,
                no_node_modules,
            }
        })
        .collect();
    let a = crate::remote::adapter::assess(root);
    let remote = match (&a.access, a.summary()) {
        (crate::remote::prepare::Access::Local, _) => None,
        (access, summary) => Some(RemoteFacts {
            commands: summary.map(|s| s.commands).unwrap_or_default(),
            approved: matches!(access, crate::remote::prepare::Access::Approved),
        }),
    };
    let last_build = IndexCache::default_location().and_then(|cache| {
        let ok: Vec<u64> = cache
            .projects(root)
            .iter()
            .filter(|p| p.meta.success())
            .map(|p| p.meta.duration_ms)
            .collect();
        (!ok.is_empty()).then(|| Duration::from_millis(ok.iter().sum()))
    });
    StartFacts {
        items,
        remote,
        last_build,
        trusted,
    }
}

impl Chorong {
    /// Works the banner's facts out again, off the UI thread (after each index load).
    pub(super) fn load_start_facts(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.as_ref().and_then(|r| r.canonicalize().ok()) else {
            return;
        };
        // The trust check reads the trust file and runs `git remote`: off the UI thread too.
        let trust = self.trust.clone();
        cx.spawn(async move |this, cx| {
            let (facts, upgrade) = cx
                .background_spawn(async move {
                    let (found, upgrade) = trust.check_read_only(&root);
                    let trusted = found == crate::trust::Trust::Trusted;
                    (read_start_facts(&root, trusted), upgrade)
                })
                .await;
            this.update(cx, |this, cx| {
                // The trust file is written only on the UI thread.
                if let Some(u) = upgrade {
                    this.trust.upgrade(u).ok();
                }
                this.start.facts = Some(facts);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A prepare is running, or its approval panel is waiting for the reader.
    pub(super) fn prepare_busy(&self) -> bool {
        self.prepare.as_ref().is_some_and(|p| {
            p.status.running() || p.status == crate::remote::prepare::Status::Asking
        })
    }

    /// The banner for this window right now, if any.
    pub(super) fn start_banner(&self) -> Option<StartBanner> {
        // `index.banner` off: the status bar's Index item, Cmd-K I and the palette still build.
        if !self.features.settings().index_banner {
            return None;
        }
        let kind = match &self.index {
            IndexState::Missing(IndexStatus::None) => Some(BannerKind::Missing),
            IndexState::Ready(_, IndexStatus::Stale) => Some(BannerKind::Stale),
            _ => None,
        };
        start_banner(StartInputs {
            facts: self.start.facts.as_ref()?,
            kind,
            // A dialog (the trust question, the approval panel, a link prompt) has the floor.
            busy: self.job.is_some() || self.prepare_busy() || self.overlay.is_some(),
            lightweight: self.features.store().effective().lightweight(),
            keep_fresh: self.keep_fresh_on(),
            dismissed: self.start.dismissed,
        })
    }

    /// Esc in the code while the banner is up (the `IndexBanner` key context): **Not now**.
    /// No key presses its build buttons (the owner's decision: indexing never starts by
    /// accident); a click, Cmd-K I or the status bar does.
    fn dismiss_start_banner_key(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.start_banner() {
            self.start_action(StartAction::Dismiss, b.kind, cx);
        }
    }

    pub(super) fn start_action(
        &mut self,
        action: StartAction,
        kind: BannerKind,
        cx: &mut Context<Self>,
    ) {
        match action {
            StartAction::Build | StartAction::Refresh => self.request_build(cx),
            StartAction::PrepareAndIndex => self.start_prepare_flow(false, cx),
            StartAction::IndexOnly => self.start_prepare_flow(true, cx),
            StartAction::KeepFresh => {
                self.start.keep_fresh = true;
                self.start.auto_tried = false;
                self.maybe_keep_fresh(cx);
            }
            StartAction::Dismiss => self.start.dismissed = Some(kind),
            StartAction::Hide => {
                // Hidden in this window at once; the setting reaches every window when saved.
                self.start.dismissed = Some(kind);
                cx.spawn(async move |this, cx| {
                    let saved = cx
                        .background_spawn(async {
                            cho_settings::SettingsStore::open_default()
                                .set(
                                    cho_settings::Scope::Global,
                                    "index.banner",
                                    cho_settings::Value::Bool(false),
                                )
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        })
                        .await;
                    this.update(cx, |this, cx| match saved {
                        Ok(()) => this.notify_text(
                            "Index banner hidden: turn index.banner back on in Settings",
                            cx,
                        ),
                        Err(e) => this.notify_text(format!("Could not save the setting: {e}"), cx),
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// The index banner: a card over the bottom right of the window, above the status bar, so
    /// it moves no pane (the files read at once, as without it). It says what an index build
    /// runs and has the buttons that start it; nothing starts by itself.
    pub(super) fn render_start_banner(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let b = self.start_banner()?;
        let t = &self.theme;
        let accent = t.players.first().map(|p| p.cursor).unwrap_or(t.text);
        let kind = b.kind;
        let mut row = div().flex().flex_wrap().gap_2().items_center();
        for (i, (label, action)) in b.actions.iter().enumerate() {
            let action = *action;
            let primary = i == 0;
            // The key that presses it, as the trust dialog shows them (`IndexBanner` context):
            // only Not now has one, so a build never starts from a key.
            let key = (action == StartAction::Dismiss).then_some("Esc");
            row = row.child(
                div()
                    .id(("start-banner-action", i))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(hsla(if primary { accent } else { t.border }))
                    .bg(hsla(if primary {
                        t.element_selected
                    } else {
                        t.editor_background
                    }))
                    .text_color(hsla(t.text))
                    .cursor_pointer()
                    .hover(|s| s.bg(hsla(t.element_selected)))
                    .child(label.clone())
                    .children(key.map(|k| self.button_key(k.to_string())))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_action(action, kind, cx);
                    })),
            );
        }
        let warning = {
            let c = crate::pane_style::colors(&self.theme, cx);
            c.warning.unwrap_or(c.text)
        };
        Some(
            div()
                .id("start-banner")
                .absolute()
                .right(px(12.))
                .bottom(px(32.))
                .w(px(460.))
                .px_3()
                .py(px(8.))
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .border_1()
                .border_l_4()
                .border_color(hsla(accent))
                .shadow_md()
                .bg(hsla(t.ui.elevated_surface_background))
                // Clicks on the card stay on it.
                .occlude()
                .text_size(px(12.5))
                .text_color(hsla(t.text))
                .child(div().child(b.text))
                .children(b.details.into_iter().map(|d| {
                    div()
                        .text_size(px(12.))
                        .text_color(hsla(t.text_muted))
                        .child(d)
                }))
                .children(
                    b.warnings
                        .into_iter()
                        .map(|w| div().text_size(px(12.)).text_color(warning).child(w)),
                )
                .child(row)
                .into_any_element(),
        )
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        BuildIndex,
        CancelIndexing,
        ShowIndexError,
        StartBannerDismiss,
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c("chorong::BuildIndex", "Build Index", Layer::View, 180),
    c(
        "chorong::CancelIndexing",
        "Cancel Indexing",
        Layer::View,
        185,
    ),
    c(
        "chorong::ShowIndexError",
        "Show Index Error",
        Layer::View,
        190,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, BuildIndex, |this, _w, cx| this.request_build(cx));
    let d = on!(d, cx, CancelIndexing, |this, _w, cx| this
        .cancel_indexing(cx));
    // Esc in the code while the banner is up (`IndexBanner`, see `render`).
    let d = on!(d, cx, StartBannerDismiss, |this, _w, cx| this
        .dismiss_start_banner_key(cx));
    on!(d, cx, ShowIndexError, |this, _w, cx| {
        if this.index_failure.is_some() {
            this.overlay = Some(Overlay::IndexError);
        } else {
            this.notify_text("No failed index build to show", cx);
        }
    })
}

/// The status bar's `Ln, Col` for byte offset `caret`, both 1-based. The column is a display
/// value in characters (Unicode scalar values), while Chorong's positions are UTF-8 byte offsets.
/// A stale caret (past the end, or inside a character, after a pane swap or history move) is
/// clamped to the text rather than panicking.
pub(super) fn caret_line_col(buffer: &cho_core::SourceBuffer, caret: usize) -> (usize, usize) {
    let lc = buffer.byte_to_line_col_in(caret, cho_core::ColumnUnit::Utf32);
    (lc.line + 1, lc.col + 1)
}

/// What a click on the status-bar index control does when no build is running. The label
/// ([`Chorong::index_label`]) and the click (`index_control`) both come from here, so they agree:
/// "show error" only when there is stored error text to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IndexClick {
    /// Opens the index error sheet.
    ShowError,
    /// Starts a build: no index, or a failure with no error text saved (an unreadable cache).
    Build,
    /// Starts a build over a stale index.
    Refresh,
    /// Says the index is current.
    Current,
    /// Starts a build, with no hint in the label (no folder yet, or a usable index with
    /// another status).
    QuietBuild,
    /// Nothing (loading).
    Nothing,
}

impl IndexClick {
    /// The word after the status in the label, if any.
    pub(super) fn hint(self) -> Option<&'static str> {
        match self {
            IndexClick::ShowError => Some("show error"),
            IndexClick::Build => Some("build"),
            IndexClick::Refresh => Some("refresh"),
            IndexClick::Current | IndexClick::QuietBuild | IndexClick::Nothing => None,
        }
    }
}

/// See [`IndexClick`]. `has_failure` is whether error text from a failed build is stored.
pub(super) fn index_click(index: &IndexState, has_failure: bool) -> IndexClick {
    match index {
        IndexState::Missing(IndexStatus::Failed) | IndexState::Failed(_) if has_failure => {
            IndexClick::ShowError
        }
        IndexState::Missing(_) | IndexState::Failed(_) => IndexClick::Build,
        IndexState::Ready(_, IndexStatus::Stale) => IndexClick::Refresh,
        IndexState::Ready(_, IndexStatus::Current) => IndexClick::Current,
        IndexState::Loading => IndexClick::Nothing,
        IndexState::Absent | IndexState::Ready(..) => IndexClick::QuietBuild,
    }
}

/// How the status bar draws the index state (docs/public/visual-design.md, "Status bar").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IndexLook {
    /// Solid dot in `success`.
    Current,
    /// Dot and text in `warning`.
    Stale,
    /// Dot and text in `error`.
    Failed,
    /// Hollow ring in `text.muted`.
    None,
    /// Dashed hollow ring (loading, or an index build running).
    Loading,
}

pub(crate) fn index_look(index: &IndexState, building: bool) -> IndexLook {
    if building {
        return IndexLook::Loading;
    }
    match index {
        IndexState::Loading => IndexLook::Loading,
        IndexState::Failed(_)
        | IndexState::Missing(IndexStatus::Failed)
        | IndexState::Ready(_, IndexStatus::Failed) => IndexLook::Failed,
        IndexState::Ready(_, IndexStatus::Current) => IndexLook::Current,
        IndexState::Ready(_, IndexStatus::Stale | IndexStatus::Partial)
        | IndexState::Missing(IndexStatus::Stale | IndexStatus::Partial) => IndexLook::Stale,
        IndexState::Absent
        | IndexState::Missing(IndexStatus::None | IndexStatus::Current)
        | IndexState::Ready(_, IndexStatus::None) => IndexLook::None,
    }
}

/// The 8 px index dot; it differs by shape as well as colour.
fn index_dot(look: IndexLook, c: &cho_layout_ui::colors::Colors) -> gpui::AnyElement {
    let d = div().size(px(8.)).flex_none().rounded(px(4.));
    match look {
        IndexLook::Current => d.bg(c.success.unwrap_or(c.text_muted)),
        IndexLook::Stale => d.bg(c.warning.unwrap_or(c.text_muted)),
        IndexLook::Failed => d.bg(c.error.unwrap_or(c.text_muted)),
        IndexLook::None => d.border_1().border_color(c.text_muted),
        IndexLook::Loading => d.border_1().border_dashed().border_color(c.text_muted),
    }
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, code: bool) -> ItemFacts {
        ItemFacts {
            label: label.into(),
            runs: "rust-analyzer scip".into(),
            runs_code: code.then(|| "build scripts and procedural macros".to_string()),
            blocked: None,
            no_node_modules: None,
        }
    }

    fn inputs(facts: &StartFacts, kind: Option<BannerKind>) -> StartInputs<'_> {
        StartInputs {
            facts,
            kind,
            busy: false,
            lightweight: false,
            keep_fresh: false,
            dismissed: None,
        }
    }

    fn actions(b: &StartBanner) -> Vec<StartAction> {
        b.actions.iter().map(|(_, a)| *a).collect()
    }

    #[test]
    fn show_error_is_offered_only_when_there_is_error_text_to_show() {
        // An unreadable cache (no stored error text) used to read "show error" while a click
        // started a build. The label and the click now come from the same decision.
        let unreadable = IndexState::Failed("index.scip: bad".into());
        assert_eq!(index_click(&unreadable, false), IndexClick::Build);
        assert_eq!(index_click(&unreadable, false).hint(), Some("build"));
        let failed = IndexState::Missing(IndexStatus::Failed);
        assert_eq!(index_click(&failed, false).hint(), Some("build"));
        // With error text: "show error", and the click opens it.
        assert_eq!(index_click(&unreadable, true), IndexClick::ShowError);
        assert_eq!(index_click(&failed, true).hint(), Some("show error"));
        // The other states are unchanged.
        let none = IndexState::Missing(IndexStatus::None);
        assert_eq!(index_click(&none, true).hint(), Some("build"));
        assert_eq!(
            index_click(&IndexState::Loading, false),
            IndexClick::Nothing
        );
        assert_eq!(
            index_click(&IndexState::Absent, false),
            IndexClick::QuietBuild
        );
        assert_eq!(index_click(&IndexState::Absent, false).hint(), None);
    }

    #[test]
    fn the_caret_column_counts_characters_and_survives_a_stale_caret() {
        let b = cho_core::SourceBuffer::new("ab\nh\u{e9}llo");
        assert_eq!(caret_line_col(&b, 0), (1, 1));
        // After `h\u{e9}` (bytes 3..6): the third character of line 2.
        assert_eq!(caret_line_col(&b, 6), (2, 3));
        // Inside the two-byte `\u{e9}` (byte 5): rounds down, no panic.
        assert_eq!(caret_line_col(&b, 5), (2, 2));
        // Past the end (a caret from a longer document): the end, no panic.
        assert_eq!(caret_line_col(&b, 100), (2, 6));
    }

    #[test]
    fn a_local_folder_with_no_index_offers_the_build_and_says_what_runs() {
        let facts = StartFacts {
            items: vec![item("rust", true)],
            last_build: Some(Duration::from_secs(130)),
            trusted: true,
            ..Default::default()
        };
        let b = start_banner(inputs(&facts, Some(BannerKind::Missing))).unwrap();
        assert_eq!(
            actions(&b),
            [StartAction::Build, StartAction::Dismiss, StartAction::Hide]
        );
        let d = b.details.join("\n");
        assert!(d.contains("rust-analyzer scip (rust)"), "{d}");
        assert!(
            d.contains("The project's own code runs: build scripts"),
            "{d}"
        );
        assert!(d.contains("2m 10s"), "{d}");
        assert!(!d.contains("trust"), "trusted: no question: {d}");
        // Untrusted: the same button, and the banner says the click asks first.
        let untrusted = StartFacts {
            trusted: false,
            ..facts.clone()
        };
        let b = start_banner(inputs(&untrusted, Some(BannerKind::Missing))).unwrap();
        assert!(b.details.join(" ").contains("trust this folder"));
        // Nothing to index, a build running, or dismissed: no banner.
        let none = StartFacts::default();
        assert!(start_banner(inputs(&none, Some(BannerKind::Missing))).is_none());
        let mut busy = inputs(&facts, Some(BannerKind::Missing));
        busy.busy = true;
        assert!(start_banner(busy).is_none());
        let mut gone = inputs(&facts, Some(BannerKind::Missing));
        gone.dismissed = Some(BannerKind::Missing);
        assert!(start_banner(gone).is_none());
        assert!(start_banner(inputs(&facts, None)).is_none());
    }

    #[test]
    fn a_remote_project_has_one_banner_with_two_buttons_and_the_recipe() {
        let facts = StartFacts {
            items: vec![item("typescript", false)],
            remote: Some(RemoteFacts {
                commands: vec!["pnpm install --frozen-lockfile".into()],
                approved: false,
            }),
            ..Default::default()
        };
        let b = start_banner(inputs(&facts, Some(BannerKind::Missing))).unwrap();
        assert_eq!(
            actions(&b),
            [
                StartAction::PrepareAndIndex,
                StartAction::IndexOnly,
                StartAction::Dismiss,
                StartAction::Hide
            ]
        );
        assert_eq!(b.actions[0].0, "Prepare and index");
        let d = b.details.join("\n");
        assert!(
            d.contains("Prepare runs: pnpm install --frozen-lockfile"),
            "{d}"
        );
        // The recipe runs package scripts: project code runs even if the indexer doesn't.
        assert!(d.contains("recipe runs the project's own code"), "{d}");
        assert!(!d.contains("None of"), "{d}");
        assert!(d.contains("approval"), "{d}");
        // No recipe: one button.
        let bare = StartFacts {
            remote: Some(RemoteFacts {
                commands: vec![],
                approved: true,
            }),
            ..facts
        };
        let b = start_banner(inputs(&bare, Some(BannerKind::Missing))).unwrap();
        assert_eq!(
            actions(&b),
            [
                StartAction::IndexOnly,
                StartAction::Dismiss,
                StartAction::Hide
            ]
        );
        assert!(
            b.details
                .join(" ")
                .contains("None of the project's own code runs")
        );
    }

    #[test]
    fn a_stale_index_offers_refresh_and_keep_fresh_except_in_lightweight() {
        let facts = StartFacts {
            items: vec![item("rust", true)],
            trusted: true,
            ..Default::default()
        };
        let b = start_banner(inputs(&facts, Some(BannerKind::Stale))).unwrap();
        assert!(b.text.starts_with("Index is out of date"));
        assert_eq!(
            actions(&b),
            [
                StartAction::Refresh,
                StartAction::KeepFresh,
                StartAction::Dismiss,
                StartAction::Hide
            ]
        );
        assert_eq!(b.actions[1].0, "Keep fresh this session");
        let mut light = inputs(&facts, Some(BannerKind::Stale));
        light.lightweight = true;
        let b = start_banner(light).unwrap();
        assert_eq!(
            actions(&b),
            [
                StartAction::Refresh,
                StartAction::Dismiss,
                StartAction::Hide
            ]
        );
        // Kept fresh: it refreshes by itself, so no banner.
        let mut fresh = inputs(&facts, Some(BannerKind::Stale));
        fresh.keep_fresh = true;
        assert!(start_banner(fresh).is_none());
        // Dismissing "missing" doesn't hide "stale".
        let mut other = inputs(&facts, Some(BannerKind::Stale));
        other.dismissed = Some(BannerKind::Missing);
        assert!(start_banner(other).is_some());
    }

    #[test]
    fn a_local_typescript_project_without_node_modules_is_warned_about() {
        let mut ts = item("typescript", true);
        ts.no_node_modules = Some("web".into());
        let facts = StartFacts {
            items: vec![ts],
            trusted: true,
            ..Default::default()
        };
        let b = start_banner(inputs(&facts, Some(BannerKind::Missing))).unwrap();
        assert_eq!(b.warnings.len(), 1);
        assert!(
            b.warnings[0].contains("No node_modules in web")
                && b.warnings[0].contains("imports between packages won't resolve"),
            "{:?}",
            b.warnings
        );
    }

    #[test]
    fn index_look_follows_the_state() {
        assert_eq!(index_look(&IndexState::Absent, false), IndexLook::None);
        assert_eq!(index_look(&IndexState::Loading, false), IndexLook::Loading);
        assert_eq!(
            index_look(&IndexState::Missing(IndexStatus::None), false),
            IndexLook::None
        );
        assert_eq!(
            index_look(&IndexState::Missing(IndexStatus::Failed), false),
            IndexLook::Failed
        );
        assert_eq!(
            index_look(&IndexState::Failed("x".into()), false),
            IndexLook::Failed
        );
        assert_eq!(
            index_look(&IndexState::Missing(IndexStatus::None), true),
            IndexLook::Loading
        );
    }
}

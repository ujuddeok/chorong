//! Command handlers: what each named action does. Key bindings never reach this code
//! directly; GPUI turns them into actions (see `keymap.rs`).

use super::*;
use crate::marks::step_occurrence;
use crate::trust::Trust;
use cho_semantic::indexer;
use cho_semantic::progress::Progress;

impl Chorong {
    // ---- Notices ----

    /// Shows `text` in the status bar for a few seconds.
    pub(super) fn notify_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let generation = self.notices.next();
        self.notice = Some(Notice {
            text: text.into(),
            generation,
        });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_TIME).await;
            this.update(cx, |this, cx| {
                if this
                    .notice
                    .as_ref()
                    .is_some_and(|n| n.generation.is(generation))
                {
                    this.notice = None;
                    this.redraw_chrome(cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A bound command whose feature does not exist yet. The key is real; the feature is not.
    pub(super) fn unavailable(&mut self, what: &str, cx: &mut Context<Self>) {
        self.notify_text(format!("{what}: not available yet"), cx);
    }

    // ---- Binding labels ----

    /// The key shown for `action`: its binding as it applies in the code view, so a key
    /// bound only there is still listed while a picker is open.
    pub(super) fn binding_label(window: &Window, action: &dyn Action) -> Option<String> {
        let ctx = KeyContext::parse("Chorong CodeView").ok()?;
        window
            .highest_precedence_binding_for_action_in_context(action, ctx)
            .map(|b| keymap::label(&b, Platform::current()))
    }

    // ---- Palette and symbol pickers ----

    pub(super) fn open_palette(&mut self) {
        let mut labels = Vec::new();
        let mut hints = Vec::new();
        let mut picks = Vec::new();
        let host = self.host_button();
        for (name, title, action) in &self.command_actions {
            // In a view of Chorong's cache "Open in editor" becomes "Open on GitHub / Bitbucket".
            let title = match (host, *name) {
                (Some(_), "chorong::OpenOnGitHub") => continue,
                (Some(h), "chorong::OpenInEditor") => format!("Open on {h}"),
                _ => title.to_string(),
            };
            labels.push(title);
            hints.push(self.key_labels.get(name).cloned().unwrap_or_default());
            picks.push(Pick::Command(action.boxed_clone()));
        }
        self.open_overlay_picker(PickerKind::Palette, labels, hints, picks);
    }

    pub(super) fn open_symbols(&mut self) {
        let Some(doc) = &self.ed().doc else { return };
        let mut labels = Vec::new();
        let mut hints = Vec::new();
        let mut picks = Vec::new();
        for e in &doc.outline {
            labels.push(format!("{}{}", "  ".repeat(e.depth), e.name));
            hints.push(e.kind.as_str().to_string());
            picks.push(Pick::Symbol(e.byte));
        }
        self.open_overlay_picker(PickerKind::Symbols, labels, hints, picks);
    }

    pub(super) fn open_overlay_picker(
        &mut self,
        kind: PickerKind,
        labels: Vec<String>,
        hints: Vec<String>,
        picks: Vec<Pick>,
    ) {
        self.search_blur();
        let matches = self.ranker.rank(&labels, "", MAX_RESULTS);
        self.picker = Some(PickerState {
            kind,
            query: String::new(),
            query_all: false,
            add_to_page: false,
            tour_scope: false,
            peek_open: false,
            matches,
            selected: 0,
            labels,
            hints,
            picks,
        });
        self.picker_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn search_blur(&mut self) {
        self.key_focus.leave(KeyFocus::Search);
    }

    // ---- Reading loop ----

    pub(super) fn highlight_symbol(&mut self, cx: &mut Context<Self>) {
        match self.ed().caret {
            Some(caret) => self.toggle_mark(caret, cx),
            None => self.notify_text("Click or move to a symbol first", cx),
        }
    }

    pub(super) fn step_highlight(&mut self, forward: bool, cx: &mut Context<Self>) {
        let caret = self.ed().caret.unwrap_or(0);
        let occ = self.ed().marks.occurrences_at(caret);
        match step_occurrence(occ, caret, forward) {
            Some(i) => {
                let byte = occ[i].start;
                self.set_caret(byte, true, cx);
            }
            None => self.notify_text(
                "No highlight yet: put the cursor on a symbol and highlight it",
                cx,
            ),
        }
    }

    /// Esc: remove the newest highlight group; again, soon after, to remove them all.
    pub(super) fn clear_highlights(&mut self) {
        if !self.ed().marks.has_own() {
            self.last_esc = None;
            return;
        }
        if self.last_esc.is_some_and(|t| t.elapsed() < ESC_AGAIN) {
            self.ed_mut().marks.clear_own();
            self.last_esc = None;
        } else {
            self.ed_mut().marks.pop_newest();
            self.last_esc = self.ed().marks.has_own().then(Instant::now);
        }
    }

    /// `[` and `]`: the previous or next declaration name from the caret.
    pub(super) fn step_declaration(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(doc) = &self.ed().doc else { return };
        let caret = self.ed().caret.unwrap_or(0);
        let starts = doc.symbols.iter().map(|s| s.name_range.start);
        let target = if forward {
            starts.filter(|s| *s > caret).min()
        } else {
            starts.filter(|s| *s < caret).max()
        };
        match target {
            Some(byte) => self.set_caret(byte, true, cx),
            None => self.notify_text(
                if forward {
                    "No later declaration"
                } else {
                    "No earlier declaration"
                },
                cx,
            ),
        }
    }

    pub(super) fn fold_all(&mut self, collapse: bool, cx: &mut Context<Self>) {
        let ed = self.ed_mut();
        if ed.late_folds != super::editor_state::LateFolds::None {
            // A large JSON file's folds are still being found: this runs when they arrive.
            ed.fold_all_later = Some(collapse);
            self.notify_text("Finding this file's folds\u{2026}", cx);
            return;
        }
        let Some(folds) = &mut ed.folds else { return };
        folds.set_all(collapse);
        if let Some(caret) = ed.caret
            && let Some(doc) = &ed.doc
        {
            let line = doc.buffer.line_of(caret);
            let visible = folds.visible_line(line);
            if visible != line {
                ed.caret = doc.buffer.line_range(visible).map(|r| r.start);
                self.refresh_context(cx);
            }
        }
    }

    // ---- Semantics ----

    fn symbol_query(
        &mut self,
        byte: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Option<(Arc<Semantic>, String, usize)> {
        let (Some(doc), Some(byte)) = (&self.ed().doc, byte.or(self.ed().caret)) else {
            self.notify_text("Move to a symbol first", cx);
            return None;
        };
        let Some(sem) = self.doc_index() else {
            let text = match &doc.at_rev {
                Some(rev) => format!("No index for {rev}: this view is syntax-only"),
                None => "This needs an index: use Build Index (status bar or command palette)"
                    .to_string(),
            };
            self.notify_text(text, cx);
            return None;
        };
        Some((sem.clone(), doc.display_path.clone(), byte))
    }

    /// Goes to the definition of the symbol at `byte` (the caret when `None`), opened at
    /// `place`: a new pane is added only once a definition is found.
    pub(super) fn go_to_definition(
        &mut self,
        byte: Option<usize>,
        place: super::shell_host::OpenAt,
        cx: &mut Context<Self>,
    ) {
        let Some((sem, path, byte)) = self.symbol_query(byte, cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let at = sem.symbol_at(&path, byte)?;
                    Some(sem.definitions(&at.id))
                })
                .await;
            this.update(cx, |this, cx| {
                match found {
                    None => this.notify_text("The index has no symbol here", cx),
                    Some(defs) => match defs.first() {
                        Some(d) => {
                            this.prepare_place(place, cx);
                            this.navigate(&d.path, d.range.start, cx)
                        }
                        None => this.notify_text("Defined outside the indexed project", cx),
                    },
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// References are listed in the context pane, which already follows the caret; this
    /// reports the count and moves the keyboard into that list (Up/Down, Enter, Esc there).
    pub(super) fn find_references(&mut self, cx: &mut Context<Self>) {
        let Some((sem, path, byte)) = self.symbol_query(None, cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let count = cx
                .background_spawn(async move {
                    let at = sem.symbol_at(&path, byte)?;
                    Some(
                        sem.references(&at.id)
                            .iter()
                            .filter(|l| !l.is_definition)
                            .count(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                match count {
                    None => this.notify_text("The index has no symbol here", cx),
                    Some(n) => {
                        this.focus_references();
                        this.notify_text(
                            format!(
                                "{n} reference{}: the keyboard is in the context pane's list",
                                if n == 1 { "" } else { "s" }
                            ),
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- Links ----

    /// Copies a `chorong://` link to the declaration at the caret (or its line when the
    /// declaration's name is not unique in the file). The repository is named by its remote;
    /// `share` drops the branch and dirty state.
    pub(super) fn copy_link(&mut self, share: bool, cx: &mut Context<Self>) {
        let (Some(root), Some(doc)) = (self.root.clone(), &self.ed().doc) else {
            self.notify_text("Open a file first", cx);
            return;
        };
        let caret = self.ed().caret.unwrap_or(0);
        let symbol = enclosing(&doc.symbols, caret)
            .zip(doc.language)
            .map(|(sym, lang)| sym.qualified_name(lang))
            .filter(|q| {
                doc.language.is_some_and(|l| {
                    matches!(find_symbol(l, doc.buffer.text(), q), SymbolMatch::Unique(_))
                })
            });
        let line = doc.buffer.line_of(caret) + 1;
        let path = doc.path.clone();
        // A pull-request view or comparison adds `pr`, `base` and `side` (provisional).
        let view = self.link_view_params();
        cx.spawn(async move |this, cx| {
            let url = cx
                .background_spawn(async move {
                    links::link_to(
                        &root,
                        &path,
                        symbol.as_deref(),
                        Some(line),
                        share,
                        view.as_ref(),
                    )
                })
                .await;
            this.update(cx, |this, cx| match url {
                Ok(url) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
                    let what = if share {
                        "Copied for sharing"
                    } else {
                        "Copied"
                    };
                    this.notify_text(format!("{what}: {url}"), cx);
                }
                Err(e) => this.notify_text(e, cx),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn banner_action(
        &mut self,
        action: BannerAction,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            BannerAction::OpenUrl(url) => cx.open_url(&url),
            BannerAction::Copy(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                self.notify_text(format!("Copied: {text}"), cx);
            }
            BannerAction::ViewAtRev => self.view_at_rev(cx),
            BannerAction::BackToCheckout => self.back_to_checkout(cx),
            BannerAction::OpenWorktree(w) => self.open_worktree(w, cx),
            BannerAction::RebuildGraph => self.rebuild_graph_from_link(cx),
            BannerAction::Dismiss => self.ed_mut().banner = None,
        }
    }

    // ---- Indexing ----

    /// Build Index: asks once per repository, then builds in the background.
    pub(super) fn request_build(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.notify_text("Open a folder to build an index", cx);
            return;
        };
        if self.job.is_some() {
            // Cmd-K I again while a build runs is the same as clicking its status-bar item.
            self.index_control(cx);
            return;
        }
        let Ok(canonical) = root.canonicalize() else {
            self.notify_text("Cannot read the folder", cx);
            return;
        };
        // The project walk and the trust check (the trust file, `git remote`) run off the UI
        // thread; only the newest request acts when its answer lands.
        let generation = self.build_checks.next();
        let (trust, asked_for) = (self.trust.clone(), self.root.clone());
        cx.spawn(async move |this, cx| {
            let (decision, upgrade) = cx
                .background_spawn(async move { build_decision(&canonical, &trust) })
                .await;
            this.update(cx, |this, cx| {
                // The trust file is written only on the UI thread.
                if let Some(u) = upgrade {
                    this.trust.upgrade(u).ok();
                }
                if !build_check_lands(this.build_checks, generation, this.job.is_some())
                    || this.root != asked_for
                {
                    return;
                }
                match decision {
                    BuildDecision::NoProjects => {
                        this.notify_text("No Rust, TypeScript or Python project found to index", cx)
                    }
                    BuildDecision::Build => this.start_build(false, cx),
                    // A remote project in Chorong's cache: its approval panel, then indexing only.
                    BuildDecision::Approve => this.start_prepare_flow(true, cx),
                    BuildDecision::Ask(notes) => {
                        this.overlay = Some(Overlay::Trust { notes });
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn trust_accept(&mut self, cx: &mut Context<Self>) {
        self.overlay = None;
        let Some(canonical) = self.root.as_ref().and_then(|r| r.canonicalize().ok()) else {
            return;
        };
        if let Err(e) = self.trust.trust(&canonical) {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                // A cache worktree: trust comes only from the project approval.
                self.notify_text(e.to_string(), cx);
                return;
            }
            self.notify_text(format!("Could not remember the choice: {e}"), cx);
        }
        self.start_build(false, cx);
    }

    /// "Keep fresh": a stale index refreshes without asking while the reader turned it on
    /// for this window (the banner) or for every session (`index.auto_refresh`). Never a
    /// first build (design/manual-start.md: every build is started by the reader), never in
    /// Lightweight mode, only in a trusted folder or an approved cache worktree, and tried once
    /// per click or session: a failed refresh is not retried automatically.
    pub(super) fn maybe_keep_fresh(&mut self, cx: &mut Context<Self>) {
        if !self.keep_fresh_due() {
            return;
        }
        let Some(canonical) = self.root.as_ref().and_then(|r| r.canonicalize().ok()) else {
            return;
        };
        // The trust check reads the trust file and runs `git remote`: off the UI thread. The
        // conditions are checked again when it lands.
        let (trust, asked_for) = (self.trust.clone(), self.root.clone());
        cx.spawn(async move |this, cx| {
            let (found, upgrade) = cx
                .background_spawn(async move { trust.check_read_only(&canonical) })
                .await;
            this.update(cx, |this, cx| {
                // The trust file is written only on the UI thread.
                if let Some(u) = upgrade {
                    this.trust.upgrade(u).ok();
                }
                if found == Trust::Trusted && this.root == asked_for && this.keep_fresh_due() {
                    this.start.auto_tried = true;
                    this.start_build(true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// A "Keep fresh" refresh would start now, trust aside.
    fn keep_fresh_due(&self) -> bool {
        !self.start.auto_tried
            && self.job.is_none()
            && self.keep_fresh_on()
            && matches!(self.index, IndexState::Ready(_, IndexStatus::Stale))
            && !self.prepare_busy()
    }

    /// Whether a stale index refreshes by itself in this window.
    pub(super) fn keep_fresh_on(&self) -> bool {
        !self.features.store().effective().lightweight()
            && (self.start.keep_fresh || self.features.on(Feature::AutoRefresh))
    }

    pub(super) fn start_build(&mut self, automatic: bool, cx: &mut Context<Self>) {
        let (Some(root), None) = (
            self.root.as_ref().and_then(|r| r.canonicalize().ok()),
            &self.job,
        ) else {
            return;
        };
        let Some(cache) = IndexCache::default_location() else {
            self.notify_text(
                "No cache directory for the index (set HOME or CHORONG_CACHE_DIR)",
                cx,
            );
            return;
        };
        // The plan walks the folder and reads the cache: off the UI thread. Only the newest
        // build request acts when it lands, and only while no build runs.
        let generation = self.build_checks.next();
        let asked_for = self.root.clone();
        cx.spawn(async move |this, cx| {
            let (cache, root, todo, blocked) = cx
                .background_spawn(async move {
                    let (blocked, runnable): (Vec<_>, Vec<_>) = indexing::plan(&root)
                        .into_iter()
                        .partition(|i| i.blocked.is_some());
                    let todo = indexing::needs_build(&cache, &root, runnable);
                    let blocked: Vec<String> = blocked
                        .iter()
                        .map(|i| {
                            format!("{}: {}", i.label, i.blocked.as_deref().unwrap_or_default())
                        })
                        .collect();
                    (cache, root, todo, blocked)
                })
                .await;
            this.update(cx, |this, cx| {
                if build_check_lands(this.build_checks, generation, this.job.is_some())
                    && this.root == asked_for
                {
                    this.run_build(automatic, cache, root, todo, blocked, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Starts building `todo` (the planned projects that need it); `blocked` are the reasons
    /// the other projects cannot be built.
    fn run_build(
        &mut self,
        automatic: bool,
        cache: IndexCache,
        root: PathBuf,
        todo: Vec<indexing::Item>,
        blocked: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if todo.is_empty() {
            if !automatic {
                let msg = blocked
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "The index is current".into());
                self.notify_text(msg, cx);
            }
            return;
        }
        // Only the automatic refreshes ("Keep fresh") are the feature's cost; a build the
        // reader asked for is theirs.
        let cost = automatic.then(|| self.features.probe(Feature::AutoRefresh));
        let live = Arc::new(std::sync::Mutex::new(Progress::default()));
        let cancel = indexer::CancelToken::new();
        let opts = indexer::Options {
            timeout: indexing::TIMEOUT,
            cancel: cancel.clone(),
            node_memory_mb: self.features.settings().index_node_memory_mb,
            // A failed prepare in this window: the index is built anyway and reads partial.
            prepare: self.prepare_note.clone(),
            tools: cho_sources::tools::config(self.features.settings()),
            low_priority: self.features.settings().index_low_priority,
        };
        if !automatic {
            // A build the reader started: an automatic refresh may be tried again afterwards.
            self.start.auto_tried = false;
        }
        self.index_cancelled = false;
        self.job = Some(Job {
            total: todo.len(),
            done: 0,
            current: todo[0].label.clone(),
            automatic,
            cancel,
            cancel_state: Cancel::No,
            started: Instant::now(),
            progress: Progress::default(),
            live: live.clone(),
            large: todo.iter().any(|i| i.large),
        });
        cx.notify();
        // Polls what the build thread has seen every two seconds (elapsed time and the step): often
        // enough to read, rarely enough to cost nothing.
        let poll = live.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let running = this
                    .update(cx, |this, cx| {
                        let Some(j) = this.job.as_mut().filter(|j| Arc::ptr_eq(&j.live, &poll))
                        else {
                            return false;
                        };
                        let now = poll.lock().map(|p| p.clone()).unwrap_or_default();
                        j.progress = now;
                        if matches!(j.cancel_state, Cancel::Confirming(t) if t <= Instant::now()) {
                            j.cancel_state = Cancel::No;
                        }
                        // The elapsed time moves: it shows in the status bar and on the Context
                        // pane's build button, nowhere else (pins are redrawn with Context).
                        this.redraw_chrome(cx);
                        this.refresh_context_panes(cx);
                        true
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let mut report = indexing::Report::default();
            let mut cancelled = false;
            report.problems.extend(blocked);
            for item in todo {
                if cancelled {
                    break;
                }
                let label = item.label.clone();
                if let Some(n) = &item.note {
                    report.notes.push(n.clone());
                }
                this.update(cx, |this, cx| {
                    if let Some(j) = &mut this.job {
                        j.current = label.clone();
                    }
                    cx.notify();
                })
                .ok();
                let (cache, root) = (cache.clone(), root.clone());
                let cost = cost.clone();
                let (opts, live) = (opts.clone(), live.clone());
                let result = cx
                    .background_spawn(async move {
                        let _t = cost.as_ref().map(|c| c.background());
                        indexer::refresh_with(&cache, &root, &item.project, &opts, &mut |t| {
                            if let Ok(mut p) = live.lock() {
                                *p = t.progress.clone();
                            }
                        })
                    })
                    .await;
                match result {
                    Ok(indexer::Refresh::Cancelled) => cancelled = true,
                    Ok(indexer::Refresh::Done(meta)) if meta.success() => report.built += 1,
                    Ok(indexer::Refresh::Done(meta)) => report.problems.push(format!(
                        "{label} failed: {}",
                        indexer::failure_reason(&meta.stderr_tail)
                            .or_else(|| {
                                meta.stderr_tail
                                    .lines()
                                    .rev()
                                    .find(|l| !l.trim().is_empty())
                            })
                            .unwrap_or("see the error details")
                    )),
                    Err(e) => report.problems.push(format!("{label}: {e}")),
                }
                this.update(cx, |this, cx| {
                    if let Some(j) = &mut this.job {
                        j.done += 1;
                    }
                    cx.notify();
                })
                .ok();
            }
            this.update(cx, |this, cx| {
                this.job = None;
                this.index_cancelled = cancelled;
                let text = match (report.built, report.problems.first()) {
                    _ if cancelled => {
                        "Index build cancelled; the previous index is kept".to_string()
                    }
                    (n, None) => format!(
                        "Index {} ({n} project{}){}",
                        if automatic {
                            "refreshed in the background"
                        } else {
                            "built"
                        },
                        if n == 1 { "" } else { "s" },
                        report
                            .notes
                            .first()
                            .map(|n| format!("; {n}"))
                            .unwrap_or_default()
                    ),
                    (_, Some(p)) => format!("Index: {p}"),
                };
                this.notify_text(text, cx);
                // The finished index replaces the old one in place; no reopening.
                this.load_index_in_background(cx);
                // A prepare result arrived while this build ran: build again with it.
                if std::mem::take(&mut this.rebuild_queued) && !cancelled {
                    this.start_build(false, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Clicking the status-bar index control.
    pub(super) fn index_control(&mut self, cx: &mut Context<Self>) {
        if self.job.is_some() {
            return self.ask_cancel_indexing(cx);
        }
        // The same decision as the label (`index_status::index_click`), so they agree.
        match index_status::index_click(&self.index, self.index_failure.is_some()) {
            index_status::IndexClick::ShowError => {
                self.overlay = Some(Overlay::IndexError);
                cx.notify();
            }
            index_status::IndexClick::Current => self.notify_text("The index is current", cx),
            index_status::IndexClick::Nothing => {}
            index_status::IndexClick::Build
            | index_status::IndexClick::Refresh
            | index_status::IndexClick::QuietBuild => self.request_build(cx),
        }
    }

    /// The first click while a build runs asks "Cancel indexing?" in the status bar; a second
    /// one within a few seconds cancels. (The command palette's Cancel Indexing needs no second
    /// click.)
    fn ask_cancel_indexing(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        match job.cancel_state {
            Cancel::Cancelling => {}
            Cancel::Confirming(until) if until > Instant::now() => self.cancel_indexing(cx),
            Cancel::No | Cancel::Confirming(_) => {
                job.cancel_state = Cancel::Confirming(Instant::now() + CANCEL_CONFIRM);
                cx.notify();
            }
        }
    }

    /// Stops the running build: the indexer's process group is killed and the previous index is
    /// kept (the cache is untouched). Safe to call when nothing is running.
    pub(super) fn cancel_indexing(&mut self, cx: &mut Context<Self>) {
        match self.job.as_mut() {
            Some(job) if job.cancel_state != Cancel::Cancelling => {
                job.cancel_state = Cancel::Cancelling;
                job.cancel.cancel();
                cx.notify();
            }
            Some(_) => {}
            None => self.notify_text("No index build is running", cx),
        }
    }

    // ---- Chord popup ----

    /// GPUI reported a change in the chord in progress. When it drops a chord without a key
    /// having been pressed (its fixed one-second timeout), the chord is held open instead,
    /// popup and all, until the next key.
    pub(super) fn pending_input_changed(&mut self, window: &Window) {
        match window.pending_input_keystrokes().filter(|p| !p.is_empty()) {
            Some(p) => self.chord = Chord::Pending(p.to_vec(), Instant::now()),
            None => {
                // A key that answers a chord clears the pending chord (keystroke observer)
                // before this runs, so a chord still pending here ran out of time.
                match std::mem::take(&mut self.chord) {
                    Chord::Pending(keys, at) if at.elapsed() >= CHORD_HOLD_AFTER => {
                        self.chord = Chord::Held(keys)
                    }
                    // Dropped by a key, or a held chord stays held.
                    Chord::Held(keys) => self.chord = Chord::Held(keys),
                    Chord::Pending(..) | Chord::Idle => {}
                }
            }
        }
    }

    /// When GPUI times a chord out it replays the chord's own keys (to the key-down
    /// listeners and the keystroke observers) after the chord was held; those are not the
    /// key that finishes it.
    pub(super) fn is_chord_replay(&self, k: &Keystroke) -> bool {
        match &self.chord {
            Chord::Held(keys) => keys
                .iter()
                .any(|h| h.key == k.key && h.modifiers == k.modifiers),
            Chord::Idle | Chord::Pending(..) => false,
        }
    }

    /// The action bound to the whole key sequence `keys` in the code view, from the live
    /// keymap (the latest binding wins, as in GPUI).
    pub(super) fn chord_action(keys: &[Keystroke], cx: &App) -> Option<Box<dyn Action>> {
        let context = [KeyContext::parse("Chorong CodeView").ok()?];
        let bindings = cx.key_bindings();
        let bindings = bindings.borrow();
        bindings
            .bindings()
            .rev()
            .find(|b| {
                b.match_keystrokes(keys) == Some(false)
                    && b.predicate().is_none_or(|p| p.depth_of(&context).is_some())
            })
            .map(|b| b.action().boxed_clone())
    }

    /// The keys that can follow the chord `pending`, with what each does.
    pub(super) fn chord_followers(pending: &[Keystroke], cx: &App) -> Vec<(String, String)> {
        if pending.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(String, String)> = Vec::new();
        for b in cx.key_bindings().borrow().bindings() {
            let strokes = b.keystrokes();
            if strokes.len() != pending.len() + 1 {
                continue;
            }
            if !pending
                .iter()
                .zip(strokes)
                .all(|(p, s)| p.key == s.inner().key && p.modifiers == s.inner().modifiers)
            {
                continue;
            }
            let key = keymap::stroke_label(strokes[pending.len()].inner(), Platform::current());
            let name = b.action().name();
            // Layout actions have no palette entry: their name, as words.
            let title = all_commands()
                .iter()
                .find(|c| c.action == name)
                .map_or_else(|| action_words(name), |c| c.title.to_string());
            if !out.iter().any(|(k, _)| *k == key) {
                out.push((key, title));
            }
        }
        out.sort();
        out
    }
}

/// `chorong::FocusPaneDown` as `Focus Pane Down`.
fn action_words(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.rsplit("::").next().unwrap_or(name).chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        OpenInEditor,
        OpenOnGitHub,
        CopyLink,
        CopyShareLink,
        SwitchTheme,
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c(
        "chorong::OpenInEditor",
        "Open in External Editor",
        Layer::View,
        200,
    ),
    c("chorong::OpenOnGitHub", "Open on GitHub", Layer::View, 210),
    c(
        "chorong::CopyLink",
        "Copy chorong:// Link",
        Layer::View,
        220,
    ),
    c(
        "chorong::CopyShareLink",
        "Copy chorong:// Link for Sharing",
        Layer::View,
        230,
    ),
    c("chorong::SwitchTheme", "Switch Theme", Layer::View, 240),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, OpenInEditor, |this, _w, cx| this.open_in_editor(cx));
    let d = on!(d, cx, CopyLink, |this, _w, cx| this.copy_link(false, cx));
    let d = on!(d, cx, CopyShareLink, |this, _w, cx| this
        .copy_link(true, cx));
    let d = on!(d, cx, OpenOnGitHub, |this, _w, cx| {
        if this.remote_header.is_some() {
            this.open_on_host(cx);
        } else {
            this.unavailable("Open on GitHub", cx);
        }
    });
    unavailable!(
        d,
        cx,
        SwitchTheme => "Switch theme",
    )
}

/// What Build Index does for a folder, worked out off the UI thread (a folder walk, the
/// trust file and `git remote`), in the order the checks were always made.
#[derive(Debug, PartialEq, Eq)]
enum BuildDecision {
    NoProjects,
    Build,
    /// A remote project in Chorong's cache: its approval panel first.
    Approve,
    /// Ask once for this repository, with these notes (blocked projects, a changed remote).
    Ask(Vec<String>),
}

/// With the trust file's upgrade for an older entry, which the caller applies on the UI thread.
fn build_decision(
    canonical: &std::path::Path,
    trust: &crate::trust::TrustStore,
) -> (BuildDecision, Option<crate::trust::Upgrade>) {
    if !indexing::has_projects(canonical) {
        return (BuildDecision::NoProjects, None);
    }
    let (found, upgrade) = trust.check_read_only(canonical);
    let decision = match found {
        Trust::Trusted => BuildDecision::Build,
        Trust::NeedsApproval(_) => BuildDecision::Approve,
        trust => {
            let mut notes: Vec<String> = indexing::plan(canonical)
                .into_iter()
                .filter_map(|i| i.blocked.map(|b| format!("{}: {b}", i.label)))
                .collect();
            if let Trust::Changed { was, now } = trust {
                let name = |r: Option<String>| r.unwrap_or_else(|| "no remote".into());
                notes.insert(
                    0,
                    format!(
                        "You trusted this folder when its remote was {}; it is now {}. \
                         Different code may be here.",
                        name(was),
                        name(now)
                    ),
                );
            }
            BuildDecision::Ask(notes)
        }
    };
    (decision, upgrade)
}

/// Whether a build check started as `generation` still acts: no newer request was made
/// (`latest`), and no build is running.
fn build_check_lands(latest: Generation, generation: Generation, building: bool) -> bool {
    latest.is(generation) && !building
}

#[cfg(test)]
mod action_words_tests {
    #[test]
    fn splits_camel_case() {
        assert_eq!(
            super::action_words("chorong::FocusPaneDown"),
            "Focus Pane Down"
        );
    }
}

#[cfg(test)]
mod build_check_tests {
    use super::*;

    #[test]
    fn a_build_check_acts_only_while_newest_and_idle() {
        let mut latest = Generation::default();
        let ticket = latest.next();
        assert!(build_check_lands(latest, ticket, false));
        let newer = latest.next();
        assert!(
            !build_check_lands(latest, ticket, false),
            "a newer request was made"
        );
        assert!(
            !build_check_lands(latest, newer, true),
            "a build is running"
        );
    }

    #[test]
    fn a_folder_without_projects_is_not_asked_about() {
        let dir = tempfile::tempdir().unwrap();
        let trust = crate::trust::TrustStore::at(dir.path().join("trusted.json"), |_| None);
        assert_eq!(
            build_decision(dir.path(), &trust).0,
            BuildDecision::NoProjects
        );
    }

    #[test]
    fn an_untrusted_project_asks_and_a_trusted_one_builds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
        let root = root.canonicalize().unwrap();
        let trust = crate::trust::TrustStore::at(dir.path().join("trusted.json"), |_| None);
        assert!(matches!(
            build_decision(&root, &trust).0,
            BuildDecision::Ask(_)
        ));
        trust.trust(&root).unwrap();
        assert_eq!(build_decision(&root, &trust).0, BuildDecision::Build);
    }
}

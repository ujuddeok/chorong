//! "Prepare and index" for one of Chorong's cache worktrees: asks the backend whether the
//! worktree may be prepared and indexed, shows the approval panel when it needs the reader's
//! say-so, runs the recipe in the background (progress and Cancel in the status bar), then
//! starts the indexing the viewer already has. "Index only" takes the same approval and skips
//! the recipe.
//!
//! The decision and every sentence are in `crate::remote::prepare` (pure, tested); the backend
//! calls are in `crate::remote::adapter`. Nothing here runs until the reader clicks: Open
//! Remote's box (for that open only), or the index banner's "Prepare and index" or "Index
//! only" (design/manual-start.md). Unchecked, declined, cancelled and not-allowed windows stay
//! syntax only, with the banner offering the start.

use super::*;
use crate::remote::adapter::{self, Assessment, RunEnd};
use crate::remote::prepare::{self as prep, Decision, FailPanel, Panel, Status};
use crate::remote::prepare::{Outcome, recheck_after_update};
use cho_sources::PrepareStage;

/// How long "prepared" stays in the status bar once indexing has taken over.
const PREPARED_SHOWN: Duration = Duration::from_secs(6);

pub(super) struct PrepareState {
    pub(super) status: Status,
    /// "Index only": the approval is asked for, the recipe is not run.
    index_only: bool,
    /// The backend's last answer: the recipe view to run or to approve.
    assessment: Option<Assessment>,
    cancel: adapter::Cancel,
    /// The last run's failure, shown as a notice that does not block (Retry, Dismiss).
    pub(super) failure: Option<FailPanel>,
    /// The failure notice's "Details" disclosure.
    pub(super) details: crate::remote::failure::Disclosure,
    /// Written by the background run's progress callback.
    live: Arc<std::sync::Mutex<Option<PrepareStage>>>,
}

impl PrepareState {
    fn new(status: Status) -> Self {
        Self {
            status,
            index_only: false,
            assessment: None,
            failure: None,
            details: Default::default(),
            cancel: adapter::Cancel::new(),
            live: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

impl Chorong {
    /// A window Open Remote just made. `checked`: the "Prepare and index" box, ticked for this
    /// open only. Unchecked, nothing starts: the index banner offers it.
    pub(super) fn begin_prepare(&mut self, checked: bool, cx: &mut Context<Self>) {
        self.prepare_wanted = checked;
        if checked {
            self.start_prepare_flow(false, cx);
        }
    }

    /// The reader asked: "Prepare and index", or "Index only" (`index_only`), which asks for
    /// the same approval and runs no recipe.
    pub(super) fn start_prepare_flow(&mut self, index_only: bool, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if self.prepare_busy() || self.job.is_some() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let assessed = cx
                .background_spawn(async move { adapter::assess(&root) })
                .await;
            this.update(cx, |this, cx| {
                this.prepare_assessed(assessed, false, index_only, cx)
            })
            .ok();
        })
        .detach();
    }

    fn prepare_assessed(
        &mut self,
        a: Assessment,
        after_approval: bool,
        index_only: bool,
        cx: &mut Context<Self>,
    ) {
        let summary = a.summary();
        let decision = prep::decide(true, &a.access, summary.as_ref().map_err(String::as_str));
        let decision = match decision {
            Decision::Run if index_only => Decision::IndexNow,
            d => d,
        };
        match decision {
            Decision::Nothing => self.prepare = None,
            Decision::SyntaxOnly(why) => self.prepare_skip(Some(why), cx),
            Decision::Ask(_) if after_approval => self.prepare_skip(
                Some("Not indexed: the approval did not cover this commit".into()),
                cx,
            ),
            Decision::Ask(panel) => {
                let mut st = PrepareState::new(Status::Asking);
                st.index_only = index_only;
                st.assessment = Some(a);
                self.prepare = Some(st);
                self.overlay = Some(Overlay::PrepareApproval(panel));
            }
            Decision::Run => {
                let mut st = PrepareState::new(Status::Skipped);
                st.assessment = Some(a);
                self.prepare = Some(st);
                self.start_prepare_run(false, cx);
            }
            Decision::IndexNow => {
                self.prepare = None;
                self.prepare_note = None;
                self.start_build(false, cx);
            }
        }
        cx.notify();
    }

    /// Syntax only for this window; `why` is said once.
    fn prepare_skip(&mut self, why: Option<String>, cx: &mut Context<Self>) {
        self.prepare = Some(PrepareState::new(Status::Skipped));
        if matches!(self.overlay, Some(Overlay::PrepareApproval(_))) {
            self.overlay = None;
        }
        if let Some(w) = why {
            self.notify_text(w, cx);
        }
        cx.notify();
    }

    /// "Approve and prepare": the same approval `cho prepare approve` stores, then the
    /// backend is asked again (the approval is what lets it run).
    pub(super) fn prepare_approve(&mut self, cx: &mut Context<Self>) {
        let (Some(root), Some(view)) = (
            self.root.clone(),
            self.prepare
                .as_ref()
                .and_then(|p| p.assessment.as_ref())
                .and_then(|a| a.view.as_ref().ok().cloned()),
        ) else {
            return;
        };
        let index_only = self.prepare.as_ref().is_some_and(|p| p.index_only);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    adapter::approve(&view)?;
                    Ok::<_, String>(adapter::assess(&root))
                })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(a) => this.prepare_assessed(a, true, index_only, cx),
                Err(e) => this.prepare_skip(Some(format!("Could not save the approval: {e}")), cx),
            })
            .ok();
        })
        .detach();
    }

    /// "Open without indexing" (or Esc) on the approval panel: syntax only; nothing runs.
    pub(super) fn prepare_decline(&mut self, cx: &mut Context<Self>) {
        self.prepare_skip(None, cx);
    }

    /// Retry after a failure: runs again even if the last run covers the recipe.
    pub(super) fn prepare_retry(&mut self, cx: &mut Context<Self>) {
        self.start_prepare_run(true, cx);
    }

    /// Dismiss on the failure notice. The status bar still says the prepare failed.
    pub(super) fn prepare_dismiss_failure(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.prepare.as_mut() {
            p.failure = None;
        }
        cx.notify();
    }

    /// After Update moved the worktree to a new head: assess again only when "Prepare and
    /// index" was ticked for this window (when it opened, or on this Update: `ticked`). The
    /// backend decides what follows: the approval panel, a new run (its record no longer
    /// covers the recipe, files or toolchain) or just a re-index. Otherwise the banner offers
    /// it.
    pub(super) fn prepare_after_update(
        &mut self,
        old_head: &str,
        new_head: &str,
        ticked: bool,
        cx: &mut Context<Self>,
    ) {
        let wanted = self.prepare_wanted || ticked;
        if recheck_after_update(wanted, self.prepare_busy(), old_head, new_head) {
            self.start_prepare_flow(false, cx);
        }
    }

    fn start_prepare_run(&mut self, force: bool, cx: &mut Context<Self>) {
        let (Some(root), Some(state)) = (self.root.clone(), self.prepare.as_mut()) else {
            return;
        };
        let Some(view) = state.assessment.as_ref().and_then(|a| a.view.as_ref().ok()) else {
            return;
        };
        let view = view.clone();
        let steps = &view.effective.recipe.steps;
        state.status = Status::Running {
            step: 1,
            of: steps.len(),
            command: steps.first().map(|s| s.display()).unwrap_or_default(),
        };
        state.failure = None;
        state.cancel = adapter::Cancel::new();
        state.live = Arc::new(std::sync::Mutex::new(None));
        let (cancel, live) = (state.cancel.clone(), state.live.clone());
        cx.notify();
        // The step the run is on, polled for the status bar (no timer once it ends).
        let poll = live.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let again = this
                    .update(cx, |this, cx| {
                        let Some(p) = this
                            .prepare
                            .as_mut()
                            .filter(|p| Arc::ptr_eq(&p.live, &poll) && p.status.running())
                        else {
                            return false;
                        };
                        let now = poll.lock().ok().and_then(|g| g.clone());
                        if let (Some(s), Status::Running { .. }) = (now, &p.status) {
                            p.status = Status::Running {
                                step: s.step,
                                of: s.of,
                                command: s.command,
                            };
                        }
                        this.redraw_chrome(cx);
                        true
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let end = cx
                .background_spawn(async move {
                    adapter::run_prepare(
                        &root,
                        &view,
                        force,
                        &move |s| {
                            if let Ok(mut g) = live.lock() {
                                *g = Some(s);
                            }
                        },
                        &cancel,
                    )
                })
                .await;
            this.update(cx, |this, cx| this.prepare_finished(end, cx))
                .ok();
        })
        .detach();
    }

    fn prepare_finished(&mut self, end: RunEnd, cx: &mut Context<Self>) {
        if self.prepare.is_none() {
            return;
        }
        match end {
            // The reader stopped it: silent, syntax only.
            RunEnd::Cancelled => self.prepare_skip(None, cx),
            RunEnd::Done {
                ran,
                result,
                message,
                log,
                log_tail,
            } => {
                let outcome = prep::after_run(ran, result, message.as_deref(), &log, &log_tail);
                self.prepare_ended(outcome, cx);
            }
            RunEnd::Error(m, detail) => self.prepare_ended(prep::after_error(&m, detail), cx),
        }
        cx.notify();
    }

    /// A run ended: indexing starts at once either way. A failure only makes the index
    /// `partial (prepare failed)` and shows a notice that does not block.
    fn prepare_ended(&mut self, outcome: Outcome, cx: &mut Context<Self>) {
        let (status, note, failure) = outcome.into_parts();
        let failed = failure.is_some();
        self.prepare_note = note;
        if let Some(state) = self.prepare.as_mut() {
            state.status = status;
            state.failure = failure;
            state.details = Default::default();
        }
        // While a build runs (the partial one from a failed run), exactly one more starts
        // when it ends.
        if self.job.is_some() {
            self.rebuild_queued = true;
        } else {
            self.start_build(false, cx);
        }
        if !failed {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PREPARED_SHOWN).await;
                this.update(cx, |this, cx| {
                    if this
                        .prepare
                        .as_ref()
                        .is_some_and(|p| matches!(p.status, Status::Prepared { .. }))
                    {
                        this.prepare = None;
                        this.redraw_chrome(cx);
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    /// Cancel in the status bar: stops the running step and its children.
    pub(super) fn cancel_prepare(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.prepare.as_mut().filter(|p| p.status.running()) {
            p.cancel.cancel();
            p.status = Status::Cancelling;
            cx.notify();
        }
    }

    /// The words for the status bar, and whether Cancel is offered.
    pub(super) fn prepare_text(&self) -> Option<(String, bool)> {
        let p = self.prepare.as_ref()?;
        // "Not indexed" is only true while there is no index and nothing is building one.
        if p.status == Status::Skipped
            && (self.job.is_some() || matches!(self.index, IndexState::Ready(..)))
        {
            return None;
        }
        Some((p.status.text()?, p.status.running()))
    }

    // ---- Panels ----

    /// A button with no click handler of its own.
    pub(super) fn plain_button(
        &self,
        id: &'static str,
        label: &'static str,
        primary: bool,
    ) -> gpui::Stateful<Div> {
        let t = &self.theme;
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(hsla(t.border))
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .when(primary, |d| d.bg(hsla(t.element_selected)))
            .hover(|s| s.bg(hsla(t.element_selected)))
            .child(label)
    }

    fn dialog_button(
        &self,
        id: &'static str,
        label: &'static str,
        primary: bool,
        accept: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        self.plain_button(id, label, primary)
            .flex()
            .items_center()
            .gap_2()
            .children(self.dialog_key(accept))
            .on_click(cx.listener(move |this, _, window, cx| {
                if accept {
                    this.dialog_accept(window, cx);
                } else {
                    this.dialog_decline(cx);
                }
                cx.notify();
            }))
    }

    /// The "Details" disclosure: a closed row that opens a monospace block with the backend's
    /// detail (the command and its error output) and a Copy details button. Nothing of the
    /// detail is shown while it is closed. GPUI text can't be selected, so Copy is the way out.
    pub(super) fn details_block(
        &self,
        id: &'static str,
        detail: &str,
        d: crate::remote::failure::Disclosure,
        toggle: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let t = &self.theme;
        let head = div()
            .id(id)
            .text_size(px(12.))
            .text_color(hsla(t.text_muted))
            .cursor_pointer()
            .hover(|s| s.text_color(hsla(t.text)))
            .child(d.label())
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this, cx);
                cx.notify();
            }));
        let mut col = div().flex().flex_col().gap_1().child(head);
        if d.open {
            let text = detail.to_string();
            col = col
                .child(
                    self.scroll_areas
                        .get("details-text")
                        .vertical_max(
                            "details-text",
                            self.features.probe_if_on(Feature::Overscroll),
                            px(220.),
                            div()
                                .p_2()
                                .font_family(self.font.clone())
                                .text_size(px(12.))
                                .text_color(hsla(t.text))
                                .children(detail.lines().map(|l| div().child(l.to_string()))),
                        )
                        .rounded_sm()
                        .bg(hsla(t.editor_background)),
                )
                .child(
                    div().flex().child(
                        self.plain_button("details-copy", "Copy details", false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    text.clone(),
                                ));
                                this.notify_text("Copied the details", cx);
                            })),
                    ),
                );
        }
        col
    }

    /// A block of monospace lines in a box (commands, files, an output tail).
    pub(super) fn code_block(&self, id: &'static str, lines: Vec<String>) -> Div {
        self.scroll_areas
            .get(id)
            .vertical_max(
                id,
                self.features.probe_if_on(Feature::Overscroll),
                px(220.),
                div()
                    .p_2()
                    .font_family(self.font.clone())
                    .text_size(px(12.))
                    .children(
                        lines
                            .into_iter()
                            .map(|l| div().whitespace_nowrap().child(l)),
                    ),
            )
            .rounded_sm()
            .bg(hsla(self.theme.editor_background))
    }

    /// The approval panel: the recipe's commands, the changed build files, and what prepare
    /// runs and where.
    pub(super) fn render_prepare_approval(
        &self,
        p: &Panel,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let muted = hsla(t.text_muted);
        let warn = hsla(t.ui.warning.unwrap_or(t.ui.text_accent));
        let mut card = self
            .card(640.)
            .p_4()
            .gap_2()
            .child(div().font_weight(FontWeight::MEDIUM).child(p.title.clone()))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child(format!("Recipe: {}", p.source)),
            );
        if let Some(why) = &p.why {
            card = card.child(div().text_color(warn).child(why.clone()));
        }
        card =
            card.child(div().text_size(px(11.5)).text_color(muted).child(
                if p.commands.is_empty() {
                    "Commands"
                } else {
                    "Commands, run in this order"
                },
            ))
            .child(self.code_block("prepare-commands", p.command_lines()));
        for n in &p.notes {
            card = card.child(div().text_size(px(12.)).text_color(muted).child(n.clone()));
        }
        if let Some(h) = &p.files_heading {
            card = card.child(div().text_size(px(11.5)).text_color(muted).child(h.clone()));
            card = card.child(
                self.code_block(
                    "prepare-files",
                    p.files
                        .iter()
                        .map(|(path, change)| format!("{path}  ({change})"))
                        .collect(),
                ),
            );
        }
        card = card
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child(prep::WHERE_NOTE),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child(prep::INDEX_NOTE),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(self.dialog_button(
                        "prepare-decline",
                        "Open without indexing",
                        false,
                        false,
                        cx,
                    ))
                    .child(self.dialog_button(
                        "prepare-approve",
                        p.approve_label(),
                        true,
                        true,
                        cx,
                    )),
            );
        self.layer(card, cx)
    }

    /// The failure notice: the failing step, the end of its output, the log, Retry and
    /// Dismiss. It sits over the corner of the window and takes no input away from it.
    pub(super) fn render_prepare_failure(
        &self,
        p: &FailPanel,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let muted = hsla(t.text_muted);
        let error = hsla(t.ui.error.unwrap_or(t.ui.text_accent));
        let mut card = self
            .card(560.)
            .absolute()
            .right(px(12.))
            .bottom(px(34.))
            .p_3()
            .gap_2()
            .child(div().font_weight(FontWeight::MEDIUM).child(format!(
                "{}. Indexing anyway; the index will read partial (prepare failed).",
                p.title
            )));
        if !p.message.is_empty() {
            card = card.child(div().text_color(error).child(p.message.clone()));
        }
        if !p.tail.is_empty() {
            card = card
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(muted)
                        .child("The end of its output"),
                )
                .child(self.code_block("prepare-output", p.tail.clone()));
        }
        if let Some(detail) = &p.detail {
            let d = self.prepare.as_ref().map(|s| s.details).unwrap_or_default();
            card = card.child(self.details_block(
                "prepare-details",
                detail,
                d,
                |this, _| {
                    if let Some(s) = this.prepare.as_mut() {
                        s.details.toggle();
                    }
                },
                cx,
            ));
        }
        if let Some(log) = &p.log {
            card = card.child(
                div()
                    .text_size(px(11.5))
                    .text_color(muted)
                    .child(format!("Log: {log}")),
            );
        }
        let text: String = std::iter::once(p.message.as_str())
            .filter(|m| !m.is_empty())
            .chain(p.tail.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        card.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .when(!text.is_empty(), |row| {
                    row.child(
                        self.plain_button("prepare-failed-copy", "Copy output", false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    text.clone(),
                                ));
                                this.notify_text("Copied the output", cx);
                            })),
                    )
                })
                .child(
                    self.plain_button("prepare-failed-dismiss", "Dismiss", false)
                        .on_click(cx.listener(|this, _, _, cx| this.prepare_dismiss_failure(cx))),
                )
                .child(
                    self.plain_button("prepare-retry", "Retry", true)
                        .on_click(cx.listener(|this, _, _, cx| this.prepare_retry(cx))),
                ),
        )
    }
}

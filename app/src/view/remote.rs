//! "Open Remote…" and "Open Recent…": the form that opens a branch, pull request or commit of a
//! remote repository, and the list of folders opened before.
//!
//! The form (`crate::remote::form`) is plain state; the open itself (`crate::remote::adapter`)
//! blocks on git and the network, so it runs on the background executor and the form stays
//! responsive. Success opens the worktree in a new window with the comparison started
//! (`Startup::remote`); a window that already shows a worktree of the same project is replaced.
//! Esc while an open runs hides the form and the open carries on; there is no way to cancel it
//! yet.

use super::*;
use crate::remote::adapter::{self, Done};
use crate::remote::failure::{Failure, Shown};
use crate::remote::form::{Field, Form, ProjectRow};
use crate::remote::header::{self, Header, Kind, PrState, PrUpdate};
use crate::remote::prepare::Prefs;
use crate::{Origin, RemoteStart, Startup};
use cho_core::RepoRef;

actions!(
    chorong,
    [
        OpenPath,
        OpenRecent,
        OpenRemote,
        RemoteClose,
        RemoteConfirm,
        RemoteDown,
        RemoteNext,
        RemotePaste,
        RemotePrev,
        RemoteUp,
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c("chorong::OpenRemote", "Open Remote\u{2026}", Layer::Go, 112),
    c(
        "chorong::OpenPath",
        "Open File or Folder\u{2026}",
        Layer::Go,
        113,
    ),
    c("chorong::OpenRecent", "Open Recent\u{2026}", Layer::Go, 114),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, OpenRecent, |this, _w, cx| this
        .open_recent_picker(cx));
    let d = on!(d, cx, OpenPath, |this, _w, cx| this.open_path_dialog(cx));
    let d = on!(d, cx, OpenRemote, |this, _w, cx| this.open_remote_form(cx));
    let d = on!(d, cx, RemoteClose, |this, _w, cx| this.remote_close(cx));
    let d = on!(d, cx, RemoteConfirm, |this, window, cx| this
        .remote_confirm(window, cx));
    let d = on!(d, cx, RemoteDown, |this, _w, cx| this.remote_move(1, cx));
    let d = on!(d, cx, RemoteNext, |this, _w, cx| this.remote_field(1, cx));
    let d = on!(d, cx, RemotePaste, |this, _w, cx| this.remote_paste(cx));
    let d = on!(d, cx, RemotePrev, |this, _w, cx| this.remote_field(-1, cx));
    on!(d, cx, RemoteUp, |this, _w, cx| this.remote_move(-1, cx))
}

/// An open in progress.
pub(super) struct Running {
    started: Instant,
    what: String,
    /// Where the open is, written by the background task's progress callback.
    stage: std::sync::Arc<std::sync::Mutex<String>>,
    /// Raised by Cancel; the background task holds a clone.
    cancel: adapter::Cancel,
    cancelling: bool,
}

impl Running {
    fn stage_text(&self) -> String {
        if self.cancelling {
            return "cancelling".into();
        }
        self.stage.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

pub(super) struct RemoteState {
    form: Form,
    error: Option<Shown>,
    /// The error's "Details" disclosure (closed for each new error).
    details: crate::remote::failure::Disclosure,
    /// The request of the last open, for the buttons of an ambiguous name.
    last: Option<adapter::Request>,
    running: Option<Running>,
    /// False after Esc while an open runs: the form is hidden, the open carries on.
    pub(super) visible: bool,
    /// The `chorong://` link this open serves (a pull-request link the reader agreed to fetch):
    /// the new window gets it, and a Before the host doesn't have is replaced by the merge
    /// base.
    link: Option<LinkInfo>,
    /// The box whose whole text is selected (Cmd-A), if any: typing or pasting replaces it.
    selected_all: Option<Field>,
}

/// The text of the form's box `field` (`None` for the checkbox).
fn field_text(f: &mut Form, field: Field) -> Option<&mut String> {
    match field {
        Field::Project => Some(&mut f.project_query),
        Field::CloneUrl => Some(&mut f.clone_url),
        Field::Input => Some(&mut f.input),
        Field::Base => Some(&mut f.base),
        Field::Prepare => None,
    }
}

impl Chorong {
    pub(super) fn remote_visible(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.visible)
    }

    pub(super) fn open_remote_form(&mut self, cx: &mut Context<Self>) {
        if self.remote.as_ref().is_some_and(|r| r.running.is_some()) {
            if let Some(r) = &mut self.remote {
                r.visible = true;
            }
            return;
        }
        self.picker = None;
        self.overlay = None;
        self.key_focus.leave(KeyFocus::Search);
        // Projects and the last used one come from files: read them off the UI thread.
        cx.spawn(async move |this, cx| {
            let (known, prefs) = cx
                .background_spawn(async { (adapter::projects(), Prefs::load()) })
                .await;
            this.update(cx, |this, cx| {
                let last = prefs
                    .last_project
                    .as_deref()
                    .and_then(|l| RepoRef::parse(l).ok());
                let mut form = Form::new(known, last.as_ref());
                form.set_prefs(prefs);
                // A URL on the clipboard is offered, selected in effect by being the whole
                // text of the box; nothing happens until Enter.
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    form.prefill(&text);
                }
                let load = form.chosen.as_ref().map(|c| c.repo.clone());
                this.remote = Some(RemoteState {
                    form,
                    error: None,
                    details: Default::default(),
                    last: None,
                    running: None,
                    visible: true,
                    link: None,
                    selected_all: None,
                });
                if let Some(repo) = load {
                    this.remote_load_branches(repo, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remote_load_branches(&mut self, repo: RepoRef, cx: &mut Context<Self>) {
        // A project with no clone has no refs to complete from: ask the remote once for
        // each project per opening of the form (never per keystroke).
        let ask = self.remote.as_mut().is_some_and(|rs| {
            let first = !rs.form.asked.contains(&repo);
            if first {
                rs.form.asked.push(repo.clone());
            }
            first
        });
        let clone_url = self
            .remote
            .as_ref()
            .and_then(|rs| rs.form.chosen.as_ref())
            .and_then(|c| c.clone_url.clone());
        let cancel = self
            .remote
            .as_ref()
            .map(|rs| rs.form.cancel.clone())
            .unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let r = repo.clone();
            let branches = cx
                .background_spawn(async move {
                    let mut branches = adapter::cached_branches(&r);
                    let mut default = None;
                    if branches.is_empty()
                        && ask
                        && let Ok(heads) =
                            adapter::remote_branches(&r, clone_url.as_deref(), &cancel)
                    {
                        branches = heads.branches;
                        default = heads.default;
                    }
                    (branches, default)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(rs) = &mut this.remote
                    && rs.form.chosen.as_ref().is_some_and(|c| c.repo == repo)
                {
                    rs.form.branches = branches.0;
                    if branches.1.is_some() {
                        rs.form.remote_default = branches.1;
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn remote_close(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &mut self.remote else { return };
        if let Some(run) = &r.running {
            r.visible = false;
            let what = run.what.clone();
            self.notify_text(format!("{what} continues in the background"), cx);
        } else {
            // Closing the form stops a branch listing still running.
            r.form.cancel.cancel();
            self.remote = None;
        }
    }

    /// Text typed into the focused box of the form.
    pub(super) fn remote_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(r) = self.remote.as_mut().filter(|r| r.running.is_none()) else {
            return;
        };
        let f = &mut r.form;
        let field = f.field;
        if field == Field::Project {
            f.project_sel = 0;
        }
        let Some(text) = field_text(f, field) else {
            if matches!(event.keystroke.key.as_str(), "space" | " ") {
                f.toggle_prepare();
                cx.notify();
            }
            return;
        };
        // Cmd-V is the RemotePaste action (first line only, trimmed).
        let mut all = r.selected_all == Some(field);
        let edit = edit_field(&event.keystroke, text, &mut all, None);
        r.selected_all = all.then_some(field);
        match edit {
            FieldEdit::None => {}
            FieldEdit::Text => self.remote_edited(cx),
            FieldEdit::Selection => cx.notify(),
            FieldEdit::Copy(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
        }
    }

    fn remote_edited(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &mut self.remote else { return };
        r.error = None;
        r.form.completion = None;
        if r.form.field == Field::Input
            && r.form.input_changed()
            && let Some(c) = &r.form.chosen
        {
            let repo = c.repo.clone();
            self.remote_load_branches(repo, cx);
        }
        cx.notify();
    }

    fn remote_paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        let text = text.lines().next().unwrap_or("").trim().to_string();
        let Some(r) = self.remote.as_mut().filter(|r| r.running.is_none()) else {
            return;
        };
        let field = r.form.field;
        let all = r.selected_all.take() == Some(field);
        if let Some(t) = field_text(&mut r.form, field) {
            if all {
                t.clear();
            }
            t.push_str(&text);
        }
        self.remote_edited(cx);
    }

    fn remote_field(&mut self, delta: isize, _cx: &mut Context<Self>) {
        let Some(r) = &mut self.remote else { return };
        let order = r.form.fields();
        let at = order.iter().position(|f| *f == r.form.field).unwrap_or(0);
        r.form.field = order[(at as isize + delta).rem_euclid(order.len() as isize) as usize];
        r.form.completion = None;
    }

    fn remote_move(&mut self, delta: isize, _cx: &mut Context<Self>) {
        let Some(r) = &mut self.remote else { return };
        match r.form.field {
            Field::Project => {
                let last = r.form.project_rows().len().saturating_sub(1);
                r.form.project_sel = r.form.project_sel.saturating_add_signed(delta).min(last);
            }
            Field::CloneUrl | Field::Prepare => {}
            Field::Input | Field::Base => {
                let n = r.form.completions(&mut self.ranker).len();
                let cur = r.form.completion;
                r.form.completion = match (cur, delta > 0) {
                    (None, true) => (n > 0).then_some(0),
                    (None, false) => None,
                    (Some(i), true) => Some((i + 1).min(n.saturating_sub(1))),
                    (Some(0), false) => None,
                    (Some(i), false) => Some(i - 1),
                };
            }
        }
    }

    pub(super) fn remote_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.remote.as_mut().filter(|r| r.running.is_none()) else {
            return;
        };
        match r.form.field {
            Field::Project => {
                let rows = r.form.project_rows();
                if r.form.project_query.trim().is_empty() && r.form.chosen.is_some() {
                    r.form.field = Field::Input;
                } else if let Some(row) = rows.get(r.form.project_sel).cloned()
                    && r.form.choose_row(&row)
                {
                    r.form.field = if r.form.needs_clone_url() {
                        Field::CloneUrl
                    } else {
                        Field::Input
                    };
                    let repo = row_repo(&row);
                    if let Some(repo) = repo {
                        self.remote_load_branches(repo, cx);
                    }
                }
            }
            Field::CloneUrl | Field::Prepare => self.remote_submit(window, cx),
            Field::Input | Field::Base => {
                if let Some(i) = r.form.completion.take() {
                    let names = r.form.completions(&mut self.ranker);
                    if let Some(name) = names.get(i) {
                        match r.form.field {
                            Field::Input => r.form.input = name.clone(),
                            _ => r.form.base = name.clone(),
                        }
                        r.form.input_changed();
                    }
                } else {
                    self.remote_submit(window, cx);
                }
            }
        }
    }

    /// Starts the open on the background executor.
    pub(super) fn remote_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.remote.as_mut().filter(|r| r.running.is_none()) else {
            return;
        };
        let req = match r.form.request() {
            Ok(q) => q,
            Err(e) => {
                r.error = Some(Shown::plain(e));
                return;
            }
        };
        self.remote_start(req, window, cx);
    }

    /// The buttons of an ambiguous name: the same open again, as the chosen kind.
    fn remote_choose(
        &mut self,
        kind: cho_sources::TargetKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut req) = self
            .remote
            .as_ref()
            .filter(|r| r.running.is_none())
            .and_then(|r| r.last.clone())
        else {
            return;
        };
        req.kind = kind;
        self.remote_start(req, window, cx);
        cx.notify();
    }

    /// Cancel: stops the open (the form or the status bar). The open ends with a silent
    /// cancelled error.
    pub(super) fn cancel_open(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.remote.as_mut().and_then(|r| r.running.as_mut()) {
            run.cancel.cancel();
            run.cancelling = true;
        }
        cx.notify();
    }

    fn remote_start(&mut self, req: adapter::Request, window: &mut Window, cx: &mut Context<Self>) {
        self.remote_start_at(req, window.window_handle(), cx);
    }

    /// The reader agreed to fetch the pull request a link names: opens it through the same
    /// path as the form (progress in the status bar, Cancel), with the form hidden.
    pub(super) fn begin_link_open(
        &mut self,
        ask: crate::AskPr,
        handle: gpui::AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        if self.remote.as_ref().is_some_and(|r| r.running.is_some()) {
            self.notify_text("Another open is running; try the link again after it", cx);
            return;
        }
        let crate::AskPr {
            mut info,
            repo,
            number,
            ..
        } = ask;
        // The open fetches, so a `rev` missing afterwards is gone from the host.
        info.fetched = true;
        let req = adapter::Request {
            repo: repo.clone(),
            clone_url: None,
            kind: cho_sources::TargetKind::PullRequest(number),
            path: info.path.clone(),
            line: info.line,
            base: info.base.clone(),
        };
        cx.spawn(async move |this, cx| {
            let (mut known, prefs) = cx
                .background_spawn(async { (adapter::projects(), Prefs::load()) })
                .await;
            if !known.contains(&repo) {
                known.push(repo.clone());
            }
            this.update(cx, |this, cx| {
                let mut form = Form::new(known, Some(&repo));
                form.set_prefs(prefs);
                this.remote = Some(RemoteState {
                    form,
                    error: None,
                    details: Default::default(),
                    last: None,
                    running: None,
                    visible: false,
                    link: Some(info),
                    selected_all: None,
                });
                this.remote_start_at(req, handle, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remote_start_at(
        &mut self,
        req: adapter::Request,
        handle: gpui::AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self.remote.as_mut().filter(|r| r.running.is_none()) else {
            return;
        };
        let what = describe(&req);
        let prepare = r.form.prepare;
        let linked = r.link.is_some();
        let stage = std::sync::Arc::new(std::sync::Mutex::new("starting".to_string()));
        let cancel = adapter::Cancel::new();
        r.error = None;
        r.last = Some(req.clone());
        r.running = Some(Running {
            started: Instant::now(),
            what: what.clone(),
            stage: stage.clone(),
            cancel: cancel.clone(),
            cancelling: false,
        });
        if let Err(e) = Prefs::remember(&req.repo.to_string()) {
            eprintln!("chorong-app: {e}");
        }
        let same_project = self
            .root
            .as_deref()
            .and_then(adapter::worktree_project)
            .is_some_and(|p| p == req.repo);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let progress = move |s| {
                        if let Ok(mut g) = stage.lock() {
                            *g = s;
                        }
                    };
                    if linked {
                        adapter::open_link(&req, &progress, &cancel)
                    } else {
                        adapter::open(&req, &progress, &cancel)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.remote_finished(result, handle, same_project, None, prepare, cx)
            })
            .ok();
        })
        .detach();
        // The elapsed time: on the form's progress row (quick ticks), or, with the form hidden,
        // in the status bar (one tick a second). Stops when the open ends.
        cx.spawn(async move |this, cx| {
            loop {
                let hidden = this
                    .update(cx, |this, _| {
                        this.remote.as_ref().is_some_and(|r| !r.visible)
                    })
                    .unwrap_or(false);
                cx.background_executor()
                    .timer(Duration::from_millis(if hidden { 1000 } else { 250 }))
                    .await;
                let live = this.update(cx, |this, cx| {
                    let running = this.remote.as_ref().is_some_and(|r| r.running.is_some());
                    if running {
                        if this.remote_visible() {
                            cx.notify();
                        } else {
                            this.redraw_chrome(cx);
                        }
                    }
                    running
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// A failed open with no form to show it in: a notice, or a banner with the command to
    /// copy (e.g. a sign-in the host needs) so it stays until dismissed.
    fn show_open_failure(&mut self, s: crate::remote::failure::Shown, cx: &mut Context<Self>) {
        let text = format!("Open failed: {}", s.text);
        match s.command {
            Some(cmd) => {
                self.ed_mut().banner = Some(Banner {
                    text,
                    detail: Some(cmd.clone()),
                    actions: vec![
                        ("Copy command".into(), BannerAction::Copy(cmd)),
                        ("Dismiss".into(), BannerAction::Dismiss),
                    ],
                    quiet: false,
                });
                cx.notify();
            }
            None => self.notify_text(text, cx),
        }
    }

    fn remote_finished(
        &mut self,
        result: Result<Done, Failure>,
        handle: gpui::AnyWindowHandle,
        same_project: bool,
        keep: Option<KeepPlace>,
        prepare: bool,
        cx: &mut Context<Self>,
    ) {
        let visible = self.remote_visible();
        if let Some(h) = &mut self.remote_header {
            h.updating = false;
        }
        match result {
            Err(e) => {
                // A cancel is silent: the form goes back to editing, or closes if hidden.
                let shown = e.shown();
                let offers = shown
                    .as_ref()
                    .is_some_and(|s| !s.choices.is_empty() || s.command.is_some());
                let route = failure_route(visible, self.remote.is_some(), shown.is_some(), offers);
                if route == FailureRoute::Form
                    && let Some(r) = &mut self.remote
                {
                    r.running = None;
                    r.error = shown;
                    r.details = Default::default();
                    // Buttons and commands need the form, even if it was hidden.
                    r.visible = true;
                } else {
                    self.remote = None;
                    if let Some(s) = shown.filter(|_| route == FailureRoute::Notice) {
                        self.show_open_failure(s, cx);
                    }
                }
            }
            Ok(done) => {
                let link = self.remote.take().and_then(|r| r.link);
                if !visible {
                    self.notify_text(format!("Opened {}", done.repo), cx);
                }
                let canon = |p: &std::path::Path| p.canonicalize().ok();
                if self.root.as_deref().and_then(canon) == canon(&done.worktree) {
                    // This window already shows that worktree.
                    let notes = done.notes.join(" ");
                    // Moved to a new head: the band shows it, and the pull request is
                    // checked again from that head.
                    let old_head = self
                        .remote_header
                        .as_ref()
                        .map(|h| h.header.head.clone())
                        .unwrap_or_default();
                    let new_head = done.header.head.clone();
                    self.remote_header = Some(HeaderState::new(done.header.clone()));
                    self.prepare_after_update(&old_head, &new_head, prepare, cx);
                    self.start_comparison(
                        done.before,
                        done.against,
                        done.merge_bases,
                        Some(notes).filter(|n| !n.is_empty()),
                        cx,
                    );
                    // Update: the files on disk moved with the worktree; reopen the one that
                    // was open at the declaration (else the line) it was at.
                    if let Some(k) = keep {
                        let target = Target::at(k.symbol, Some(k.line));
                        self.open(k.file, target, false, cx);
                    }
                } else {
                    let mut startup = startup_for(done, prepare, link);
                    if let Some(k) = keep {
                        startup.symbol = k.symbol;
                    }
                    // After this update: opening a window and closing this one while it is
                    // borrowed would panic.
                    cx.defer(move |cx| {
                        crate::open_window_with_defaults(startup, cx);
                        if same_project {
                            handle
                                .update(cx, |_, window, _| window.remove_window())
                                .ok();
                        }
                    });
                }
            }
        }
        cx.notify();
    }

    // ---- Open Recent ----

    /// Cmd-O: the system's Open dialog, for a file or a folder. The choice goes where a path
    /// from the command line goes: a file in this window's folder opens here; another file or
    /// folder brings forward the window that shows it, or opens a new one.
    pub(super) fn open_path_dialog(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                cx.update(|cx| crate::send_link(path.to_string_lossy().into_owned(), cx))
                    .ok();
            }
        })
        .detach();
    }

    pub(super) fn open_recent_picker(&mut self, cx: &mut Context<Self>) {
        let cache = adapter::cache();
        let (mut labels, mut picks) = (Vec::new(), Vec::new());
        for folder in cho_sources::recent::list() {
            if !folder.is_dir() {
                continue;
            }
            labels.push(crate::recent::label(&folder, cache.as_ref()));
            picks.push(Pick::Folder(folder));
        }
        if labels.is_empty() {
            self.notify_text("No folders opened before", cx);
            return;
        }
        let hints = vec![String::new(); labels.len()];
        self.open_overlay_picker(PickerKind::Recent, labels, hints, picks);
    }

    // ---- Rendering ----

    pub(super) fn render_remote(
        &self,
        r: &RemoteState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let (muted, selected_bg) = (hsla(t.text_muted), hsla(t.element_selected));
        let error_colour = hsla(t.ui.error.unwrap_or(t.ui.text_accent));
        let f = &r.form;
        let busy = r.running.is_some();
        let cursor = || div().w(px(1.5)).h(px(16.)).bg(hsla(t.text));
        let selected_all = r.selected_all;
        let boxed = |id: &'static str, field: Field, text: &str, placeholder: &str| {
            let focused = f.field == field && !busy;
            div()
                .id(id)
                .h(px(30.))
                .px_2()
                .flex()
                .items_center()
                .border_1()
                .rounded_sm()
                .border_color(if focused {
                    hsla(t.ui.border_focused)
                } else {
                    hsla(t.border)
                })
                .cursor_text()
                .when(text.is_empty(), |d| {
                    d.child(div().text_color(muted).child(placeholder.to_string()))
                })
                .child(self.query_text(text, focused && selected_all == Some(field)))
                .when(focused, |d| d.child(cursor()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(r) = this.remote.as_mut().filter(|r| r.running.is_none()) {
                        r.form.field = field;
                        r.form.completion = None;
                    }
                    cx.notify();
                }))
        };
        let label = |text: &'static str| {
            div()
                .pt_1()
                .text_size(px(11.5))
                .text_color(muted)
                .child(text)
        };
        let row = |id: (&'static str, usize), text: String, selected: bool| {
            div()
                .id(id)
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .whitespace_nowrap()
                .cursor_pointer()
                .when(selected, |d| d.bg(selected_bg))
                .hover(|s| s.bg(selected_bg))
                .child(text)
        };
        let project_text = if f.field == Field::Project && !busy {
            f.project_query.clone()
        } else {
            String::new()
        };
        let chosen = f
            .chosen
            .as_ref()
            .map(|c| c.repo.to_string())
            .unwrap_or_default();
        let project_shown = if project_text.is_empty() && f.field != Field::Project {
            chosen.clone()
        } else {
            project_text
        };
        let completions = if busy {
            Vec::new()
        } else {
            let mut ranker = Ranker::paths();
            f.completions(&mut ranker)
        };
        let hint = f.hint();
        let base_default = f.base_default();
        let mut card = self
            .card(580.)
            .p_4()
            .gap_1()
            .child(div().font_weight(FontWeight::MEDIUM).child("Open Remote"))
            .child(label("Project"))
            .child(boxed(
                "remote-project",
                Field::Project,
                &project_shown,
                if chosen.is_empty() {
                    "Type to filter, or paste a repository URL"
                } else {
                    &chosen
                },
            ));
        if f.field == Field::Project && !busy {
            card = card.children(f.project_rows().into_iter().enumerate().take(7).map(
                |(i, row_kind)| {
                    let is_add = !matches!(row_kind, ProjectRow::Known(_));
                    row(("remote-proj-row", i), row_kind.label(), i == f.project_sel)
                        .when(is_add, |d| d.text_color(muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remote_pick_project(i, cx);
                        }))
                },
            ));
        }
        if f.needs_clone_url() {
            card = card.child(label("Clone URL")).child(boxed(
                "remote-clone-url",
                Field::CloneUrl,
                &f.clone_url,
                "https://host/scm/PROJECT/repo.git",
            ));
        }
        card = card.child(label("What to open")).child(boxed(
            "remote-input",
            Field::Input,
            &f.input,
            "Branch, #PR, commit, or URL",
        ));
        if f.field == Field::Input {
            card = card.children(completions.iter().enumerate().map(|(i, name)| {
                row(("remote-comp", i), name.clone(), f.completion == Some(i))
                    .on_click(cx.listener(move |this, _, _, cx| this.remote_pick_completion(i, cx)))
            }));
        }
        card = card.child(
            div()
                .min_h(px(18.))
                .text_size(px(12.))
                .text_color(if hint.error { error_colour } else { muted })
                .child(hint.text),
        );
        card = card.child(label("Compare against")).child(boxed(
            "remote-base",
            Field::Base,
            &f.base,
            &base_default,
        ));
        if f.field == Field::Base {
            card = card.children(completions.iter().enumerate().map(|(i, name)| {
                row(("remote-comp", i), name.clone(), f.completion == Some(i))
                    .on_click(cx.listener(move |this, _, _, cx| this.remote_pick_completion(i, cx)))
            }));
        }
        let prepare_focused = f.field == Field::Prepare && !busy;
        card = card.child(
            div()
                .id("remote-prepare")
                .mt_1()
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .gap_0p5()
                .rounded_sm()
                .border_1()
                .border_color(if prepare_focused {
                    hsla(t.ui.border_focused)
                } else {
                    hsla(t.border)
                })
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("remote-prepare-box")
                                .size(px(14.))
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .border_1()
                                .border_color(hsla(t.text))
                                .text_size(px(11.))
                                .child(if f.prepare { "\u{2713}" } else { "" }),
                        )
                        .child("Prepare and index"),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(muted)
                        .child(if f.prepare {
                            "Chorong asks before it runs the project's setup commands, then builds the \
                             index."
                        } else {
                            "Read-only, syntax only: nothing from the project runs and no index is \
                             built."
                        }),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(r) = this.remote.as_mut().filter(|r| r.running.is_none()) {
                        r.form.field = Field::Prepare;
                        r.form.completion = None;
                        r.form.toggle_prepare();
                    }
                    cx.notify();
                })),
        );
        if let Some(run) = &r.running {
            const FRAMES: [&str; 8] = [
                "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
                "\u{2827}",
            ];
            let elapsed = run.started.elapsed();
            let frame = FRAMES[(elapsed.as_millis() / 120) as usize % FRAMES.len()];
            card = card.child(
                div()
                    .pt_2()
                    .text_size(px(12.))
                    .flex()
                    .gap_2()
                    .child(frame)
                    .child(format!(
                        "{}\u{2026} {} ({}s)",
                        run.what,
                        run.stage_text(),
                        elapsed.as_secs()
                    )),
            );
        }
        if let Some(e) = &r.error {
            card = card.child(
                div()
                    .pt_2()
                    .text_size(px(12.))
                    .text_color(error_colour)
                    .child(e.text.clone()),
            );
            if let Some(detail) = &e.detail {
                card = card.child(div().pt_1().child(self.details_block(
                    "remote-details",
                    detail,
                    r.details,
                    |this, _| {
                        if let Some(r) = this.remote.as_mut() {
                            r.details.toggle();
                        }
                    },
                    cx,
                )));
            }
            if let Some(cmd) = &e.command {
                card = card.child(
                    div()
                        .id("remote-error-command")
                        .mt_1()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(hsla(t.border))
                        .font_family(self.font.clone())
                        .text_size(px(12.))
                        .cursor_pointer()
                        .tooltip(tip("Click to copy".into()))
                        .child(cmd.clone())
                        .on_click({
                            let cmd = cmd.clone();
                            cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(cmd.clone()));
                                this.notify_text("Copied the command", cx);
                            })
                        }),
                );
            }
            if !e.choices.is_empty() {
                card = card.child(div().flex().gap_2().pt_1().children(
                    e.choices.iter().enumerate().map(|(i, c)| {
                        let kind = c.kind.clone();
                        div()
                            .id(("remote-choice", i))
                            .px_3()
                            .py_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(hsla(t.border))
                            .cursor_pointer()
                            .font_weight(FontWeight::MEDIUM)
                            .hover(|s| s.bg(selected_bg))
                            .child(c.label.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remote_choose(kind.clone(), window, cx);
                            }))
                    }),
                ));
            }
        }
        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(hsla(t.border))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .when(primary, |d| d.bg(selected_bg))
                .hover(|s| s.bg(selected_bg))
                .child(text)
        };
        card = card.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .pt_2()
                .when(busy, |d| {
                    d.child(
                        button("remote-stop", "Cancel open", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cancel_open(cx);
                            },
                        )),
                    )
                })
                .child(
                    button("remote-cancel", if busy { "Hide" } else { "Cancel" }, false).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.remote_close(cx);
                            cx.notify();
                        }),
                    ),
                )
                .child(button("remote-open", "Open", true).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.remote_submit(window, cx);
                        cx.notify();
                    },
                ))),
        );
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(70.))
            .bg(gpui::hsla(0., 0., 0., 0.25))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.remote_close(cx);
                    cx.notify();
                }),
            )
            .child(card)
    }

    fn remote_pick_project(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(r) = self.remote.as_mut().filter(|r| r.running.is_none()) else {
            return;
        };
        let Some(row) = r.form.project_rows().get(index).cloned() else {
            return;
        };
        if r.form.choose_row(&row) {
            r.form.field = if r.form.needs_clone_url() {
                Field::CloneUrl
            } else {
                Field::Input
            };
            if let Some(repo) = row_repo(&row) {
                self.remote_load_branches(repo, cx);
            }
        }
        cx.notify();
    }

    fn remote_pick_completion(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(r) = &mut self.remote {
            r.form.completion = Some(index);
        }
        self.remote_confirm_completion(cx);
    }

    fn remote_confirm_completion(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.remote.as_mut().filter(|r| r.running.is_none()) else {
            return;
        };
        if let Some(i) = r.form.completion.take()
            && let Some(name) = r.form.completions(&mut self.ranker).get(i).cloned()
        {
            match r.form.field {
                Field::Input => r.form.input = name,
                _ => r.form.base = name,
            }
        }
        cx.notify();
    }
}

/// Where the reader was when Update ran, to land on the same place afterwards.
pub(super) struct KeepPlace {
    file: std::path::PathBuf,
    symbol: Option<String>,
    /// One-based.
    line: usize,
}

/// The remote header band's state. Lives only in a window on a cache worktree.
pub(super) struct HeaderState {
    pub(super) header: Header,
    /// Seconds since the Unix epoch when the relative times were last computed: at load and
    /// when the window is activated, never on a timer.
    now: u64,
    update: PrUpdate,
    /// The last pull request check could not reach the host. Shown as "offline" until a later
    /// check works; never saved.
    check_offline: bool,
    checking: bool,
    pub(super) updating: bool,
}

impl HeaderState {
    pub(super) fn new(header: Header) -> Self {
        Self {
            header,
            now: cho_sources::now_unix(),
            update: PrUpdate::Unknown,
            check_offline: false,
            checking: false,
            updating: false,
        }
    }
}

/// "0:14", "12:05", "1:02:09".
fn clock(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

impl Chorong {
    // ---- Progress in the status bar ----

    /// "Opening github:acme/api #12… 0:14" while an open runs with the form hidden.
    pub(super) fn open_progress(&self) -> Option<String> {
        let r = self.remote.as_ref().filter(|r| !r.visible)?;
        let run = r.running.as_ref()?;
        Some(format!(
            "{}\u{2026} {} \u{b7} {}",
            run.what,
            clock(run.started.elapsed().as_secs()),
            run.stage_text()
        ))
    }

    /// Shows the form again, with its progress.
    pub(super) fn show_remote_form(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = &mut self.remote {
            r.visible = true;
        }
        cx.notify();
    }

    // ---- The header band ----

    /// "GitHub" or "Bitbucket" in a window on a cache worktree, else `None`.
    pub(super) fn host_button(&self) -> Option<&'static str> {
        header::host_label(&self.remote_header.as_ref()?.header.repo)
    }

    /// Reads what the cache recorded about this window's worktree (background). A folder
    /// that is not a cache worktree gets no band.
    pub(super) fn load_remote_header(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { adapter::read_header(&root) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(h) = found {
                    this.remote_header = Some(HeaderState::new(h));
                    this.check_pr_head(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The window came to the front: refresh the relative times and look at the pull request
    /// again. Not polled.
    pub(super) fn remote_header_activated(&mut self, cx: &mut Context<Self>) {
        let Some(h) = &mut self.remote_header else {
            return;
        };
        h.now = cho_sources::now_unix();
        self.check_pr_head(cx);
        cx.notify();
    }

    /// Asks the host for the pull request's head (background; one check at a time) and
    /// decides whether to show "PR updated". Called on open and on window activation only.
    pub(super) fn check_pr_head(&mut self, cx: &mut Context<Self>) {
        let Some(h) = self.remote_header.as_mut().filter(|h| !h.checking) else {
            return;
        };
        let Some(number) = h.header.pull_request() else {
            return;
        };
        h.checking = true;
        let (repo, mine) = (h.header.repo.clone(), h.header.head.clone());
        cx.spawn(async move |this, cx| {
            let r = repo.clone();
            let host_head = cx
                .background_spawn(async move { adapter::pr_head(&r, number) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(h) = &mut this.remote_header
                    && h.header.repo == repo
                    && h.header.head == mine
                {
                    h.checking = false;
                    // A failed check says nothing about the banner (neither shown nor
                    // cleared); an unreachable host shows "offline" until a check works.
                    match host_head {
                        Ok(host_head) => {
                            h.check_offline = false;
                            h.update =
                                header::pr_update(&mine, host_head.as_ref().map(|p| p.id.as_str()));
                        }
                        Err(e) if e.offline => h.check_offline = true,
                        Err(_) => {}
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Update": opens the same pull request again through the Open Remote path and keeps
    /// the place by declaration (the file, the enclosing declaration, else the line).
    pub(super) fn update_pull_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.remote_header.as_mut().filter(|h| !h.updating) else {
            return;
        };
        let Some(number) = h.header.pull_request() else {
            return;
        };
        h.updating = true;
        let repo = h.header.repo.clone();
        let target = h.header.target.clone();
        let (mut path, mut line) = (None, None);
        let mut keep = None;
        if let (Some(doc), Some(root)) = (&self.ed().doc, &self.root) {
            let caret = self.ed().caret.unwrap_or(0);
            path = relative_to(root, &doc.path);
            line = Some(doc.buffer.line_of(caret) + 1);
            let symbol = enclosing(&doc.symbols, caret)
                .zip(doc.language)
                .map(|(sym, lang)| sym.qualified_name(lang))
                .filter(|q| {
                    doc.language.is_some_and(|l| {
                        matches!(find_symbol(l, doc.buffer.text(), q), SymbolMatch::Unique(_))
                    })
                });
            if let (Some(p), Some(l)) = (&path, line) {
                keep = Some(KeepPlace {
                    file: root.join(p),
                    symbol,
                    line: l,
                });
            }
        }
        // A comparison against something other than the target stays.
        let base = self
            .compare
            .as_ref()
            .filter(|c| Some(&c.against) != target.as_ref())
            .map(|c| c.against.clone());
        let req = adapter::Request {
            repo,
            clone_url: None,
            kind: cho_sources::TargetKind::PullRequest(number),
            path,
            line,
            base,
        };
        let handle = window.window_handle();
        let wanted = self.prepare_wanted;
        self.notify_text(format!("Updating pull request #{number}\u{2026}"), cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    adapter::open(&req, &|_| {}, &adapter::Cancel::default())
                })
                .await;
            this.update(cx, |this, cx| {
                this.remote_finished(result, handle, true, keep, wanted, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Open on GitHub / Bitbucket: the current file and line on the side being read (Before
    /// is the comparison's base commit, After the head), in the browser.
    pub(super) fn open_on_host(&mut self, cx: &mut Context<Self>) {
        let Some(h) = &self.remote_header else { return };
        let Some(label) = header::host_label(&h.header.repo) else {
            return;
        };
        let (Some(doc), Some(root)) = (&self.ed().doc, &self.root) else {
            self.notify_text("Open a file first", cx);
            return;
        };
        let Some(rel) = relative_to(root, &doc.path) else {
            self.notify_text("This file is not in the repository", cx);
            return;
        };
        // The line is the one shown on this side: no mapping through the diff.
        let line = self.selected_line().map(|l| l + 1);
        let before = self.ed().side == Side::Before;
        let base = self.compare.as_ref().map(|c| c.base.as_str());
        match header::web_link(&h.header.repo, before, &h.header.head, base, &rel, line) {
            Some(url) => {
                cx.open_url(&url);
                self.notify_text(format!("Opening on {label}: {url}"), cx);
            }
            None => self.notify_text(format!("Cannot make a {label} link for this file"), cx),
        }
    }

    pub(super) fn render_remote_header(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let hs = self.remote_header.as_ref()?;
        let t = &self.theme;
        let mut shown = hs.header.clone();
        shown.offline |= hs.check_offline;
        let text = shown.text(hs.now);
        let muted = hsla(t.text_muted);
        let warn = hsla(t.ui.warning.unwrap_or(t.ui.text_accent));
        let chip = |state: PrState| {
            let col = match state {
                PrState::Open => hsla(t.ui.success.unwrap_or(t.ui.text_accent)),
                PrState::Merged => hsla(t.ui.text_accent),
                PrState::Declined => muted,
            };
            div()
                .flex_none()
                .px_1p5()
                .rounded_sm()
                .border_1()
                .border_color(col)
                .text_color(col)
                .child(state.label())
        };
        let tip_title = match &hs.header.kind {
            Kind::PullRequest { .. } => format!(
                "{} \u{b7} {}",
                text.title.clone().unwrap_or_default(),
                hs.header.repo
            ),
            _ => format!("{} \u{b7} {}", hs.header.repo, hs.header.head),
        };
        let mut band = div()
            .id("remote-header")
            .flex_none()
            .h(px(26.))
            .px_3()
            .gap_2()
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(12.))
            .bg(hsla(t.status_bar_background))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(text.lead.clone()),
            );
        if let Some(title) = text.title.clone() {
            band = band.child(
                div()
                    .id("remote-header-title")
                    .min_w_0()
                    .truncate()
                    .child(title)
                    .tooltip(tip(tip_title)),
            );
        }
        if let Some(a) = text.author.clone() {
            band = band.child(div().flex_none().text_color(muted).child(format!("by {a}")));
        }
        if let Some(s) = text.state {
            band = band.child(chip(s));
        }
        if let PrUpdate::Newer(short) = &hs.update {
            let accent = hsla(t.ui.text_accent);
            band = band.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .rounded_sm()
                    .bg(hsla(t.element_selected))
                    .child(format!("PR updated: new head {short} \u{b7}"))
                    .child(
                        div()
                            .id("remote-pr-update")
                            .cursor_pointer()
                            .text_color(accent)
                            .font_weight(FontWeight::MEDIUM)
                            .child(if hs.updating {
                                "Updating\u{2026}"
                            } else {
                                "Update"
                            })
                            .tooltip(tip(
                                "Open the new head and keep your place by declaration".into()
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.update_pull_request(window, cx);
                            })),
                    ),
            );
        }
        band = band.child(div().flex_1());
        if let Some(b) = text.behind.clone() {
            band = band.child(
                div()
                    .id("remote-header-behind")
                    .flex_none()
                    .text_color(warn)
                    .child(b)
                    .tooltip(tip(crate::remote::header::BEHIND_TIP.to_string())),
            );
        }
        if let Some(age) = text.stale.clone() {
            band = band.child(
                div()
                    .id("remote-header-details-age")
                    .flex_none()
                    .text_color(muted)
                    .child(age)
                    .tooltip(tip(
                        "The title, author and state were saved earlier; open the pull \
                         request again to read them from the host"
                            .into(),
                    )),
            );
        }
        if let Some(f) = text.freshness.clone() {
            band = band.child(
                div()
                    .flex_none()
                    .text_color(if shown.offline { warn } else { muted })
                    .child(f),
            );
        }
        Some(band.into_any_element())
    }
}

/// `path` relative to `root` with `/` separators, trying the canonical root too.
fn relative_to(root: &std::path::Path, path: &std::path::Path) -> Option<String> {
    let rel = path
        .strip_prefix(root)
        .map(std::path::Path::to_path_buf)
        .ok()
        .or_else(|| {
            let canon = root.canonicalize().ok()?;
            path.strip_prefix(canon)
                .ok()
                .map(std::path::Path::to_path_buf)
        })?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

fn row_repo(row: &ProjectRow) -> Option<RepoRef> {
    match row {
        ProjectRow::Known(r) | ProjectRow::Add(r, _) => Some(r.clone()),
        ProjectRow::AddFromUrl => None,
    }
}

/// "Opening github:acme/api #123".
fn describe(req: &crate::remote::adapter::Request) -> String {
    use cho_sources::TargetKind::*;
    let what = match &req.kind {
        Default => String::new(),
        Branch(b) => format!(" {b}"),
        PullRequest(n) => format!(" #{n}"),
        Commit(c) | BranchOrCommit(c) => format!(" {}", cho_git::short(c)),
        RefPath(p) => format!(" {p}"),
    };
    format!("Opening {}{what}", req.repo)
}

/// Where a failed open's message goes.
#[derive(Debug, PartialEq, Eq)]
enum FailureRoute {
    /// Into the form, shown (with its buttons or command).
    Form,
    /// Into the status bar, or a banner when it has a command to copy.
    Notice,
    /// A cancel: nowhere.
    Silent,
}

/// `visible`: the form is on screen; `has_form`: there is one (Update runs without);
/// `shown`: not a cancel; `offers`: choices or a command that need the form.
fn failure_route(visible: bool, has_form: bool, shown: bool, offers: bool) -> FailureRoute {
    if has_form && (visible || offers) {
        FailureRoute::Form
    } else if shown {
        FailureRoute::Notice
    } else {
        FailureRoute::Silent
    }
}

/// The window an open that worked starts: the worktree, the file if one was named, and the
/// comparison to start.
fn startup_for(done: Done, prepare: bool, link: Option<LinkInfo>) -> Startup {
    Startup {
        symbol: link.as_ref().and_then(|l| l.symbol.clone()),
        file: done.path.as_ref().map(|p| done.worktree.join(p)),
        line: done.line,
        root: Some(done.worktree),
        origin: Origin::Remote(Box::new(RemoteStart {
            before: done.before,
            against: done.against,
            merge_bases: done.merge_bases,
            notes: done.notes,
            header: done.header,
            prepare,
            link,
        })),
        extra_files: Vec::new(),
        name: None,
        compare: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Update runs without a form: a failure that offers a command used to be lost.
    #[test]
    fn a_failure_with_no_form_still_reaches_the_reader() {
        use FailureRoute::*;
        assert_eq!(failure_route(false, false, true, true), Notice);
        assert_eq!(failure_route(false, false, true, false), Notice);
        assert_eq!(failure_route(false, false, false, false), Silent);
        assert_eq!(failure_route(true, true, true, false), Form);
        assert_eq!(failure_route(false, true, true, true), Form);
        assert_eq!(failure_route(false, true, true, false), Notice);
        assert_eq!(failure_route(false, true, false, false), Silent);
    }

    #[test]
    fn the_progress_clock_reads_minutes_and_seconds() {
        assert_eq!(clock(14), "0:14");
        assert_eq!(clock(725), "12:05");
        assert_eq!(clock(3729), "1:02:09");
    }

    #[test]
    fn paths_are_made_relative_to_the_worktree() {
        let root = std::path::Path::new("/cache/wt");
        assert_eq!(
            relative_to(root, std::path::Path::new("/cache/wt/src/a.rs")).as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(relative_to(root, std::path::Path::new("/else/a.rs")), None);
    }

    #[test]
    fn the_new_window_gets_the_worktree_file_and_comparison() {
        let s = startup_for(
            Done {
                repo: "github:acme/api".into(),
                worktree: "/cache/wt".into(),
                before: "abc".into(),
                against: "main".into(),
                merge_bases: 1,
                path: Some("src/lib.rs".into()),
                line: Some(12),
                notes: vec!["Added github:acme/api to your projects.".into()],
                header: Header {
                    repo: RepoRef::parse("github:acme/api").unwrap(),
                    kind: Kind::Branch("main".into()),
                    head: "abc".into(),
                    target: Some("main".into()),
                    behind: Some(0),
                    fetched_at: None,
                    offline: false,
                },
            },
            true,
            None,
        );
        let Origin::Remote(r) = &s.origin else {
            panic!("a remote open starts a remote window")
        };
        assert!(r.prepare);
        assert_eq!(s.root.as_deref(), Some(std::path::Path::new("/cache/wt")));
        assert_eq!(
            s.file.as_deref(),
            Some(std::path::Path::new("/cache/wt/src/lib.rs"))
        );
        assert_eq!(s.line, Some(12));
        assert_eq!((r.before.as_str(), r.against.as_str()), ("abc", "main"));
        assert_eq!(r.notes.len(), 1);
    }
}

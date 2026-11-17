//! The Projects, Cache and Accounts sections: drawing, keys, and the background work.
//!
//! Every read and write goes through `crate::remote::adapter` on the background executor, so
//! the window stays responsive. The words and the state machines are in `remote_model`
//! (tested without a window). Nothing here runs unless a section is shown or a button is
//! pressed: Cache sizes are measured when the section opens, a token is checked only when
//! "Check" is pressed.

use super::*;
use crate::remote::adapter::{self, AccountInfo, ProjectInfo};
use crate::settings_window::remote_model::{
    CheckState, Confirm, ConfirmState, Editing, FormFocus, Load, Notice, Tab, TokenForm, ago_text,
    approval_text, check_text, email_text, env_instruction, keychain_problem, parse_cap,
    quota_text, size_text, sized_all, sized_repo, sized_worktree, token_text, worktree_label,
};
use cho_sources::{Host, Protocol};
use gpui::{ElementId, Stateful};

impl SettingsWindow {
    // ---- Showing a section ----

    /// Switches section. Each one reads what it shows again, in the background.
    pub(super) fn show_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.remote.editing = Editing::None;
        self.remote.notice = None;
        match tab {
            Tab::Settings => {}
            Tab::Projects => self.load_projects(cx),
            Tab::Cache => self.measure_cache(cx),
            Tab::Accounts => self.load_accounts(cx),
        }
        cx.notify();
    }

    fn load_projects(&mut self, cx: &mut Context<Self>) {
        self.remote.projects = Load::Loading;
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_spawn(async { adapter::project_infos() })
                .await;
            this.update(cx, |this, cx| {
                this.remote.projects = Load::from_result(r);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Measures the cache (walks its files) on a background thread.
    fn measure_cache(&mut self, cx: &mut Context<Self>) {
        self.remote.usage = Load::Loading;
        cx.spawn(async move |this, cx| {
            let r = cx.background_spawn(async { adapter::cache_usage() }).await;
            this.update(cx, |this, cx| {
                this.remote.usage = Load::from_result(r);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_accounts(&mut self, cx: &mut Context<Self>) {
        self.remote.accounts = Load::Loading;
        cx.spawn(async move |this, cx| {
            let a = cx.background_spawn(async { adapter::accounts() }).await;
            this.update(cx, |this, cx| {
                this.remote.accounts = Load::Ready(a);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_current(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Settings => {}
            Tab::Projects => self.load_projects(cx),
            Tab::Cache => self.measure_cache(cx),
            Tab::Accounts => self.load_accounts(cx),
        }
    }

    // ---- Projects ----

    fn project(&self, repo: &str) -> Option<ProjectInfo> {
        self.remote
            .projects
            .ready()?
            .iter()
            .find(|p| p.repo == repo)
            .cloned()
    }

    /// Saves protocol, cap and auto-prepare of one project (`Projects::insert`, atomic).
    fn save_project(
        &mut self,
        repo: String,
        protocol: Protocol,
        cap: usize,
        cx: &mut Context<Self>,
    ) {
        self.remote.notice = None;
        cx.spawn(async move |this, cx| {
            let name = repo.clone();
            let r = cx
                .background_spawn(async move { adapter::save_project(&repo, protocol, cap) })
                .await;
            this.update(cx, |this, cx| {
                if let Err(e) = r {
                    this.remote.notice = Some(Notice::Error(format!("Could not save {name}: {e}")));
                }
                this.load_projects(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_protocol(&mut self, repo: &str, protocol: Protocol, cx: &mut Context<Self>) {
        if let Some(p) = self.project(repo).filter(|p| p.protocol != protocol) {
            self.save_project(p.repo, protocol, p.cap, cx);
        }
    }

    fn commit_cap(&mut self, cx: &mut Context<Self>) {
        let Editing::Cap { repo, text } = &self.remote.editing else {
            return;
        };
        match parse_cap(text) {
            Ok(n) => {
                let repo = repo.clone();
                self.remote.editing = Editing::None;
                if let Some(p) = self.project(&repo) {
                    self.save_project(p.repo, p.protocol, n, cx);
                }
            }
            Err(e) => self.remote.notice = Some(Notice::Error(e)),
        }
        cx.notify();
    }

    // ---- Confirmations and cleanup ----

    /// Opens a confirmation naming what goes. The size comes from the measured cache, or for
    /// a project, from a measurement of just that repository.
    fn ask(&mut self, what: Confirm, cx: &mut Context<Self>) {
        self.remote.notice = None;
        let usage = self.remote.usage.ready();
        let size = match &what {
            Confirm::RemoveProject(_) => None,
            Confirm::ClearWorktree { repo, slug } => usage.and_then(|u| {
                let r = u.repos.iter().find(|r| &r.repo == repo)?;
                r.worktrees
                    .iter()
                    .find(|w| &w.slug == slug)
                    .map(sized_worktree)
            }),
            Confirm::ClearRepo(repo) => usage
                .and_then(|u| u.repos.iter().find(|r| &r.repo == repo))
                .map(sized_repo),
            Confirm::ClearAll => usage.map(sized_all),
            Confirm::RemoveToken(_) => None,
        };
        let project = match &what {
            Confirm::RemoveProject(r) => Some(r.clone()),
            _ => None,
        };
        self.remote.editing = Editing::Confirm(ConfirmState { what, size });
        if let Some(repo) = project {
            cx.spawn(async move |this, cx| {
                let name = repo.clone();
                let r = cx
                    .background_spawn(async move { adapter::repo_size(&repo) })
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(c) = this
                        .remote
                        .editing
                        .confirm_mut()
                        .filter(|c| matches!(&c.what, Confirm::RemoveProject(r) if *r == name))
                    {
                        // An unreadable cache has nothing to delete that can be sized.
                        c.size = Some(r.as_ref().map(sized_repo).unwrap_or(
                            crate::settings_window::remote_model::Sized {
                                bytes: 0,
                                worktrees: 0,
                            },
                        ));
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn confirm_no(&mut self, cx: &mut Context<Self>) {
        self.remote.editing.take_confirm();
        cx.notify();
    }

    fn confirm_yes(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self
            .remote
            .editing
            .take_confirm()
            .filter(ConfirmState::can_confirm)
        else {
            return;
        };
        self.remote.busy = Some("Working\u{2026}".into());
        let what = c.what;
        cx.spawn(async move |this, cx| {
            let w = what.clone();
            let done: Result<String, String> = cx
                .background_spawn(async move {
                    match w {
                        Confirm::RemoveProject(r) => adapter::remove_project(&r)
                            .map(|n| format!("Removed {r} and {n} cached worktrees")),
                        Confirm::ClearWorktree { repo, slug } => {
                            adapter::clear_cache(Some(&repo), Some(&slug))
                                .map(|_| format!("Removed {slug} of {repo}"))
                        }
                        Confirm::ClearRepo(r) => adapter::clear_cache(Some(&r), None)
                            .map(|_| format!("Cleared the cache of {r}")),
                        Confirm::ClearAll => {
                            adapter::clear_cache(None, None).map(|_| "Cleared the cache".into())
                        }
                        Confirm::RemoveToken(h) => adapter::remove_token(&h).map(|had| {
                            if had {
                                format!("Removed the token for {h}")
                            } else {
                                format!("There was no keychain token for {h}")
                            }
                        }),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.remote.busy = None;
                this.remote.notice = Some(match done {
                    Ok(t) => Notice::Done(t),
                    Err(e) => Notice::Error(e),
                });
                // A cleanup invalidates what was measured; read the section again.
                this.refresh_current(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ---- Accounts ----

    fn start_check(&mut self, host: String, cx: &mut Context<Self>) {
        self.remote
            .checks
            .insert(host.clone(), CheckState::Checking);
        cx.spawn(async move |this, cx| {
            let h = host.clone();
            let r = cx
                .background_spawn(async move { adapter::check_account(&h) })
                .await;
            this.update(cx, |this, cx| {
                this.remote.checks.insert(host, CheckState::Done(r));
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_form(&mut self, host: &Host, cx: &mut Context<Self>) {
        self.remote.notice = None;
        self.remote.editing = Editing::Token(TokenForm::new(host));
        cx.notify();
    }

    /// Enter in the form: the token leaves the field and goes to the keychain on a background
    /// thread; the copy there is zeroed as soon as the call returns.
    fn submit_form(&mut self, cx: &mut Context<Self>) {
        let Some(sub) = self.remote.editing.token_mut().and_then(TokenForm::submit) else {
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let host = sub.host.clone();
            let r = cx
                .background_spawn(async move {
                    let r = adapter::add_token(&sub.host, sub.token.as_str(), sub.email.as_deref());
                    drop(sub); // zeroes the token
                    r
                })
                .await;
            this.update(cx, |this, cx| {
                match r {
                    Ok(()) => {
                        this.remote.editing = Editing::None;
                        this.remote.notice = Some(Notice::Done(format!(
                            "Stored the token for {host} in the keychain"
                        )));
                        this.load_accounts(cx);
                    }
                    Err(e) => {
                        if let Some(f) = this.remote.editing.token_mut() {
                            f.busy = false;
                            f.error = Some(e);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ---- Keys ----

    /// A key in one of the three sections (the Settings page has its own handling).
    pub(super) fn remote_key(
        &mut self,
        event: &KeyDownEvent,
        paste: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;
        let key = k.key.as_str();
        let remote = &mut self.remote;
        match &mut remote.editing {
            Editing::Token(f) => match key {
                "escape" => remote.editing = Editing::None,
                "enter" => return self.submit_form(cx),
                "tab" => {
                    f.next_focus();
                    remote.field_all = false;
                }
                _ => f.field(k, paste.as_deref(), &mut remote.field_all),
            },
            Editing::Cap { text: t, .. } => match key {
                "escape" => remote.editing = Editing::None,
                "enter" => return self.commit_cap(cx),
                _ => {
                    let edit = cho_layout_ui::field::edit_field(
                        k,
                        t,
                        &mut remote.field_all,
                        paste.as_deref(),
                    );
                    if edit == cho_layout_ui::field::FieldEdit::Text {
                        t.retain(|c| c.is_ascii_digit());
                    }
                }
            },
            Editing::Confirm(_) => match key {
                "escape" => remote.editing = Editing::None,
                "enter" => return self.confirm_yes(cx),
                _ => {}
            },
            Editing::None => {
                if key == "escape" {
                    window.remove_window();
                    return;
                }
            }
        }
        cx.notify();
    }

    /// Whether a field on this tab has the keys (a cap or a token form being typed in).
    pub(super) fn remote_typing(&self) -> bool {
        matches!(self.remote.editing, Editing::Token(_) | Editing::Cap { .. })
    }

    // ---- Drawing: pieces ----

    fn button(
        &self,
        id: impl Into<ElementId>,
        label: &str,
        enabled: bool,
        primary: bool,
    ) -> Stateful<gpui::Div> {
        let (t, ui) = (&self.theme, &self.theme.ui);
        div()
            .id(id)
            .flex_none()
            .px_3()
            .py(px(2.))
            .rounded_sm()
            .border_1()
            .border_color(if primary && enabled {
                hsla(ui.border_focused)
            } else {
                hsla(t.border)
            })
            .text_size(px(12.5))
            .text_color(hsla(if enabled { t.text } else { ui.text_disabled }))
            .when(primary && enabled, |d| d.bg(hsla(t.element_selected)))
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(hsla(ui.ghost_element_hover)))
            })
            .child(label.to_string())
    }

    fn segment(&self, id: impl Into<ElementId>, label: &str, on: bool) -> Stateful<gpui::Div> {
        let (t, ui) = (&self.theme, &self.theme.ui);
        div()
            .id(id)
            .px_3()
            .py(px(2.))
            .text_size(px(12.5))
            .cursor_pointer()
            .text_color(hsla(if on { t.text } else { t.text_muted }))
            .when(on, |d| d.bg(hsla(t.element_selected)))
            .hover(|s| s.bg(hsla(ui.ghost_element_hover)))
            .child(label.to_string())
    }

    fn field_box(
        &self,
        id: impl Into<ElementId>,
        text: String,
        focused: bool,
        width: f32,
    ) -> Stateful<gpui::Div> {
        let (t, ui) = (&self.theme, &self.theme.ui);
        div()
            .id(id)
            .flex_none()
            .w(px(width))
            .px_2()
            .py(px(2.))
            .rounded_sm()
            .border_1()
            .border_color(if focused {
                hsla(ui.border_focused)
            } else {
                hsla(t.border)
            })
            .bg(hsla(t.editor_background))
            .text_size(px(13.))
            .text_color(hsla(t.text))
            .overflow_hidden()
            .whitespace_nowrap()
            .cursor_pointer()
            .when(focused && self.remote.field_all, |d| {
                // Cmd-A: the whole text selected (typing replaces it).
                d.bg(hsla(t.element_selected))
            })
            .child(if focused && !self.remote.field_all {
                format!("{text}\u{258f}")
            } else {
                text
            })
    }

    fn card(&self) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .rounded_lg()
            .border_1()
            .border_color(hsla(self.theme.ui.border_variant))
    }

    fn muted(&self, text: impl Into<SharedString>) -> gpui::Div {
        div()
            .text_size(px(12.))
            .text_color(hsla(self.theme.text_muted))
            .child(text.into())
    }

    fn remote_notice(&self) -> Option<gpui::Div> {
        Some(match self.remote.notice.clone()? {
            Notice::Error(text) => self.notice(true, text),
            Notice::Done(text) => self.good_notice(text),
        })
    }

    /// A finished action: the theme's success colour, not the yellow of a warning.
    fn good_notice(&self, text: String) -> gpui::Div {
        let (t, ui) = (&self.theme, &self.theme.ui);
        let base = ui.success.unwrap_or(t.text_muted);
        div()
            .flex()
            .gap_2()
            .items_start()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .text_size(px(12.))
            .bg(ui.success_background.map_or_else(|| fade(base, 0.10), hsla))
            .border_color(ui.success_border.map_or_else(|| fade(base, 0.45), hsla))
            .child(div().flex_none().text_color(hsla(base)).child("\u{2713}"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(hsla(t.text))
                    .child(text),
            )
    }

    fn confirm_bar(&self, c: &ConfirmState, cx: &mut Context<Self>) -> gpui::Div {
        let (t, ui) = (&self.theme, &self.theme.ui);
        let text = c.text();
        let ok = c.can_confirm();
        self.card()
            .border_color(hsla(ui.border_focused))
            .bg(hsla(t.panel_background))
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(hsla(t.text))
                    .child(text.question),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(hsla(t.text))
                    .child(text.detail),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        self.button("confirm-yes", text.button, ok, true)
                            .when(ok, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.confirm_yes(cx)))
                            }),
                    )
                    .child(
                        self.button("confirm-no", "Cancel", true, false)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_no(cx))),
                    ),
            )
    }

    fn loading_or_failed<T>(&self, l: &Load<T>, what: &str) -> Option<gpui::Div> {
        match l {
            Load::Idle | Load::Loading => Some(self.muted(format!("{what}\u{2026}"))),
            Load::Failed(e) => Some(self.notice(true, e.clone())),
            Load::Ready(_) => None,
        }
    }

    // ---- Drawing: the sections ----

    /// The body of Projects, Cache or Accounts.
    pub(super) fn remote_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div().flex().flex_col().gap_3().p_4();
        if let Some(b) = &self.remote.busy {
            col = col.child(self.muted(b.clone()));
        }
        if let Some(n) = self.remote_notice() {
            col = col.child(n);
        }
        if let Some(c) = self.remote.editing.confirm() {
            col = col.child(self.confirm_bar(c, cx));
        }
        col = match self.tab {
            Tab::Settings => col,
            Tab::Projects => self.projects_section(col, cx),
            Tab::Cache => self.cache_section(col, cx),
            Tab::Accounts => self.accounts_section(col, cx),
        };
        self.scroll_areas
            .get("remote-body")
            .vertical("remote-body", self.stretch(), div().child(col))
            .flex_1()
            .min_h_0()
            .into_any_element()
    }

    fn projects_section(&self, mut col: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        let t = &self.theme;
        col = col.child(self.muted(
            "Remote repositories Chorong remembers. Changes are saved at once to projects.json.",
        ));
        if let Some(m) = self.loading_or_failed(&self.remote.projects, "Reading projects") {
            return col.child(m);
        }
        let projects = self.remote.projects.ready().cloned().unwrap_or_default();
        if projects.is_empty() {
            return col.child(self.muted("No projects yet. Open Remote adds one."));
        }
        let mono = cx.global::<cho_fonts::Fonts>().mono.clone();
        for (i, p) in projects.iter().enumerate() {
            let repo = p.repo.clone();
            let editing = self.remote.editing.cap_for(&p.repo).map(str::to_string);
            let cap_text = editing.clone().unwrap_or_else(|| p.cap.to_string());
            let r1 = repo.clone();
            let r2 = repo.clone();
            let r3 = repo.clone();
            let r5 = repo.clone();
            let cap = p.cap;
            col = col.child(
                self.card()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_family(mono.clone())
                                    .text_size(px(13.5))
                                    .text_color(hsla(t.text))
                                    .child(repo.clone()),
                            )
                            .child(
                                self.button(
                                    ("remove-project", i),
                                    "Remove project\u{2026}",
                                    true,
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.ask(Confirm::RemoveProject(r5.clone()), cx);
                                    },
                                )),
                            ),
                    )
                    .when_some(p.url.clone(), |d, u| {
                        d.child(
                            self.muted(format!("Clone URL: {u} (used instead of the protocol)")),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap_x_6()
                            .gap_y_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(self.muted("Clone with"))
                                    .child(
                                        div()
                                            .flex()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(hsla(t.border))
                                            .overflow_hidden()
                                            .child(
                                                self.segment(
                                                    ("https", i),
                                                    "HTTPS",
                                                    p.protocol == Protocol::Https,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.set_protocol(&r1, Protocol::Https, cx);
                                                })),
                                            )
                                            .child(
                                                self.segment(
                                                    ("ssh", i),
                                                    "SSH",
                                                    p.protocol == Protocol::Ssh,
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.set_protocol(&r2, Protocol::Ssh, cx);
                                                })),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(self.muted("Worktrees kept (1\u{2013}50)"))
                                    .child(
                                        self.field_box(
                                            ("cap", i),
                                            cap_text,
                                            editing.is_some(),
                                            56.,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.remote.editing = Editing::Cap {
                                                    repo: r3.clone(),
                                                    text: cap.to_string(),
                                                };
                                                this.remote.field_all = false;
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(self.muted(format!("Prepare: {}", approval_text(&p.approval)))),
                    ),
            );
        }
        col
    }

    fn cache_section(&self, mut col: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        let t = &self.theme;
        let mono = cx.global::<cho_fonts::Fonts>().mono.clone();
        if let Some(m) = self.loading_or_failed(&self.remote.usage, "Measuring") {
            return col.child(m);
        }
        let Some(u) = self.remote.usage.ready() else {
            return col;
        };
        let root = u.root.clone();
        let total = sized_all(u);
        col =
            col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(14.)).text_color(hsla(t.text)).child(
                                format!(
                                    "{} in the cache ({} repositor{}, {} worktrees)",
                                    size_text(total.bytes),
                                    u.repos.len(),
                                    if u.repos.len() == 1 { "y" } else { "ies" },
                                    total.worktrees
                                ),
                            ))
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .text_color(hsla(t.text_muted))
                                    .child(root.display().to_string()),
                            ),
                    )
                    .child(
                        self.button("reveal", "Reveal", true, false)
                            .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&root))),
                    )
                    .child(
                        self.button("clear-all", "Clear all\u{2026}", !u.repos.is_empty(), false)
                            .when(!u.repos.is_empty(), |d| {
                                d.on_click(
                                    cx.listener(|this, _, _, cx| this.ask(Confirm::ClearAll, cx)),
                                )
                            }),
                    ),
            );
        col = col.child(self.muted(
            "Sizes were measured when you opened this page. Clearing also removes the indexes \
             of what goes. Your own clones are never touched.",
        ));
        if u.repos.is_empty() {
            return col.child(self.muted("The cache is empty."));
        }
        let now = cho_sources::now_unix();
        for (i, r) in u.repos.iter().enumerate() {
            let repo = r.repo.clone();
            let mut card = self.card().child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(mono.clone())
                            .text_size(px(13.5))
                            .text_color(hsla(t.text))
                            .child(r.repo.clone()),
                    )
                    .child(self.muted(format!("{} in total", size_text(r.total_bytes))))
                    .child(
                        self.button(("clear-repo", i), "Clear\u{2026}", true, false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask(Confirm::ClearRepo(repo.clone()), cx);
                            })),
                    ),
            );
            card = card.child(self.muted(format!(
                "Clone (shared by the worktrees): {}",
                size_text(r.clone_bytes)
            )));
            for (j, w) in r.worktrees.iter().enumerate() {
                let (repo, slug) = (r.repo.clone(), w.slug.clone());
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(13.))
                                .text_color(hsla(t.text))
                                .child(worktree_label(w)),
                        )
                        .child(self.muted(size_text(w.bytes)))
                        .child(self.muted(format!("opened {}", ago_text(w.opened, now))))
                        .child(
                            self.button(("rm-wt", i * 1000 + j), "Remove\u{2026}", true, false)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ask(
                                        Confirm::ClearWorktree {
                                            repo: repo.clone(),
                                            slug: slug.clone(),
                                        },
                                        cx,
                                    );
                                })),
                        ),
                );
            }
            col = col.child(card);
        }
        col
    }

    fn accounts_section(&self, mut col: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        col = col.child(self.muted(
            "Tokens are kept in your system keychain only; Chorong never shows or saves them \
             anywhere else. A token in the environment works too.",
        ));
        if let Some(m) = self.loading_or_failed(&self.remote.accounts, "Reading accounts") {
            return col.child(m);
        }
        let Some(accounts) = self.remote.accounts.ready() else {
            return col;
        };
        for (i, a) in accounts.hosts.iter().enumerate() {
            col = col.child(self.account_card(i, a, keychain_problem(accounts, a), cx));
        }
        col
    }

    fn account_card(
        &self,
        i: usize,
        a: &AccountInfo,
        problem: Option<String>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let t = &self.theme;
        let host = a.host.name().to_string();
        let has_token = a.status.source.is_some();
        let from_keychain = a.status.source == Some(cho_sources::TokenSource::Keychain);
        let check = self.remote.checks.get(&host);
        let (h1, h2) = (host.clone(), host.clone());
        let host_for_form = a.host.clone();
        let mut card = self.card().child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(14.))
                        .text_color(hsla(t.text))
                        .child(host.clone()),
                )
                .child(self.muted(token_text(a)))
                .when_some(email_text(a), |d, e| d.child(self.muted(e)))
                .child(
                    self.button(("check", i), "Check", has_token, false)
                        .when(has_token, |d| {
                            d.on_click(
                                cx.listener(move |this, _, _, cx| this.start_check(h1.clone(), cx)),
                            )
                        }),
                ),
        );
        let line = match check {
            Some(CheckState::Checking) => Some("Checking\u{2026}".to_string()),
            Some(CheckState::Done(r)) => {
                let mut s = check_text(r);
                if let Some(q) = r.as_ref().ok().and_then(|c| quota_text(c.rate)) {
                    s.push_str(&format!(" \u{b7} {q}"));
                }
                Some(s)
            }
            None => has_token.then(|| "not checked yet".to_string()),
        };
        if let Some(l) = line {
            card = card.child(self.muted(format!("Status: {l}")));
        }
        if let Some(p) = &problem {
            // No keychain: say so, and where the token goes instead. No form.
            return card.child(self.notice(false, format!("{p}\n{}", env_instruction(&a.host))));
        }
        if let Some(f) = self.remote.editing.token().filter(|f| f.host == host) {
            return card.child(self.token_form(f, cx));
        }
        card.child(
            div()
                .flex()
                .gap_2()
                .child(
                    self.button(("add-token", i), "Add token\u{2026}", true, false)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_form(&host_for_form, cx)),
                        ),
                )
                .when(from_keychain, |d| {
                    d.child(
                        self.button(("rm-token", i), "Remove token\u{2026}", true, false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask(Confirm::RemoveToken(h2.clone()), cx);
                            })),
                    )
                }),
        )
    }

    /// The inline form: the token is drawn as bullets, never as text.
    fn token_form(&self, f: &TokenForm, cx: &mut Context<Self>) -> gpui::Div {
        let t = &self.theme;
        let on_token = f.focus == FormFocus::Token;
        let busy = f.busy;
        let mut col = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(60.))
                        .text_size(px(12.5))
                        .text_color(hsla(t.text_muted))
                        .child("Token"),
                )
                .child(
                    self.field_box("token-field", f.token.masked(), on_token && !busy, 360.)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(f) = this.remote.editing.token_mut() {
                                f.focus = FormFocus::Token;
                            }
                            cx.notify();
                        })),
                ),
        );
        if f.needs_email {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(60.))
                            .text_size(px(12.5))
                            .text_color(hsla(t.text_muted))
                            .child("Email"),
                    )
                    .child(
                        self.field_box("email-field", f.email.clone(), !on_token && !busy, 360.)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(f) = this.remote.editing.token_mut() {
                                    f.focus = FormFocus::Email;
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        if let Some(e) = &f.error {
            col = col.child(self.notice(true, e.clone()));
        }
        col.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    self.button(
                        "form-save",
                        if busy { "Saving\u{2026}" } else { "Save token" },
                        !busy,
                        true,
                    )
                    .when(!busy, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| this.submit_form(cx)))
                    }),
                )
                .child(
                    self.button("form-cancel", "Cancel", !busy, false)
                        .when(!busy, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| {
                                this.remote.editing = Editing::None;
                                cx.notify();
                            }))
                        }),
                )
                .child(self.muted(
                    "Stored in the keychain only. Paste with Ctrl/Cmd-V; Tab switches field.",
                )),
        )
    }

    /// The footer's key hints for a section.
    pub(super) fn remote_footer_hint(&self) -> &'static str {
        match self.remote.editing {
            Editing::Token(_) => "Enter saves \u{b7} Tab next field \u{b7} Esc cancels",
            Editing::Cap { .. } => "Enter saves \u{b7} Esc cancels",
            Editing::Confirm(_) => "Enter confirms \u{b7} Esc cancels",
            Editing::None => "Ctrl/Cmd-1 to 4 switch sections \u{b7} Esc closes",
        }
    }
}

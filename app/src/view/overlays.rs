//! Full-window layers: the `?` cheat sheet, the trust prompt, the index error popover and the
//! Cmd-K chord popup. Every key shown comes from the live keymap (`Chorong::key_labels`).

use super::*;

impl Chorong {
    pub(super) fn layer(&self, children: impl IntoElement, cx: &mut Context<Self>) -> gpui::Div {
        // A click outside the card dismisses it. Modal: the panes behind take no hover or
        // scroll (a scrolling card, such as the cheat sheet, scrolls itself).
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(70.))
            .bg(gpui::hsla(0., 0., 0., 0.25))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dialog_decline(cx);
                    cx.notify();
                }),
            )
            .child(children)
    }

    pub(super) fn card(&self, width: f32) -> gpui::Div {
        let t = &self.theme;
        div()
            .w(px(width))
            .flex()
            .flex_col()
            .bg(hsla(t.panel_background))
            .border_1()
            .border_color(hsla(t.border))
            .rounded_md()
            .shadow_lg()
            .text_size(px(13.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }

    /// A key shown inside a button, after its label (`Show graph  Enter`).
    pub(super) fn button_key(&self, label: String) -> gpui::Div {
        let t = &self.theme;
        div()
            .flex_none()
            .whitespace_nowrap()
            .px_1()
            .rounded_sm()
            .border_1()
            .border_color(hsla(t.border))
            .font_family(self.font.clone())
            .font_weight(FontWeight::NORMAL)
            .text_size(px(11.))
            .text_color(hsla(t.text_muted))
            .child(label)
    }

    /// The key hint for a dialog's accept or decline button, when the key is bound.
    pub(super) fn dialog_key(&self, accept: bool) -> Option<gpui::Div> {
        let action = if accept {
            "chorong::TrustAccept"
        } else {
            "chorong::TrustDecline"
        };
        let k = self.overlay_keys.get(action)?;
        Some(self.button_key(k.clone()))
    }

    pub(super) fn key_cap(&self, label: String) -> gpui::Div {
        let t = &self.theme;
        div()
            .flex_none()
            .whitespace_nowrap()
            .px_2()
            .rounded_sm()
            .border_1()
            .border_color(hsla(t.border))
            .bg(hsla(t.element_selected))
            .font_family(self.font.clone())
            .text_size(px(12.))
            .text_color(hsla(t.text))
            .child(label)
    }

    /// The cheat sheet: every bound command, in the three layers of the scheme.
    pub(super) fn render_cheat_sheet(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let muted = hsla(t.text_muted);
        // Wide enough for three columns, never wider or taller than the window: the groups
        // scroll under a fixed heading.
        let card = self
            .card(940.)
            .max_w(gpui::relative(0.94))
            .max_h(gpui::relative(0.86))
            .p_4()
            .gap_1()
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .pb_1()
                    .child("Key bindings")
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child("From the live keymap. Esc closes."),
                    ),
            );
        let mut body = div().flex().flex_col().gap_1();
        // While a tour or call stack is open its keys come first, where they are seen.
        let layers = if self.inv.is_some() {
            [
                Layer::Steps,
                Layer::Comparison,
                Layer::Go,
                Layer::View,
                Layer::Read,
                Layer::Compare,
            ]
        } else {
            [
                Layer::Go,
                Layer::View,
                Layer::Read,
                Layer::Compare,
                Layer::Steps,
                Layer::Comparison,
            ]
        };
        for layer in layers {
            let mut rows: Vec<_> = all_commands()
                .iter()
                .filter(|c| c.layer == layer)
                // Compare's palette-only commands are listed too, so the group is complete.
                .filter_map(|c| {
                    let key = match self.key_labels.get(c.action) {
                        Some(k) => k.clone(),
                        None if layer == Layer::Compare => "Palette".to_string(),
                        None => return None,
                    };
                    Some((key, c.title))
                })
                .collect();
            if layer == Layer::Steps {
                // The step panes' own keys (their `Steps` context), first.
                let pane_keys = [
                    ("j / k", "Next / previous step in the pane (also Down / Up)"),
                    ("Enter", "Open the step"),
                    ("n / N", "Next / previous highlighted chunk in the code"),
                    ("Esc", "Back to the code"),
                ];
                for (i, (key, title)) in pane_keys.into_iter().enumerate() {
                    rows.insert(i, (key.to_string(), title));
                }
            }
            if layer == Layer::Comparison {
                // The Comparison pane's own keys, first.
                let pane_keys = [
                    ("j / k", "Next / previous row (also Down / Up)"),
                    ("Enter", "Open the row: both sides on one page"),
                ];
                for (i, (key, title)) in pane_keys.into_iter().enumerate() {
                    rows.insert(i, (key.to_string(), title));
                }
            }
            if rows.is_empty() {
                continue;
            }
            body = body.child(
                div()
                    .pt_2()
                    .text_size(px(11.5))
                    .text_color(muted)
                    .child(layer.title()),
            );
            // Three columns; a long title wraps inside its own column.
            body = body.child(div().flex().flex_wrap().children(rows.into_iter().map(
                |(key, title)| {
                    div()
                        .w(gpui::relative(1. / 3.))
                        .pr_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py(px(2.))
                        .child(div().w(px(96.)).flex_none().child(self.key_cap(key)))
                        .child(div().flex_1().min_w_0().child(title))
                },
            )));
        }
        let body = self
            .scroll_areas
            .get("cheat-sheet")
            .vertical(
                "cheat-sheet-body",
                self.features.probe_if_on(Feature::Overscroll),
                body,
            )
            .flex_1()
            .min_h_0();
        self.layer(card.child(body), cx)
    }

    /// Enter in a dialog, or its primary button.
    pub(super) fn dialog_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.overlay.take() {
            Some(Overlay::Trust { .. }) => self.trust_accept(cx),
            Some(Overlay::OpenLink { url, folder }) => {
                // Known from now on, so following the link again opens it.
                if let Err(e) = cho_sources::recent::remember(&folder) {
                    self.notify_text(format!("Could not remember the folder: {e}"), cx);
                    return;
                }
                crate::send_link(url, cx);
            }
            Some(Overlay::OpenPullRequest(ask)) => {
                // The reader agreed: this is the one place a link makes Chorong fetch.
                self.begin_link_open(ask, window.window_handle(), cx);
            }
            Some(Overlay::PrepareApproval(_)) => self.prepare_approve(cx),
            other => self.overlay = other,
        }
        cx.notify();
    }

    /// Esc in a dialog, or a click outside it. For the prepare panels that is "Open without
    /// indexing".
    pub(super) fn dialog_decline(&mut self, cx: &mut Context<Self>) {
        if let Some(Overlay::PrepareApproval(_)) = self.overlay.take() {
            self.prepare_decline(cx);
        }
        cx.notify();
    }

    /// The question before a `chorong://` link opens a folder Chorong has never opened.
    pub(super) fn render_open_link(
        &self,
        url: &str,
        folder: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let muted = hsla(t.text_muted);
        let button = |id: &'static str, label: &'static str, primary: bool| {
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
                .flex()
                .items_center()
                .gap_2()
                .child(label)
                .children(self.dialog_key(primary))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if primary {
                        this.dialog_accept(window, cx);
                    } else {
                        this.overlay = None;
                    }
                    cx.notify();
                }))
        };
        let card = self
            .card(560.)
            .p_4()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Open a folder Chorong has not opened before?"),
            )
            .child(div().child(folder.display().to_string()))
            .child(div().text_color(muted).child(
                "A link from outside Chorong (a web page, a chat, another program) asked to open \
                 this folder. Opening it only reads it; nothing in it runs unless you trust it \
                 later. Open it only if you expected this link.",
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child(url.to_string()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(button("open-link-cancel", "Don't open", false))
                    .child(button("open-link-accept", "Open folder", true)),
            );
        self.layer(card, cx)
    }

    /// The question before the first index build in a repository.
    pub(super) fn render_trust(
        &self,
        notes: &[String],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let muted = hsla(t.text_muted);
        let name = self
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "this folder".into());
        let button = |id: &'static str, label: String, primary: bool, accept: bool| {
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
                .flex()
                .items_center()
                .gap_2()
                .child(label)
                .children(self.dialog_key(accept))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if accept {
                        this.dialog_accept(window, cx);
                    } else {
                        this.overlay = None;
                    }
                    cx.notify();
                }))
        };
        let card = self
            .card(520.)
            .p_4()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("Trust {name} to build its index?")),
            )
            .child(div().text_color(muted).child(
                "Indexing runs this project's own code as you: Rust build scripts and \
                 procedural macros, and its TypeScript or Python tooling. That code can read \
                 your files. Chorong passes it only the settings a build needs, not tokens or keys \
                 from your environment, and never runs a package install.",
            ))
            .child(div().text_color(muted).child(
                "The answer is remembered for this folder in Chorong's own settings, not in the \
                 repository. Each build still starts only when you ask.",
            ))
            .children(notes.iter().map(|n| {
                div()
                    .text_size(px(12.))
                    .text_color(hsla(t.players.get(1).map_or(t.text, |p| p.cursor)))
                    .child(n.clone())
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(button("trust-cancel", "Not now".into(), false, false))
                    .child(button(
                        "trust-accept",
                        "Trust and build index".into(),
                        true,
                        true,
                    )),
            );
        self.layer(card, cx)
    }

    /// The stderr tail of a failed build.
    pub(super) fn render_index_error(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let tail = self
            .index_failure
            .clone()
            .unwrap_or_else(|| "No error recorded.".into());
        let lines: Vec<String> = {
            let all: Vec<&str> = tail.lines().collect();
            let skip = all.len().saturating_sub(18);
            all[skip..].iter().map(|l| l.to_string()).collect()
        };
        // Copies the whole recorded tail, not only the lines shown.
        let copy = self
            .plain_button("index-error-copy", "Copy error", false)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(tail.clone()));
                this.notify_text("Copied the error", cx);
            }));
        let card = self
            .card(760.)
            .p_4()
            .gap_2()
            .child(div().font_weight(FontWeight::MEDIUM).child("Index: failed"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(hsla(t.text_muted))
                    .child("The end of the indexer's stderr. Build Index tries again."),
            )
            .child(
                div()
                    .p_2()
                    .rounded_sm()
                    .bg(hsla(t.editor_background))
                    .font_family(self.font.clone())
                    .text_size(px(11.5))
                    .children(
                        lines
                            .into_iter()
                            .map(|l| div().whitespace_nowrap().overflow_hidden().child(l)),
                    ),
            )
            .child(div().flex().justify_end().child(copy));
        self.layer(card, cx)
    }

    /// The popup listing what can follow the chord in progress (Cmd-K).
    pub(super) fn render_chord_popup(
        &self,
        pending: &[(String, String)],
        prefix: String,
    ) -> impl IntoElement {
        let t = &self.theme;
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(34.))
            .flex()
            .justify_center()
            .child(
                self.card(520.)
                    .p_3()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_color(hsla(t.text_muted))
                            .child(self.key_cap(prefix))
                            .child("then\u{2026}  (changes how you view, never where you are)"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .children(pending.iter().map(|(k, title)| {
                                div()
                                    .w(gpui::relative(0.5))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .py(px(2.))
                                    .child(
                                        div().w(px(84.)).flex_none().child(self.key_cap(k.clone())),
                                    )
                                    .child(title.clone())
                            })),
                    ),
            )
    }
}

impl Chorong {
    /// The strip above the source about the link that was opened (see `Banner`). Dismiss
    /// sits at the end of the message; the other actions form one row below it.
    pub(super) fn render_banner(
        &self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let b = self.editors.get(&pane)?.banner.as_ref()?;
        let t = &self.theme;
        let accent = t.players.first().map(|p| p.cursor).unwrap_or(t.text);
        let button = move |i: usize, label: &str, action: &BannerAction, cx: &mut Context<Self>| {
            let action = action.clone();
            div()
                .id(("banner-action", i))
                .flex_none()
                .px_2()
                .rounded_sm()
                .border_1()
                .border_color(hsla(t.border))
                .bg(hsla(t.editor_background))
                .text_color(hsla(t.text))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(t.element_selected)))
                .child(label.to_string())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.activate_editor(pane, cx);
                    this.banner_action(action.clone(), window, cx);
                    cx.notify();
                }))
        };
        let mut dismiss = None;
        let mut row = div().flex().gap_2().items_center().overflow_hidden();
        for (i, (label, action)) in b.actions.iter().enumerate() {
            if matches!(action, BannerAction::Dismiss) {
                dismiss = Some(button(i, label, action, cx));
            } else {
                row = row.child(button(i, label, action, cx));
            }
        }
        let has_row = b
            .actions
            .iter()
            .any(|(_, a)| !matches!(a, BannerAction::Dismiss));
        Some(
            div()
                .id("link-banner")
                .flex_none()
                .px_3()
                .py(px(if b.quiet { 4. } else { 8. }))
                .flex()
                .flex_col()
                .gap_1()
                .border_b_1()
                .border_color(hsla(t.border))
                .when(!b.quiet, |d| d.border_l_4().border_color(hsla(accent)))
                .bg(hsla(t.panel_background))
                .text_size(px(if b.quiet { 12. } else { 12.5 }))
                .text_color(hsla(if b.quiet { t.text_muted } else { t.text }))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_start()
                        .child(div().flex_1().min_w_0().child(b.text.clone()))
                        .children(dismiss),
                )
                .children(b.detail.clone().map(|d| {
                    div()
                        .font_family(self.font.clone())
                        .text_size(px(12.))
                        .text_color(hsla(t.text_muted))
                        .child(d)
                }))
                .when(has_row, |d| d.child(row))
                .into_any_element(),
        )
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [ShowCheatSheet, DismissOverlay, TrustAccept, TrustDecline,]
);

pub(super) const COMMANDS: &[Command] = &[c(
    "chorong::ShowCheatSheet",
    "Show Key Bindings",
    Layer::Read,
    340,
)];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, ShowCheatSheet, |this, _w, cx| this.overlay =
        Some(Overlay::Cheat));
    let d = on!(d, cx, DismissOverlay, |this, _w, cx| this.overlay = None);
    let d = on!(d, cx, TrustAccept, |this, window, cx| this
        .dialog_accept(window, cx));
    on!(d, cx, TrustDecline, |this, _w, cx| this.dialog_decline(cx))
}

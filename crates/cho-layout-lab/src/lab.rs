//! The lab's host of the shell: the window frame around `cho_layout_ui::shell::Shell`. The
//! root tracks the lab's focus and carries the `LayoutLab` key context, binds the layout
//! actions, and draws the status bar (placeholder fields and chips around the shell's dock
//! toggles, notice and Layout-mode hint bar).

use crate::body;
use cho_layout::{PaneContent, PaneId, Pin, Workspace};
use cho_layout_ui::actions::ReopenFocusLayout;
use cho_layout_ui::colors::Colors;
use cho_layout_ui::keys;
use cho_layout_ui::shell::{PaneBodies, Shell, bind_actions};
use cho_layout_ui::widgets::small_icon;
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    div, prelude::FluentBuilder, px,
};

/// The lab's pane contents: placeholders (see `body.rs`).
pub struct LabBodies {
    pub colors: Colors,
}

impl PaneBodies for LabBodies {
    fn body(
        &self,
        _pane: PaneId,
        content: &PaneContent,
        _ws: &Workspace,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        body::render(content, &self.colors)
    }

    fn qualifier(&self, _pane: PaneId, content: &PaneContent, _cx: &App) -> String {
        body::qualifier(content)
    }

    fn pin(&self, n: usize, _cx: &App) -> Option<Pin> {
        Some(crate::scenes::pin(n))
    }
}

/// The lab window's root view.
pub struct LabView {
    shell: Entity<Shell>,
    focus: FocusHandle,
}

impl LabView {
    pub fn new(shell: Entity<Shell>, focus: FocusHandle) -> Self {
        Self { shell, focus }
    }
}

impl Focusable for LabView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LabView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.shell.read(cx).colors().clone();
        let status = self.status_bar(&c, window, cx);
        bind_actions(
            div()
                .key_context("LayoutLab")
                .track_focus(&self.focus)
                .size_full()
                .flex()
                .flex_col()
                .bg(c.panel)
                .font_family(c.ui_font.clone())
                .text_color(c.text)
                .text_size(px(12.5)),
            &self.shell,
        )
        .child(self.shell.clone())
        .child(status)
    }
}

impl LabView {
    fn status_bar(&self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let left = self
            .shell
            .update(cx, |shell, cx| shell.status_left(window, cx));
        let hint = self
            .shell
            .update(cx, |shell, cx| shell.hint_bar(window, cx));
        let (notice, in_mode, history) = {
            let s = self.shell.read(cx);
            (
                s.notice(),
                s.in_layout_mode(),
                s.workspace().history().len(),
            )
        };
        let field = |s: &str| div().child(SharedString::from(s.to_string()));
        let history_key = keys::label(&ReopenFocusLayout, window);
        div()
            .h(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(6.))
            .bg(c.status_bar)
            .border_t_1()
            .border_color(c.border)
            .text_size(px(11.5))
            .text_color(c.text_muted)
            .child(left)
            .when(!in_mode, |d| {
                d.child(div().text_color(c.text).child("Rust"))
                    .child(field("412 lines"))
                    .child(field("parse complete"))
                    .child(field("38 declarations"))
                    .child(
                        // Index: current, a solid success dot (placeholder state).
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(
                                div()
                                    .size(px(7.))
                                    .rounded(px(4.))
                                    .bg(c.success.unwrap_or(c.text_muted)),
                            )
                            .child("Index: current"),
                    )
            })
            .when_some(notice, |d, n| {
                d.child(
                    div()
                        .px(px(6.))
                        .rounded(px(4.))
                        .text_color(c.warning.unwrap_or(c.text))
                        .when_some(c.warning_bg, |d, bg| d.bg(bg))
                        .child(SharedString::from(n)),
                )
            })
            .when_some(hint, |d, bar| d.child(bar))
            .when(!in_mode, |d| {
                d.child(div().flex_1())
                    .child(div().text_color(c.text).child("Ln 12, Col 4"))
                    .child(status_chip(c, None, "Critical Lens"))
                    .child(status_chip(c, Some("feather"), "Lightweight"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(small_icon("lock", c.icon_muted))
                            .child("Read-only"),
                    )
                    .child(
                        div()
                            .id("focus-history")
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .px(px(4.))
                            .rounded(px(4.))
                            .hover(|d| d.bg(c.hover))
                            .child(small_icon("history", c.icon_muted))
                            .child(SharedString::from(format!("Focus history {history}")))
                            .when_some(history_key, |d, k| d.child(SharedString::from(k)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                window.focus(&this.focus);
                                window.dispatch_action(Box::new(ReopenFocusLayout), cx);
                            })),
                    )
                    .child(field(&c.name))
            })
            .into_any_element()
    }
}

/// A status-bar chip that is on (Critical Lens, Lightweight): text.accent at 16%, the
/// icon in icon.accent. Placeholders in the lab.
fn status_chip(c: &Colors, glyph: Option<&str>, label: &'static str) -> AnyElement {
    div()
        .flex_none()
        .h(px(18.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded(px(4.))
        .bg(c.accent_wash)
        .text_color(c.text)
        .when(glyph.is_some(), |d| d.font_weight(FontWeight::MEDIUM))
        .children(glyph.map(|g| small_icon(g, c.icon_accent)))
        .child(label)
        .into_any_element()
}

//! The shell around the tree: focus bar and page strip, tray, status-bar pieces, pane menu and
//! focus history.

use super::*;

impl Shell {
    pub(super) fn render_focus_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let f = self.ws.focus().expect("in focus");
        let on_screen = f
            .on_screen()
            .iter()
            .filter_map(|p| self.ws.content(*p).map(|x| x.title()))
            .collect::<Vec<_>>()
            .join(" · ");
        let key = keys::label(&RestoreLayout, window);
        // text.accent at 10% over panel.background.
        div()
            .flex_none()
            .bg(c.panel)
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .bg(c.accent_bar)
                    .border_b_1()
                    .border_color(c.border)
                    .text_size(px(11.5))
                    .child(small_icon("maximize", c.icon_accent))
                    .child(
                        div()
                            .text_color(c.text)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Focus layout"),
                    )
                    .child(self.render_page_strip(window, cx))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(c.text_muted)
                            .child(SharedString::from(on_screen)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("restore-layout")
                            .h(px(20.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded(px(4.))
                            .bg(c.element_bg)
                            .border_1()
                            .border_color(c.border)
                            .hover(|d| d.bg(c.hover))
                            .active(|d| d.bg(c.active))
                            .text_color(c.text)
                            .child(small_icon("rotate-ccw", c.icon))
                            .child("Exit focus")
                            .when_some(key, |d, k| d.child(key_chip(c, &k)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                window.focus(&this.return_focus);
                                window.dispatch_action(Box::new(RestoreLayout), cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_tray(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let hot = self.hint == Some(Hint::Tray);
        let tray = self.ws.tray();
        div()
            .id("tray")
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .bg(c.elevated)
            .border_t_1()
            .border_color(c.border)
            .shadow_md()
            .when(hot, |d| {
                d.bg(c.drop_target)
                    .border_1()
                    .border_dashed()
                    .border_color(c.text_accent)
            })
            .child(small_icon("inbox", c.icon_muted))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(c.text_muted)
                    .mr(px(4.))
                    .child(SharedString::from(format!("Tray {}", tray.len()))),
            )
            .children(tray.into_iter().map(|p| {
                let content = self.ws.content(p).expect("exists");
                // In Layout mode each chip shows its jump label: pressing it brings the pane up.
                let label = self.mode.as_ref().and_then(|m| {
                    m.tray
                        .iter()
                        .find(|(_, id)| *id == p)
                        .map(|(l, _)| SharedString::from(l.to_string()))
                });
                let badge = label.map(|l| {
                    div()
                        .flex_none()
                        .h(px(16.))
                        .min_w(px(16.))
                        .px(px(3.))
                        .flex()
                        .justify_center()
                        .items_center()
                        .rounded(px(3.))
                        .bg(c.text_accent)
                        .text_color(c.editor)
                        .text_size(px(11.))
                        .font_weight(FontWeight::BOLD)
                        .child(l)
                });
                div()
                    .id(("chip", p.0))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(c.border_variant)
                    .bg(c.panel)
                    .hover(|d| d.bg(c.hover))
                    .cursor_grab()
                    .text_size(px(12.))
                    .text_color(c.text)
                    .children(badge)
                    .child(small_icon(kind_icon(content.kind()), c.icon))
                    .child(SharedString::from(content.title()))
                    .on_drag(self.payload(Dragged::Pane(p)), |p, at, _, cx| {
                        let p = p.clone();
                        cx.new(|_| DragChip(p, at))
                    })
            }))
            .on_drag_move::<DragPayload>(cx.listener(
                |this, e: &DragMoveEvent<DragPayload>, _, cx| {
                    let what = e.drag(cx).what;
                    if e.bounds.contains(&e.event.position) {
                        this.set_hint(what, Hint::Tray, cx);
                    } else {
                        this.clear_hint_if(|h| h == Hint::Tray, cx);
                    }
                },
            ))
            .into_any_element()
    }

    /// The three dock toggles for the left of the status bar.
    pub fn status_left(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let in_focus = self.ws.in_focus();
        let toggle =
            |d: DockId, glyph: &'static str, label: &'static str, action: Box<dyn Action>| {
                let on = self.ws.arrangement().dock(d).visible;
                let action = std::rc::Rc::new(action);
                div()
                    .id(SharedString::from(format!("toggle-{d:?}")))
                    .size(px(20.))
                    .flex()
                    .justify_center()
                    .items_center()
                    .rounded(px(4.))
                    // Shown: ghost_element.selected with the icon in text; hidden: transparent.
                    .when(on && !in_focus, |e| e.bg(c.ghost_selected))
                    .when(!in_focus, |e| {
                        e.hover(|e| e.bg(c.hover)).active(|e| e.bg(c.ghost_active))
                    })
                    .child(small_icon(
                        glyph,
                        if in_focus {
                            c.icon_disabled
                        } else if on {
                            c.text
                        } else {
                            c.icon
                        },
                    ))
                    .tooltip(tip(label, c))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.return_focus);
                        window.dispatch_action(action.boxed_clone(), cx);
                    }))
            };
        div()
            .flex()
            .gap(px(2.))
            .child(toggle(
                DockId::Left,
                "panel-left",
                "Left dock",
                Box::new(ToggleLeftDock),
            ))
            .child(toggle(
                DockId::Bottom,
                "panel-bottom",
                "Bottom bar",
                Box::new(ToggleBottomBar),
            ))
            .child(toggle(
                DockId::Right,
                "panel-right",
                "Right dock",
                Box::new(ToggleRightDock),
            ))
            .into_any_element()
    }

    pub(super) fn render_menu(
        &self,
        menu: &Menu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &self.c;
        let pane = menu.pane;
        let kind = self.ws.kind(pane);
        let in_focus = self.ws.in_focus();
        let minimized = self
            .ws
            .group_of(pane)
            .and_then(|g| self.ws.group(g))
            .is_some_and(|g| g.minimized);
        let mut items: Vec<(SharedString, Box<dyn Action>)> = Vec::new();
        if !in_focus {
            if kind != Some(PaneKind::Editor) {
                items.push(("Move to left dock".into(), Box::new(MovePaneToLeftDock)));
                items.push(("Move to right dock".into(), Box::new(MovePaneToRightDock)));
                items.push(("Move to bottom bar".into(), Box::new(MovePaneToBottomBar)));
                let in_editor_area = self
                    .ws
                    .arrangement()
                    .locate(pane)
                    .is_some_and(|(r, _)| r == cho_layout::Region::Editor);
                if !in_editor_area {
                    items.push(("Move to code area".into(), Box::new(MovePaneToEditorArea)));
                }
            }
        } else {
            items.push(("Send to tray".into(), Box::new(SendPaneToTray)));
            items.push((
                "Send to next page".into(),
                Box::new(MovePaneToNextFocusPage),
            ));
        }
        items.push(("Split right".into(), Box::new(SplitPaneRight)));
        items.push(("Split down".into(), Box::new(SplitPaneDown)));
        if kind == Some(PaneKind::Context) {
            items.push(("Pin here".into(), Box::new(PinContextHere)));
            items.push(("Pin to bottom bar".into(), Box::new(PinContextToBottomBar)));
        }
        items.push((
            if minimized { "Expand" } else { "Minimise" }.into(),
            Box::new(MinimizePane),
        ));
        items.push((
            if in_focus { "Exit focus" } else { "Maximise" }.into(),
            Box::new(ToggleMaximizePane),
        ));
        items.push((
            if kind == Some(PaneKind::PinnedContext) {
                "Unpin"
            } else {
                // "pane": the investigation panes' menus also have Close investigation.
                "Close pane"
            }
            .into(),
            Box::new(ClosePane),
        ));
        if let Some(content) = self.ws.content(pane) {
            insert_host_items(&mut items, self.bodies.menu_items(pane, content, cx));
        }
        let rows = items.into_iter().enumerate().map(|(i, (label, action))| {
            let key = keys::label(action.as_ref(), window);
            let action = std::rc::Rc::new(action);
            div()
                .id(("menu-item", i))
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(16.))
                .rounded(px(4.))
                .hover(|d| d.bg(c.hover))
                .text_color(c.text)
                .child(div().flex_1().child(label))
                .when_some(key, |d, k| d.child(key_chip(c, &k)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.dispatch_on(pane, action.boxed_clone(), window, cx);
                }))
        });
        deferred(
            anchored().position(menu.at).snap_to_window().child(
                div()
                    .id("pane-menu")
                    .occlude()
                    .min_w(px(200.))
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .bg(c.elevated)
                    .border_1()
                    .border_color(c.border)
                    .rounded(px(6.))
                    .shadow_lg()
                    .text_size(px(12.5))
                    .children(rows)
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                        this.close_popup(window, cx);
                    })),
            ),
        )
        .into_any_element()
    }

    pub(super) fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let now = now();
        let entries = self.ws.focus_history(now, |s| s.to_content().is_some());
        let body: Vec<AnyElement> = if entries.is_empty() {
            vec![
                div()
                    .p(px(8.))
                    .text_color(c.text_muted)
                    .child("No focus layouts yet. Maximise a pane, then restore.")
                    .into_any_element(),
            ]
        } else {
            entries
                .into_iter()
                .map(|e| {
                    let index = e.index;
                    let missing = e
                        .missing
                        .iter()
                        .map(|(page, s)| format!("{} (page {})", s.label(), page + 1))
                        .collect::<Vec<_>>()
                        .join(", ");
                    div()
                        .id(("history", index))
                        .px(px(8.))
                        .py(px(4.))
                        .flex()
                        .flex_col()
                        .rounded(px(4.))
                        .hover(|d| d.bg(c.hover))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(div().text_color(c.text).child(SharedString::from(e.label)))
                                .child(
                                    div()
                                        .text_color(c.text_muted)
                                        .child(SharedString::from(e.ago)),
                                ),
                        )
                        .when(!missing.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(c.error.unwrap_or(c.text_muted))
                                    .child(SharedString::from(format!("missing: {missing}"))),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.popup = Popup::None;
                            this.apply(cx, |ws| {
                                ws.reopen_focus(index, now, |s| s.to_content().is_some())
                                    .map(|_| ())
                            });
                        }))
                        .into_any_element()
                })
                .collect()
        };
        deferred(
            div()
                .absolute()
                .top(px(60.))
                .left(px(0.))
                .right(px(0.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("history-list")
                        .occlude()
                        .w(px(460.))
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .bg(c.elevated)
                        .border_1()
                        .border_color(c.border)
                        .rounded(px(6.))
                        .shadow_lg()
                        .text_size(px(12.5))
                        .child(
                            div()
                                .px(px(8.))
                                .py(px(4.))
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c.text_muted)
                                .child("Reopen focus layout"),
                        )
                        .children(body)
                        .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                            this.close_popup(window, cx);
                        })),
                ),
        )
        .into_any_element()
    }

    /// The page chips in the focus bar: `1 2 3 +`. A chip shows its page, and takes a dropped
    /// pane to that page.
    pub(super) fn render_page_strip(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let f = self.ws.focus().expect("in focus");
        let n = f.pages.len();
        // The strip: panel.background, 1 px border.variant; chips transparent at rest.
        let mut strip = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(5.))
            .bg(c.panel)
            .border_1()
            .border_color(c.border_variant);
        for i in 0..n {
            let current = i == f.current;
            let hot = self.hint == Some(Hint::Page(i));
            let empty = f.pages[i].root.is_none();
            let host = self.bodies.page_chip(i, &self.ws, cx);
            if host.as_ref().is_some_and(|h| h.divider_before) {
                strip = strip.child(div().mx(px(3.)).w(px(1.)).h(px(14.)).bg(c.border));
            }
            let label: SharedString = host
                .as_ref()
                .map(|h| h.label.clone())
                .unwrap_or_else(|| SharedString::from((i + 1).to_string()));
            let chip = div()
                .id(("page", i))
                .h(px(18.))
                .min_w(px(20.))
                .px(px(5.))
                .relative()
                .flex()
                .justify_center()
                .items_center()
                .rounded(px(4.))
                .when(hot, |d| {
                    d.border_1()
                        .border_dashed()
                        .border_color(c.text_accent)
                        .bg(c.drop_target)
                });
            let chip = match host.as_ref().map(|h| h.look) {
                None => chip
                    .when(!current, |d| d.hover(|d| d.bg(c.hover).text_color(c.text)))
                    // The current page: filled text.accent, number on-accent.
                    .when(current && !hot, |d| d.bg(c.text_accent))
                    .text_color(if current {
                        c.editor
                    } else if empty {
                        c.icon_disabled
                    } else {
                        c.text_muted
                    }),
                Some(look) => {
                    // A tour's badges (design/layout.md, "Tours"), as the Tour pane draws them.
                    let (ok, bad) = (
                        c.success.unwrap_or(c.text_accent),
                        c.error.unwrap_or(c.text),
                    );
                    let outlined =
                        |d: gpui::Stateful<gpui::Div>, colour: gpui::Hsla, text: gpui::Hsla| {
                            d.border(px(1.5)).border_color(colour).text_color(text)
                        };
                    let d = chip
                        .rounded_full()
                        .when(!hot, |d| d.hover(|d| d.bg(c.hover)));
                    match look {
                        ChipLook::Current => {
                            d.when(!hot, |d| d.bg(c.text_accent)).text_color(c.editor)
                        }
                        ChipLook::Done => outlined(d, ok, ok),
                        ChipLook::ToDo => outlined(d, c.border, c.text_muted),
                        ChipLook::Broken => outlined(d, bad, bad),
                        ChipLook::Own => outlined(d, c.border, c.text_muted).border_dashed(),
                    }
                }
            };
            let edited = host.as_ref().is_some_and(|h| h.edited);
            let tooltip = host.and_then(|h| h.tooltip);
            strip = strip.child(
                chip.when(current, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(label)
                    .when(edited, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(-2.))
                                .right(px(-2.))
                                .size(px(6.))
                                .rounded_full()
                                .bg(c.warning.unwrap_or(c.text_accent)),
                        )
                    })
                    .when_some(tooltip, |d, t| d.tooltip(tip_owned(t, c)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.return_focus);
                        window.dispatch_action(Box::new(GoToFocusPage(i + 1)), cx);
                    }))
                    .on_drag_move::<DragPayload>(cx.listener(
                        move |this, e: &DragMoveEvent<DragPayload>, _, cx| {
                            let what = e.drag(cx).what;
                            if e.bounds.contains(&e.event.position) {
                                this.set_hint(what, Hint::Page(i), cx);
                            } else {
                                this.clear_hint_if(|h| h == Hint::Page(i), cx);
                            }
                        },
                    )),
            );
        }
        let key = keys::label(&NewFocusPage, window);
        let new_page_tip: &'static str = if key.is_some() {
            "New page (see the pane menu for keys)"
        } else {
            "New page"
        };
        strip
            .child(
                div()
                    .id("new-page")
                    .h(px(18.))
                    .w(px(20.))
                    .flex()
                    .justify_center()
                    .items_center()
                    .rounded(px(4.))
                    .text_color(c.text_muted)
                    .hover(|d| d.bg(c.hover))
                    .child("+")
                    .tooltip(tip(new_page_tip, c))
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.return_focus);
                        window.dispatch_action(Box::new(NewFocusPage), cx);
                    })),
            )
            .into_any_element()
    }

    /// An empty focus page: a drop target for panes from the tray or other pages.
    pub(super) fn render_empty_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let hot = self.hint == Some(Hint::EmptyPage);
        div()
            .id("empty-page")
            .flex_1()
            .m(px(12.))
            .flex()
            .justify_center()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_dashed()
            .border_color(if hot { c.text_accent } else { c.border })
            .when(hot, |d| d.bg(c.accent_wash))
            .text_color(c.text_muted)
            .child("Empty page: drag panes here from the tray")
            .on_drag_move::<DragPayload>(cx.listener(
                |this, e: &DragMoveEvent<DragPayload>, _, cx| {
                    let what = e.drag(cx).what;
                    if e.bounds.contains(&e.event.position) {
                        this.set_hint(what, Hint::EmptyPage, cx);
                    } else {
                        this.clear_hint_if(|h| h == Hint::EmptyPage, cx);
                    }
                },
            ))
            .into_any_element()
    }
}

/// The host's pane-menu entries go before "Split right" (after the moves), or last when the
/// menu has none.
fn insert_host_items<T>(items: &mut Vec<(SharedString, T)>, host: Vec<(SharedString, T)>) {
    let at = items
        .iter()
        .position(|(l, _)| l.as_ref() == "Split right")
        .unwrap_or(items.len());
    items.splice(at..at, host);
}

#[cfg(test)]
mod menu_tests {
    use super::*;

    #[test]
    fn host_items_come_before_split_right() {
        let mut items: Vec<(SharedString, ())> = ["Move to left dock", "Split right", "Close pane"]
            .into_iter()
            .map(|l| (l.into(), ()))
            .collect();
        insert_host_items(
            &mut items,
            vec![("Expand all".into(), ()), ("Collapse all".into(), ())],
        );
        let labels: Vec<&str> = items.iter().map(|(l, _)| l.as_ref()).collect();
        assert_eq!(
            labels,
            [
                "Move to left dock",
                "Expand all",
                "Collapse all",
                "Split right",
                "Close pane"
            ]
        );
    }
}

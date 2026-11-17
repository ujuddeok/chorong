//! Drawing the layout tree: groups, headers, tab strips, docks and the normal view.

use super::*;

impl Shell {
    // ----- rendering: trees, groups, headers -------------------------------------------------

    pub(super) fn render_node(
        &self,
        node: &Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            Node::Group(g) => self.render_group(g, window, cx),
            Node::Split(s) => {
                let row = s.axis == Axis::Row;
                let split = s.id;
                let weights: Vec<f32> = s.children.iter().map(|c| c.weight).collect();
                let mut container = div()
                    .id(("split", split.0))
                    .flex()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .when(row, |d| d.flex_row())
                    .when(!row, |d| d.flex_col())
                    .on_drag_move::<SplitHandle>(cx.listener(
                        move |this, e: &DragMoveEvent<SplitHandle>, _, cx| {
                            let h = *e.drag(cx);
                            if h.split != split {
                                return;
                            }
                            let b = e.bounds;
                            let pos = e.event.position;
                            let frac = if h.axis == Axis::Row {
                                (pos.x - b.left()) / b.size.width
                            } else {
                                (pos.y - b.top()) / b.size.height
                            };
                            let boundary: f32 = weights[..=h.handle].iter().sum();
                            let _ = this.ws.resize_split(split, h.handle, frac - boundary);
                            this.dirty = true;
                            cx.notify();
                        },
                    ));
                let n = s.children.len();
                for (i, child) in s.children.iter().enumerate() {
                    let folded = !row && matches!(&child.node, Node::Group(g) if g.minimized);
                    let mut item = div().flex().min_w_0().min_h_0().overflow_hidden();
                    if folded {
                        item = item.flex_none().h(px(HEADER));
                    } else {
                        // Flexbox hands out only part of the free space when grow factors
                        // sum to less than 1 (a folded sibling leaves the rest at < 1), so
                        // scale shares up; only their ratios matter.
                        item = item.flex_basis(px(0.)).flex_shrink();
                        item.style().flex_grow = Some(child.weight * 1000.);
                    }
                    container =
                        container.child(item.child(self.render_node(&child.node, window, cx)));
                    if i + 1 < n {
                        container = container.child(self.split_handle(SplitHandle {
                            split,
                            handle: i,
                            axis: s.axis,
                        }));
                    }
                }
                container.into_any_element()
            }
        }
    }

    pub(super) fn split_handle(&self, h: SplitHandle) -> AnyElement {
        let row = h.axis == Axis::Row;
        let c = &self.c;
        div()
            .id(SharedString::from(format!(
                "handle-{}-{}",
                h.split.0, h.handle
            )))
            .flex_none()
            .flex()
            .justify_center()
            .items_center()
            .when(row, |d| {
                d.w(px(HANDLE)).h_full().flex_row().cursor_col_resize()
            })
            .when(!row, |d| {
                d.h(px(HANDLE)).w_full().flex_col().cursor_row_resize()
            })
            .child(
                div()
                    .bg(c.border)
                    .when(row, |d| d.w(px(1.)).h_full())
                    .when(!row, |d| d.h(px(1.)).w_full()),
            )
            .hover(|d| d.bg(c.hover))
            .on_drag(h, |_, _, _, cx| cx.new(|_| Nothing))
            .into_any_element()
    }

    pub(super) fn render_group(
        &self,
        g: &Group,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &self.c;
        let group = g.id;
        let active = g.active_pane().expect("groups are never empty");
        let content = self.ws.content(active).expect("pane exists").clone();
        let body_bg = if content.kind() == PaneKind::Editor {
            c.editor
        } else {
            c.panel
        };
        let mut zones: Vec<(Option<Edge>, Option<&str>)> = match self.hint {
            Some(Hint::Zone { group: hg, edge }) if hg == group => vec![(edge, None)],
            _ => Vec::new(),
        };
        zones.extend(self.previews.iter().filter_map(|(dir, t)| {
            let edge = match *t {
                Target::Tabs { group: g, .. } if g == group => None,
                Target::Beside { group: g, edge } if g == group => Some(edge),
                Target::Dock(d)
                    if self.dock_shown(d)
                        && self
                            .ws
                            .arrangement()
                            .dock(d)
                            .root
                            .as_ref()
                            .map(|n| n.first_group().id)
                            == Some(group) =>
                {
                    None
                }
                _ => return None,
            };
            Some((edge, Some(arrow(*dir))))
        }));
        let body = (!g.minimized && self.body_visible(group))
            .then(|| self.body_view(active, &content, cx));
        div()
            .id(("group", group.0))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(body_bg)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    if let Some(p) = this.ws.group(group).and_then(Group::active_pane) {
                        let _ = this.ws.activate(p);
                    }
                    this.mode = None;
                    window.focus(&this.return_focus);
                    cx.notify();
                }),
            )
            .on_drag_move::<DragPayload>(cx.listener(
                move |this, e: &DragMoveEvent<DragPayload>, _, cx| {
                    let what = e.drag(cx).what;
                    let b = e.bounds;
                    let p = e.event.position;
                    let in_body = b.contains(&p) && p.y >= b.top() + px(HEADER);
                    if !in_body {
                        this.clear_hint_if(
                            |h| matches!(h, Hint::Zone { group: hg, .. } if hg == group),
                            cx,
                        );
                        return;
                    }
                    let body_h = b.size.height - px(HEADER);
                    let x = (p.x - b.left()) / b.size.width;
                    let y = (p.y - b.top() - px(HEADER)) / body_h;
                    let edges = [
                        (x, Edge::Left),
                        (1. - x, Edge::Right),
                        (y, Edge::Top),
                        (1. - y, Edge::Bottom),
                    ];
                    let (d, edge) = edges
                        .into_iter()
                        .fold((f32::MAX, Edge::Left), |a, b| if b.0 < a.0 { b } else { a });
                    let edge = (d < EDGE_SHARE).then_some(edge);
                    this.set_hint(what, Hint::Zone { group, edge }, cx);
                },
            ))
            .child(self.header_view(g, cx))
            .when_some(body, |d, body| {
                d.child(div().flex_1().min_h_0().overflow_hidden().child(body))
            })
            .child(self.measure(group))
            .children(
                zones
                    .into_iter()
                    .map(|(edge, key)| self.zone_overlay(edge, key)),
            )
            .when(
                self.ws.focused().is_some_and(|f| g.tabs.contains(&f)),
                |d| {
                    let outline = div().absolute().inset_0();
                    d.child(if self.mode.is_some() {
                        outline.border_2().border_color(c.text_accent)
                    } else {
                        outline.border_1().border_color(c.pane_focus)
                    })
                },
            )
            .into_any_element()
    }

    /// The tinted, dashed area where a dropped pane would go; in Layout mode's preview, with
    /// the arrow that moves it there.
    pub(super) fn zone_overlay(&self, edge: Option<Edge>, key: Option<&str>) -> AnyElement {
        let c = &self.c;
        let base = div()
            .absolute()
            .flex()
            .justify_center()
            .items_center()
            .when_some(key, |e, k| e.child(self.arrow_cap(k)))
            .bg(c.drop_target)
            .border_1()
            .border_dashed()
            .border_color(c.text_accent)
            .rounded(px(4.));
        let top = px(HEADER + 2.);
        match edge {
            None => base.top(top).left(px(4.)).right(px(4.)).bottom(px(4.)),
            Some(Edge::Left) => base.top(top).bottom(px(2.)).left(px(2.)).w(relative(0.5)),
            Some(Edge::Right) => base.top(top).bottom(px(2.)).right(px(2.)).w(relative(0.5)),
            Some(Edge::Top) => base.top(top).left(px(2.)).right(px(2.)).h(relative(0.45)),
            Some(Edge::Bottom) => base
                .bottom(px(2.))
                .left(px(2.))
                .right(px(2.))
                .h(relative(0.45)),
        }
        .into_any_element()
    }

    /// Where the 2 px accent line goes in this group's tab strip, as a visual slot (0 = before
    /// the first tab, `tabs.len()` = after the last).
    pub(super) fn accent_slot(&self, g: &Group) -> Option<usize> {
        let Some(Hint::Tab { group, index, .. }) = self.hint else {
            return None;
        };
        if group != g.id {
            return None;
        }
        let dragged = match self.dragged? {
            Dragged::Pane(p) => p,
            Dragged::Group(gr) => self.ws.group(gr).map(|x| x.tabs[0])?,
        };
        let eff = self.ws.tab_drop_index(group, dragged, index)?;
        let without: Vec<PaneId> = g.tabs.iter().copied().filter(|p| *p != dragged).collect();
        Some(match without.get(eff) {
            Some(next) => g
                .tabs
                .iter()
                .position(|p| p == next)
                .unwrap_or(g.tabs.len()),
            None => g.tabs.len(),
        })
    }

    /// The tab drop handler for one slot of a tab strip: dropping on the left half of tab `i`
    /// inserts before it, on the right half after it.
    pub(super) fn tab_drop(
        &self,
        group: NodeId,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl Fn(&DragMoveEvent<DragPayload>, &mut Window, &mut App) + 'static {
        cx.listener(move |this, e: &DragMoveEvent<DragPayload>, _, cx| {
            let what = e.drag(cx).what;
            let b = e.bounds;
            let p = e.event.position;
            if b.contains(&p) {
                let after = p.x > b.left() + b.size.width / 2.;
                this.set_hint(
                    what,
                    Hint::Tab {
                        group,
                        index: index + usize::from(after),
                        from: index,
                    },
                    cx,
                );
            } else {
                this.clear_hint_if(
                    |h| {
                        matches!(h, Hint::Tab { group: hg, from, .. }
                            if hg == group && from == index)
                    },
                    cx,
                );
            }
        })
    }

    pub(super) fn accent_line(&self, at_end: bool) -> AnyElement {
        div()
            .absolute()
            .top(px(4.))
            .h(px(HEADER - 8.))
            .w(px(2.))
            .when(at_end, |d| d.right(px(0.)))
            .when(!at_end, |d| d.left(px(0.)))
            .bg(self.c.text_accent)
            .into_any_element()
    }

    /// Everything a group's header shows, hashed: when it changes the header is drawn anew.
    /// Keep in step with [`Shell::render_header`]: whatever it reads goes in here.
    pub(super) fn header_key(&self, g: &Group, cx: &mut Context<Self>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (g.id, &g.tabs, g.active, g.minimized).hash(&mut h);
        for &pane in &g.tabs {
            if let Some(content) = self.ws.content(pane) {
                (content.kind(), content.title()).hash(&mut h);
            }
        }
        if let Some(active) = g.active_pane()
            && let Some(content) = self.ws.content(active)
        {
            self.bodies.qualifier(active, content, cx).hash(&mut h);
            self.bodies
                .qualifier_warns(active, content, cx)
                .hash(&mut h);
            self.bodies
                .badge(active, content, cx)
                .map(|b| (b.text, b.tone as u8))
                .hash(&mut h);
            self.bodies.header_key(active, content, cx).hash(&mut h);
        }
        self.ws
            .focused()
            .is_some_and(|f| g.tabs.contains(&f))
            .hash(&mut h);
        self.ws.in_focus().hash(&mut h);
        self.accent_slot(g).hash(&mut h);
        self.mode
            .as_ref()
            .and_then(|m| m.labels.iter().find(|(_, id)| *id == g.id))
            .map(|(l, _)| l.to_string())
            .hash(&mut h);
        // The strip's width decides which tabs are drawn.
        self.sizes
            .borrow()
            .get(&g.id)
            .map(|b| f32::from(b.size.width).to_bits())
            .hash(&mut h);
        self.colors_generation.hash(&mut h);
        h.finish()
    }

    /// The cached view of `g`'s header, made anew when [`Shell::header_key`] changed.
    pub(super) fn header_view(&self, g: &Group, cx: &mut Context<Self>) -> AnyElement {
        // GPUI redraws the whole window, past every view cache, on each pointer move of a
        // drag (design/layout.md, Performance): a cached view would only add work then.
        if cx.has_active_drag() {
            return self.render_header(g, cx);
        }
        let key = self.header_key(g, cx);
        let mut map = self.headers.borrow_mut();
        let fresh = header_stale(map.get(&g.id).map(|slot| slot.key), key);
        if fresh {
            let (group, shell) = (g.id, cx.weak_entity());
            let view = cx.new(|_| PaneHeader { group, shell });
            map.insert(g.id, HeaderSlot { view, key });
        }
        let view = map[&g.id].view.clone();
        AnyView::from(view)
            .cached(
                StyleRefinement::default()
                    .w_full()
                    .h(px(HEADER))
                    .flex_none(),
            )
            .into_any_element()
    }

    pub(super) fn render_header(&self, g: &Group, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let group = g.id;
        let active = g.active_pane().expect("non-empty");
        let content = self.ws.content(active).expect("exists");
        let pinned = content.kind() == PaneKind::PinnedContext;
        let focused = self.ws.focused().is_some_and(|f| g.tabs.contains(&f));
        let in_focus = self.ws.in_focus();
        let slot = self.accent_slot(g);
        let single = g.tabs.len() == 1;
        let dragging = cx.has_active_drag();

        // During a drag GPUI applies no hover or active style, starts no new drag, shows no
        // tooltip and sends no click, so the pieces that only exist for those are left out:
        // no id (and so no element state), no handlers. Cursors and drop listeners stay.
        let grip = div()
            .flex_none()
            .cursor_grab()
            .child(small_icon("grip-vertical", c.icon_disabled));
        let grip = if dragging {
            grip.into_any_element()
        } else {
            grip.id(("grip", group.0))
                .on_drag(self.payload(Dragged::Group(group)), |p, at, _, cx| {
                    let p = p.clone();
                    cx.new(|_| DragChip(p, at))
                })
                .into_any_element()
        };

        let label = self.mode.as_ref().and_then(|m| {
            m.labels
                .iter()
                .find(|(_, id)| *id == group)
                .map(|(l, _)| l.to_string())
        });
        let badge = label.map(|l| {
            div()
                .flex_none()
                .h(px(18.))
                .min_w(px(18.))
                .px(px(4.))
                .flex()
                .justify_center()
                .items_center()
                .rounded(px(4.))
                .bg(c.text_accent)
                .text_color(c.editor)
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(l))
        });

        let mut strip = div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .overflow_hidden();
        if single {
            // One pane: the header itself is the only "tab".
            let title = div()
                .relative()
                .h_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .pr(px(6.))
                .min_w_0()
                .cursor_grab()
                .child(small_icon(
                    kind_icon(content.kind()),
                    if pinned || focused {
                        c.icon_accent
                    } else {
                        c.icon
                    },
                ))
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.text)
                        .child(SharedString::from(content.title())),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .text_color(if self.bodies.qualifier_warns(active, content, cx) {
                            c.warning.unwrap_or(c.text_accent)
                        } else {
                            c.text_muted
                        })
                        .child(SharedString::from(
                            self.bodies.qualifier(active, content, cx),
                        )),
                )
                .children(self.bodies.badge(active, content, cx).map(|b| {
                    let (fg, bg) = match b.tone {
                        BadgeTone::Neutral => (c.text_accent, c.accent_wash),
                        BadgeTone::Good => (
                            c.success.unwrap_or(c.text_accent),
                            c.success_bg.unwrap_or(c.accent_wash),
                        ),
                        BadgeTone::Bad => (
                            c.error.unwrap_or(c.text_accent),
                            c.error_bg.unwrap_or(c.accent_wash),
                        ),
                    };
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded_sm()
                        .whitespace_nowrap()
                        .text_size(px(10.5))
                        .text_color(fg)
                        .bg(bg)
                        .child(SharedString::from(b.text))
                }))
                .on_drag_move::<DragPayload>(self.tab_drop(group, 0, cx))
                .when(slot == Some(0), |d| d.child(self.accent_line(false)))
                .when(slot == Some(1), |d| d.child(self.accent_line(true)));
            let title = if dragging {
                title.into_any_element()
            } else {
                title
                    .id(("title", group.0))
                    .on_drag(self.payload(Dragged::Group(group)), |p, at, _, cx| {
                        let p = p.clone();
                        cx.new(|_| DragChip(p, at))
                    })
                    .into_any_element()
            };
            strip = strip.child(title);
        } else {
            let shown = self.visible_tabs(g);
            let hidden = g.tabs.len() - shown.len();
            for i in shown.clone() {
                let pane = g.tabs[i];
                let tc = self.ws.content(pane).expect("exists");
                let is_active = i == g.active;
                let tab = div()
                    .relative()
                    .h_full()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .border_r_1()
                    .border_color(c.border_variant)
                    .text_size(px(12.))
                    .when(is_active, |d| {
                        d.bg(c.tab_active)
                            .text_color(c.text)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!is_active, |d| {
                        d.bg(c.tab_inactive).text_color(c.text_muted)
                    })
                    // The active tab of the focused group: a 2 px text.accent bar on top.
                    .when(is_active && focused, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(0.))
                                .left(px(0.))
                                .right(px(0.))
                                .h(px(2.))
                                .bg(c.text_accent),
                        )
                    })
                    .child(small_icon(
                        kind_icon(tc.kind()),
                        if is_active && (tc.kind() != PaneKind::Editor) {
                            c.icon_accent
                        } else {
                            c.icon
                        },
                    ))
                    .child(SharedString::from(tc.title()))
                    .on_drag_move::<DragPayload>(self.tab_drop(group, i, cx))
                    .when(slot == Some(i), |d| d.child(self.accent_line(false)))
                    .when(slot == Some(shown.end) && i + 1 == shown.end, |d| {
                        d.child(self.accent_line(true))
                    });
                let tab = if dragging {
                    tab.into_any_element()
                } else {
                    tab.id(("tab", pane.0))
                        .when(!is_active, |d| d.hover(|d| d.bg(c.hover)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let _ = this.ws.activate(pane);
                            window.focus(&this.return_focus);
                            cx.notify();
                        }))
                        .on_drag(self.payload(Dragged::Pane(pane)), |p, at, _, cx| {
                            let p = p.clone();
                            cx.new(|_| DragChip(p, at))
                        })
                        .into_any_element()
                };
                // Divider between tool tabs and pins (layout.md, bottom bar).
                let pin_starts = i > 0
                    && tc.kind().is_pin()
                    && !self.ws.kind(g.tabs[i - 1]).is_some_and(PaneKind::is_pin);
                if pin_starts {
                    strip = strip.child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(c.border));
                }
                strip = strip.child(tab);
            }
            if hidden > 0 {
                // Tabs that do not fit are not drawn; the count says they exist.
                strip = strip.child(
                    div()
                        .flex_none()
                        .px(px(6.))
                        .text_size(px(11.))
                        .text_color(c.text_muted)
                        .child(SharedString::from(format!("+{hidden}"))),
                );
            }
        }
        // Empty space after the tabs: dropping there appends.
        let n = g.tabs.len();
        strip = strip.child(
            div()
                .id(("strip-end", group.0))
                .flex_1()
                .h_full()
                .on_drag_move::<DragPayload>(self.tab_drop(group, n, cx)),
        );

        let maximised_here = in_focus;
        let mut header = div()
            .id(("header", group.0))
            .h(px(HEADER))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .pl(px(4.))
            .pr(px(4.))
            .relative()
            .bg(if single { c.header } else { c.tab_bar })
            .border_b_1()
            .border_color(c.border)
            .children(badge)
            .child(grip)
            .child(strip)
            .children(self.bodies.header_actions(active, content, cx))
            .child(self.header_button(
                ("min", group.0),
                if g.minimized { "chevron-down" } else { "minus" },
                if g.minimized { "Expand" } else { "Minimise" },
                active,
                Box::new(MinimizePane),
                false,
                cx,
            ))
            .child(self.header_button(
                ("max", group.0),
                if maximised_here {
                    "minimize-2"
                } else {
                    "maximize-2"
                },
                if maximised_here {
                    "Exit focus"
                } else {
                    "Maximise"
                },
                active,
                Box::new(ToggleMaximizePane),
                maximised_here,
                cx,
            ))
            .child({
                let menu = div()
                    .size(px(20.))
                    .flex()
                    .justify_center()
                    .items_center()
                    .rounded(px(4.))
                    .child(small_icon("ellipsis", c.icon));
                if dragging {
                    // Bare during a drag, as in `header_button`.
                    menu.into_any_element()
                } else {
                    menu.id(("menu", group.0))
                        .hover(|d| d.bg(c.hover))
                        .active(|d| d.bg(c.ghost_active))
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                            cx.stop_propagation(); // not also a folded-header click
                            let _ = this.ws.activate(active);
                            let menu = Menu {
                                pane: active,
                                at: e.position(),
                            };
                            this.open_popup(Popup::Menu(menu), window, cx);
                        }))
                        .into_any_element()
                }
            });
        if pinned {
            // The pinned wash: text.accent, faint, over the header band.
            header = header
                .child(div().absolute().inset_0().bg(c.accent_faint))
                .relative();
            header = header.child(self.header_button(
                ("close", group.0),
                "x",
                "Unpin",
                active,
                Box::new(ClosePane),
                false,
                cx,
            ));
        }
        if g.minimized {
            header = header.on_click(cx.listener(move |this, _, _, cx| {
                let _ = this.ws.toggle_minimize(active);
                cx.notify();
            }));
        }
        header.into_any_element()
    }

    // Each argument is one property of the button; a struct would only rename them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn header_button(
        &self,
        id: (&'static str, u64),
        glyph: &'static str,
        label: &'static str,
        pane: PaneId,
        action: Box<dyn Action>,
        toggled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &self.c;
        // A ghost button: transparent at rest, ghost_element.hover / .active / .selected.
        let button = div()
            .flex_none()
            .size(px(20.))
            .flex()
            .justify_center()
            .items_center()
            .rounded(px(4.))
            .when(toggled, |d| d.bg(c.ghost_selected))
            .child(small_icon(glyph, if toggled { c.text } else { c.icon }));
        if cx.has_active_drag() {
            // During a drag GPUI applies no hover or active style, shows no tooltip and sends
            // no click, so the button is drawn bare: no id, hitbox, element state or handlers.
            return button.into_any_element();
        }
        let action = std::rc::Rc::new(action);
        button
            .id(id)
            .hover(|d| d.bg(c.hover))
            .active(|d| d.bg(c.ghost_active))
            .tooltip(tip(label, c))
            .on_click(cx.listener(move |this, _, window, cx| {
                // A folded header expands on any click; without this the click would bubble
                // there and undo the button's own toggle.
                cx.stop_propagation();
                this.dispatch_on(pane, action.boxed_clone(), window, cx);
            }))
            .into_any_element()
    }

    // ----- rendering: window regions ---------------------------------------------------------

    pub(super) fn dock_shown(&self, d: DockId) -> bool {
        let dock = self.ws.arrangement().dock(d);
        dock.visible && dock.root.is_some()
    }

    pub(super) fn render_dock(
        &self,
        d: DockId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dock = self.ws.arrangement().dock(d);
        let root = dock.root.as_ref().expect("shown docks have panes");
        div()
            .flex()
            .flex_none()
            .min_w_0()
            .min_h_0()
            .bg(self.c.panel)
            .when(d == DockId::Bottom, |e| e.h(px(dock.size)).w_full())
            .when(d != DockId::Bottom, |e| e.w(px(dock.size)).h_full())
            .child(self.render_node(root, window, cx))
            .into_any_element()
    }

    pub(super) fn dock_handle(&self, d: DockId) -> AnyElement {
        let c = &self.c;
        let row = d != DockId::Bottom;
        div()
            .id(SharedString::from(format!("dock-handle-{d:?}")))
            .flex_none()
            .flex()
            .justify_center()
            .items_center()
            .when(row, |e| {
                e.w(px(HANDLE)).h_full().flex_row().cursor_col_resize()
            })
            .when(!row, |e| {
                e.h(px(HANDLE)).w_full().flex_col().cursor_row_resize()
            })
            .child(
                div()
                    .bg(c.border)
                    .when(row, |e| e.w(px(1.)).h_full())
                    .when(!row, |e| e.h(px(1.)).w_full()),
            )
            .hover(|e| e.bg(c.hover))
            .on_drag(DockHandle(d), |_, _, _, cx| cx.new(|_| Nothing))
            .into_any_element()
    }

    /// While dragging, an empty or hidden dock shows a slim target to drop into.
    pub(super) fn dock_target(&self, d: DockId, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let key = self.preview_key(Target::Dock(d));
        let hot = self.hint == Some(Hint::Dock(d)) || key.is_some();
        let (glyph, label) = match d {
            DockId::Left => ("panel-left", "Left dock"),
            DockId::Right => ("panel-right", "Right dock"),
            DockId::Bottom => ("panel-bottom", "Bottom bar"),
        };
        div()
            .id(SharedString::from(format!("dock-target-{d:?}")))
            .flex_none()
            .flex()
            .justify_center()
            .items_center()
            .gap(px(6.))
            .m(px(3.))
            .rounded(px(4.))
            .border_1()
            .border_dashed()
            .border_color(if hot { c.text_accent } else { c.border })
            .when(hot, |e| e.bg(c.accent_wash))
            .text_size(px(11.))
            .text_color(c.text_muted)
            .when(d == DockId::Bottom, |e| e.h(px(36.)))
            .when(d != DockId::Bottom, |e| e.w(px(44.)).flex_col())
            .child(small_icon(glyph, c.icon))
            .when(d == DockId::Bottom, |e| e.child(label))
            .when_some(key, |e, k| e.child(self.arrow_cap(k)))
            .on_drag_move::<DragPayload>(cx.listener(
                move |this, e: &DragMoveEvent<DragPayload>, _, cx| {
                    let what = e.drag(cx).what;
                    if e.bounds.contains(&e.event.position) {
                        this.set_hint(what, Hint::Dock(d), cx);
                    } else {
                        this.clear_hint_if(|h| h == Hint::Dock(d), cx);
                    }
                },
            ))
            .into_any_element()
    }

    /// While dragging, a slim target along `edge` of the code area (or the focus page): a
    /// drop there spans that whole edge.
    fn area_target(&self, edge: Edge, cx: &mut Context<Self>) -> AnyElement {
        let c = &self.c;
        let key = self.preview_key(Target::AreaEdge(edge));
        let hot = self.hint == Some(Hint::AreaEdge(edge)) || key.is_some();
        // Outside focus the code area sits between the docks, so it isn't the window's width.
        let width = if self.ws.in_focus() {
            "Whole width"
        } else {
            "Code area width"
        };
        let (glyph, label) = match edge {
            Edge::Top => ("panel-top", width),
            Edge::Bottom => ("panel-bottom", width),
            Edge::Left => ("panel-left", ""),
            Edge::Right => ("panel-right", ""),
        };
        let row = matches!(edge, Edge::Top | Edge::Bottom);
        div()
            .id(SharedString::from(format!("area-target-{edge:?}")))
            .flex_none()
            .flex()
            .justify_center()
            .items_center()
            .gap(px(6.))
            .m(px(3.))
            .rounded(px(4.))
            .border_1()
            .border_dashed()
            .border_color(if hot { c.text_accent } else { c.border })
            .when(hot, |e| e.bg(c.accent_wash))
            .text_size(px(11.))
            .text_color(c.text_muted)
            .when(row, |e| e.h(px(24.)))
            .when(!row, |e| e.w(px(28.)).flex_col())
            .child(small_icon(glyph, c.icon))
            .when(row, |e| e.child(label))
            .when_some(key, |e, k| e.child(self.arrow_cap(k)))
            .on_drag_move::<DragPayload>(cx.listener(
                move |this, e: &DragMoveEvent<DragPayload>, _, cx| {
                    let what = e.drag(cx).what;
                    if e.bounds.contains(&e.event.position) {
                        this.set_hint(what, Hint::AreaEdge(edge), cx);
                    } else {
                        this.clear_hint_if(|h| h == Hint::AreaEdge(edge), cx);
                    }
                },
            ))
            .into_any_element()
    }

    /// The code area (or a focus page), with [`Self::area_target`] strips on its four sides
    /// while something is being dragged and there is something to span, and on the sides
    /// Layout mode's preview would span.
    pub(super) fn with_area_targets(
        &self,
        area: AnyElement,
        dragging: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A drag skips edges one group already spans: dropping beside it does the same.
        let show = |e: Edge| {
            (dragging && !self.ws.area_edge_is_one_group(e))
                || self.preview_key(Target::AreaEdge(e)).is_some()
        };
        if !Edge::ALL.into_iter().any(show) {
            return area;
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(show(Edge::Top), |d| {
                d.child(self.area_target(Edge::Top, cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .when(show(Edge::Left), |d| {
                        d.child(self.area_target(Edge::Left, cx))
                    })
                    .child(div().flex_1().min_w_0().flex().child(area))
                    .when(show(Edge::Right), |d| {
                        d.child(self.area_target(Edge::Right, cx))
                    }),
            )
            .when(show(Edge::Bottom), |d| {
                d.child(self.area_target(Edge::Bottom, cx))
            })
            .into_any_element()
    }

    /// The arrow that moves the pane to `target` in Layout mode's preview, if one does.
    fn preview_key(&self, target: Target) -> Option<&'static str> {
        self.previews
            .iter()
            .find(|(_, t)| *t == target)
            .map(|(dir, _)| arrow(*dir))
    }

    /// A key cap with an arrow on it, in a preview zone.
    fn arrow_cap(&self, key: &str) -> AnyElement {
        let c = &self.c;
        div()
            .flex_none()
            .px(px(6.))
            .py(px(1.))
            .rounded(px(4.))
            .border_1()
            .border_color(c.text_accent)
            .bg(c.panel)
            .text_color(c.text_accent)
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .child(SharedString::from(key.to_string()))
            .into_any_element()
    }

    pub(super) fn render_normal(
        &self,
        dragging: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &self.c;
        let a = self.ws.arrangement();
        let editor = match &a.editor {
            Some(root) => self.render_node(root, window, cx),
            None => div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .text_color(c.text_muted)
                .child("No file open")
                .into_any_element(),
        };
        let target = |d: DockId| {
            !self.dock_shown(d) && (dragging || self.preview_key(Target::Dock(d)).is_some())
        };
        div()
            .id("main")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .on_drag_move::<DockHandle>(cx.listener(
                |this, e: &DragMoveEvent<DockHandle>, _, cx| {
                    let DockHandle(d) = *e.drag(cx);
                    let b = e.bounds;
                    let p = e.event.position;
                    let size = match d {
                        DockId::Left => p.x - b.left(),
                        DockId::Right => b.right() - p.x,
                        DockId::Bottom => b.bottom() - p.y,
                    };
                    let _ = this.ws.resize_dock(d, f32::from(size));
                    this.dirty = true;
                    cx.notify();
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .when(target(DockId::Left), |e| {
                        e.child(self.dock_target(DockId::Left, cx))
                    })
                    .when(self.dock_shown(DockId::Left), |e| {
                        e.child(self.render_dock(DockId::Left, window, cx))
                            .child(self.dock_handle(DockId::Left))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .bg(c.editor)
                            .child(self.with_area_targets(
                                editor,
                                dragging && a.editor.is_some(),
                                cx,
                            )),
                    )
                    .when(self.dock_shown(DockId::Right), |e| {
                        e.child(self.dock_handle(DockId::Right))
                            .child(self.render_dock(DockId::Right, window, cx))
                    })
                    .when(target(DockId::Right), |e| {
                        e.child(self.dock_target(DockId::Right, cx))
                    }),
            )
            .when(self.dock_shown(DockId::Bottom), |e| {
                e.child(self.dock_handle(DockId::Bottom))
                    .child(self.render_dock(DockId::Bottom, window, cx))
            })
            .when(target(DockId::Bottom), |e| {
                e.child(self.dock_target(DockId::Bottom, cx))
            })
            .into_any_element()
    }

    /// Records the group's size at prepaint, for the next frame's visibility decisions.
    pub(super) fn measure(&self, group: NodeId) -> AnyElement {
        let sizes = self.sizes.clone();
        canvas(
            move |bounds, _, _| {
                sizes.borrow_mut().insert(group, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    /// Whether a group's body has room to show anything (unknown sizes count as visible).
    pub(super) fn body_visible(&self, group: NodeId) -> bool {
        self.sizes
            .borrow()
            .get(&group)
            .is_none_or(|b| b.size.height > px(HEADER + 12.))
    }

    /// The tabs to draw: a run around the active tab that fits the strip's width as last
    /// measured (tab widths estimated from their titles), so a 50-tab strip draws only what
    /// can be seen.
    pub(super) fn visible_tabs(&self, g: &Group) -> std::ops::Range<usize> {
        const CHROME: f32 = 96.; // grip and the three header buttons
        let avail = self
            .sizes
            .borrow()
            .get(&g.id)
            .map_or(f32::MAX, |b| f32::from(b.size.width) - CHROME);
        let width = |i: usize| {
            let title = self
                .ws
                .content(g.tabs[i])
                .map_or(0, |c| c.title().chars().count());
            36. + 6.5 * title as f32
        };
        // Keep adding tabs while there is room left; the last one may be cut off by the
        // strip's clip, which is fine. Tabs past it are not drawn at all.
        let (mut lo, mut hi) = (g.active, g.active + 1);
        let mut used = width(g.active);
        loop {
            let mut grew = false;
            if hi < g.tabs.len() && used < avail {
                used += width(hi);
                hi += 1;
                grew = true;
            }
            if lo > 0 && used < avail {
                used += width(lo - 1);
                lo -= 1;
                grew = true;
            }
            if !grew {
                return lo..hi;
            }
        }
    }
}

/// A key chip (`Esc`, bindings in menus): element.background, muted mono text.
pub(super) fn key_chip(c: &Colors, k: &str) -> AnyElement {
    crate::widgets::key_chip(c, k)
}

pub(super) fn tip(
    text: &'static str,
    c: &Colors,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let c = c.clone();
    move |_, cx| cx.new(|_| Tip(text.into(), c.clone())).into()
}

/// [`tip`] for text made at run time.
pub(super) fn tip_owned(
    text: SharedString,
    c: &Colors,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let c = c.clone();
    move |_, cx| cx.new(|_| Tip(text.clone(), c.clone())).into()
}

pub(super) struct Tip(SharedString, Colors);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let c = &self.1;
        div()
            .px(px(6.))
            .py(px(2.))
            .rounded(px(4.))
            .bg(c.elevated)
            .border_1()
            .border_color(c.border)
            .font_family(c.ui_font.clone())
            .text_size(px(11.))
            .text_color(c.text)
            .child(self.0.clone())
    }
}

impl Render for Shell {
    /// Everything but the status bar. Frame timing starts here: this is the big subtree.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.stats.borrow_mut().begin();
        self.settle_switcher(window, cx);
        if std::mem::take(&mut self.dirty)
            && self.autosave
            && let Some(saver) = &self.saver
        {
            let _t = self.metrics.as_ref().map(|(m, _, id)| m.start(*id));
            saver.changed(&self.ws);
        }
        // A failed save shows in the status bar once (the host redraws it when notified).
        if let Some(text) = self.saver.as_ref().and_then(Saver::take_failure) {
            self.notice = Some(text);
            cx.notify();
        }
        {
            let ws = &self.ws;
            self.pane_bodies
                .borrow_mut()
                .retain(|id, _| ws.content(*id).is_some());
            self.headers
                .borrow_mut()
                .retain(|id, _| ws.group(*id).is_some());
        }
        let dragging = cx.has_active_drag();
        if !dragging {
            self.hint = None;
            self.dragged = None;
        }
        self.previews = self.arrow_previews();
        if std::mem::take(&mut self.preview_stale) && !self.previews.is_empty() {
            // This frame's preview used the sizes from before the move: draw once more.
            let shell = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = shell.update(cx, |_, cx| cx.notify());
            });
        }
        let c = self.c.clone();
        let in_focus = self.ws.in_focus();
        let root = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .flex()
            .flex_col()
            .bg(c.panel)
            .font_family(c.ui_font.clone())
            .text_color(c.text)
            .text_size(px(12.5))
            .on_drop(cx.listener(|this, p: &DragPayload, _, cx| this.drop_payload(p, cx)))
            // A drag (resize or move) ends with the button: the next one is a new undo step.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.ws.seal_undo()),
            )
            .child(self.input_probe());
        let main = if in_focus {
            let root = self.ws.focus().expect("in focus").root().cloned();
            let side = self.bodies.clone().focus_side(&self.ws, window, cx);
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .bg(c.editor)
                .children(side)
                .child(match root {
                    Some(r) => {
                        let page = self.render_node(&r, window, cx);
                        self.with_area_targets(page, dragging, cx)
                    }
                    None => self.render_empty_page(cx),
                })
                .into_any_element()
        } else {
            self.render_normal(dragging, window, cx)
        };
        let tree = root
            .when(in_focus, |d| d.child(self.render_focus_bar(window, cx)))
            .child(main)
            .when(in_focus, |d| d.child(self.render_tray(cx)))
            .children(match &self.popup {
                Popup::None => None,
                Popup::Menu(m) => Some(self.render_menu(m, window, cx)),
                Popup::History => Some(self.render_history(cx)),
                Popup::Overlay(_) => self.render_overlay(window, cx),
            })
            .when(self.show_stats, |d| {
                let _t = self.metrics.as_ref().map(|(m, id, _)| m.start(*id));
                d.child(self.render_stats(window))
            })
            .child(self.frame_end());
        self.stats.borrow_mut().built();
        tree.into_any_element()
    }
}

/// The arrow key for a direction, as Layout mode's preview shows it.
fn arrow(dir: Direction) -> &'static str {
    match dir {
        Direction::Left => "←",
        Direction::Right => "→",
        Direction::Up => "↑",
        Direction::Down => "↓",
    }
}

/// Whether a cached header (`cached`: the key it was drawn for) must be drawn anew for `key`.
fn header_stale(cached: Option<u64>, key: u64) -> bool {
    cached != Some(key)
}

#[cfg(test)]
mod header_tests {
    use super::header_stale;
    use std::hash::{Hash, Hasher};

    fn key(host: u64) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ("lib.rs", "Rust", Some(("After", 1u8))).hash(&mut h);
        host.hash(&mut h);
        h.finish()
    }

    #[test]
    fn a_changed_host_key_redraws_the_header() {
        // The host key goes into the header key alongside the title, qualifier and badge.
        assert!(header_stale(Some(key(0)), key(1)));
        assert!(!header_stale(Some(key(7)), key(7)));
        assert!(header_stale(None, key(0)));
    }
}

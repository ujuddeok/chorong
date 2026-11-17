//! The fuzzy pickers (files, symbols, commands) and their popup.

use super::shell_host::OpenAt;
use super::*;

impl Chorong {
    pub(super) fn open_picker(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.key_focus.leave(KeyFocus::Search);
        self.picker = Some(PickerState {
            kind: PickerKind::Files,
            query: String::new(),
            query_all: false,
            add_to_page: false,
            tour_scope: self.tour_open(),
            peek_open: false,
            matches: Vec::new(),
            selected: 0,
            labels: Vec::new(),
            hints: Vec::new(),
            picks: Vec::new(),
        });
        self.rerank();
    }

    /// The indices into `files` of the open tour's files, for Go to file…'s Tour chip.
    fn tour_file_indices(&self) -> Vec<usize> {
        let Some(st) = self.inv.as_ref().filter(|_| self.tour_open()) else {
            return Vec::new();
        };
        let tour: HashSet<String> = crate::investigation::tour_files(&st.steps)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        (0..self.files.len())
            .filter(|&i| {
                let rel = self.files[i].to_string_lossy().replace('\\', "/");
                tour.contains(&rel)
            })
            .collect()
    }

    /// Alt-T in Go to file…: the Tour chip on or off (while a tour is open).
    pub(super) fn toggle_picker_tour_scope(&mut self) {
        let can = self.tour_open();
        let Some(p) = &mut self.picker else { return };
        if p.kind != PickerKind::Files || p.add_to_page || !can {
            return;
        }
        p.tour_scope = !p.tour_scope;
        self.rerank();
    }

    /// With the Tour chip off while a tour is open: the rest of "tour" when the query is its
    /// start (`Some("tour")` for an empty query), for the hint and for Tab.
    fn tour_completion(&self, p: &PickerState) -> Option<&'static str> {
        if p.kind != PickerKind::Files || p.add_to_page || p.tour_scope || !self.tour_open() {
            return None;
        }
        let q = p.query.trim().to_lowercase();
        "tour".strip_prefix(q.as_str())
    }

    /// Tab in Go to file…: puts the Tour chip back when the query is empty or the start of
    /// "tour".
    pub(super) fn picker_tab(&mut self) {
        let Some(p) = &self.picker else { return };
        if self.tour_completion(p).is_none() {
            return;
        }
        if let Some(p) = &mut self.picker {
            p.query.clear();
            p.query_all = false;
            p.tour_scope = true;
        }
        self.rerank();
    }

    /// The Tour chip before Go to file…'s query: the list is the tour's files.
    fn tour_scope_chip(
        &self,
        picker: &PickerState,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !picker.tour_scope {
            return None;
        }
        let c = crate::pane_style::colors(&self.theme, cx);
        Some(
            cho_layout_ui::widgets::mode_chip(&c, "tour-scope-chip", "map", "Tour")
                .mr_2()
                .tooltip(tip(
                    "Only the tour's files: Backspace or click for every file (Alt-T toggles)"
                        .into(),
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_picker_tour_scope();
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// A key binding as separate caps: `⇧⌘P` is ⇧ ⌘ P, `⌘K L` is ⌘ K, then L, and
    /// `Ctrl-Shift-P` is Ctrl Shift P. Chord steps are spaced a little further apart.
    fn key_caps(&self, binding: &str) -> gpui::Div {
        let t = &self.theme;
        let cap = |k: String| {
            div()
                .flex_none()
                .h(px(18.))
                .min_w(px(18.))
                .px(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_color(hsla(t.border))
                .text_size(px(11.))
                .text_color(hsla(t.text_muted))
                .child(k)
        };
        div()
            .flex_none()
            .pl_3()
            .flex()
            .items_center()
            .gap(px(8.))
            .children(binding_steps(binding).into_iter().map(|step| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .children(step.into_iter().map(cap))
            }))
    }

    /// A query field's text, with the selection colour behind it when it is all selected.
    pub(super) fn query_text(&self, query: &str, all: bool) -> gpui::Div {
        let selection = self.theme.players.first().map(|p| p.selection);
        div()
            .rounded_sm()
            .when_some(selection.filter(|_| all), |d, c| d.bg(hsla(c)))
            .child(query.to_string())
    }

    /// The file picker for building a focus page: each file chosen opens in a new code pane
    /// on the current page, and the picker stays open for the next one.
    pub(super) fn open_add_to_page_picker(&mut self) {
        self.open_picker();
        if let Some(p) = &mut self.picker {
            p.add_to_page = true;
        }
    }

    /// Whether Backspace on an empty add-to-page query (or the Files chip) can step back to
    /// Bring from tray…: the tray has something to bring.
    pub(super) fn tray_to_return_to(&self, cx: &App) -> bool {
        !self.shell.read(cx).workspace().tray().is_empty()
    }

    /// Back from the add-to-page file picker to Bring from tray….
    pub(super) fn back_to_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self.shell
            .update(cx, |shell, cx| shell.open_tray_picker(window, cx));
        cx.notify();
    }

    /// The Files chip before the add-to-page query, when the tray can be gone back to.
    fn files_mode_chip(
        &self,
        picker: &PickerState,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !picker.add_to_page || !self.tray_to_return_to(cx) {
            return None;
        }
        let c = crate::pane_style::colors(&self.theme, cx);
        Some(
            cho_layout_ui::widgets::mode_chip(&c, "files-mode-chip", "file", "Files")
                .mr_2()
                .tooltip(tip(
                    "Adding files: Backspace or click to bring from the tray".into(),
                ))
                .on_click(cx.listener(|this, _, window, cx| this.back_to_tray(window, cx)))
                .into_any_element(),
        )
    }

    /// The highlighted file of the add-to-page picker, in a new code pane at `edge` of the
    /// focused one. The picker stays open with an empty query.
    fn add_file_to_page(&mut self, place: OpenAt, cx: &mut Context<Self>) {
        let Some(rel) = self.picker.as_ref().and_then(|p| {
            let m = p.matches.get(p.selected)?;
            self.files.get(m.index).cloned()
        }) else {
            return;
        };
        let path = self.root.clone().unwrap_or_default().join(rel);
        self.open_at(path, place, cx);
        if let Some(p) = &mut self.picker {
            p.query.clear();
            p.query_all = false;
        }
        self.requery(cx);
        cx.notify();
    }

    /// Confirms the highlighted row at `place`: a file, or a symbol of this file, opens in a
    /// new code pane there. The add-to-page picker adds at `place` and stays open; other
    /// pickers confirm as Enter does.
    fn picker_place(&mut self, place: OpenAt, window: &mut Window, cx: &mut Context<Self>) {
        self.compare_confirm_typed();
        let Some(p) = &self.picker else { return };
        if p.add_to_page {
            return self.add_file_to_page(place, cx);
        }
        let selected = p.selected;
        let chosen = p.matches.get(selected).and_then(|m| match p.kind {
            PickerKind::Files => self.files.get(m.index).map(|rel| (rel.clone(), None)),
            PickerKind::Symbols => match p.picks.get(m.index) {
                Some(Pick::Symbol(byte)) => {
                    let doc = self.ed().doc.as_ref()?;
                    let root = self.root.clone().unwrap_or_default();
                    let rel = doc.path.strip_prefix(&root).ok()?.to_path_buf();
                    Some((rel, Some(*byte)))
                }
                _ => None,
            },
            _ => None,
        });
        let Some((rel, byte)) = chosen else {
            return self.picker_open(selected, window, cx);
        };
        self.picker = None;
        self.prepare_place(place, cx);
        let path = self.root.clone().unwrap_or_default().join(rel);
        let target = Target {
            byte,
            ..Target::default()
        };
        self.open(path, target, true, cx);
    }

    pub(super) fn requery(&mut self, cx: &mut Context<Self>) {
        self.compare_picker_query(cx);
        self.rerank();
    }

    /// Ranks the picker's rows for its query again (`requery` without the Compare picker's
    /// typed-commit lookup, for Go to file…).
    fn rerank(&mut self) {
        let scoped = self
            .picker
            .as_ref()
            .is_some_and(|p| p.kind == PickerKind::Files && p.tour_scope);
        let tour = if scoped {
            self.tour_file_indices()
        } else {
            Vec::new()
        };
        let Some(p) = &mut self.picker else { return };
        p.matches = if scoped {
            // Ranked among the tour's files, then mapped back to indices into `files`.
            let names: Vec<String> = tour.iter().map(|&i| self.file_names[i].clone()).collect();
            let mut m = self.ranker.rank(&names, &p.query, MAX_RESULTS);
            for x in &mut m {
                x.index = tour[x.index];
            }
            m
        } else {
            let candidates: &[String] = match p.kind {
                PickerKind::Files => &self.file_names,
                _ => &p.labels,
            };
            self.ranker.rank(candidates, &p.query, MAX_RESULTS)
        };
        p.selected = 0;
        self.picker_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    pub(super) fn picker_move(&mut self, delta: isize) {
        let Some(p) = &mut self.picker else { return };
        if p.matches.is_empty() {
            return;
        }
        let last = p.matches.len() - 1;
        p.selected = p.selected.saturating_add_signed(delta).min(last);
        let strategy = step_strategy(delta);
        self.picker_scroll.scroll_to_item(p.selected, strategy);
    }

    pub(super) fn picker_open(
        &mut self,
        result: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.compare_defer_confirm() {
            return;
        }
        let Some(p) = self.picker.take() else { return };
        let Some(m) = p.matches.get(result) else {
            return;
        };
        match p.kind {
            PickerKind::Files => {
                if let Some(rel) = self.files.get(m.index) {
                    let path = self.root.clone().unwrap_or_default().join(rel);
                    self.open(path, Target::default(), true, cx);
                }
            }
            PickerKind::Compare => {
                if let Some(Pick::Ref { label, rev }) = p.picks.get(m.index) {
                    self.choose_comparison(label.clone(), rev.clone(), cx);
                }
            }
            PickerKind::Recent => {
                if let Some(Pick::Folder(folder)) = p.picks.get(m.index) {
                    // The window that shows this folder comes forward, else a new one opens.
                    crate::send_link(folder.to_string_lossy().into_owned(), cx);
                }
            }
            PickerKind::Investigations => match p.picks.get(m.index) {
                Some(Pick::Investigation(source)) => self.open_investigation(source.clone(), cx),
                Some(Pick::InvestigationFile) => self.open_investigation_file(cx),
                Some(Pick::Unavailable(why)) => self.notify_text(why.clone(), cx),
                _ => {}
            },
            PickerKind::Symbols => {
                if let Some(Pick::Symbol(byte)) = p.picks.get(m.index) {
                    self.jump_to_byte(*byte, cx);
                }
            }
            PickerKind::Palette => {
                if let Some(Pick::Command(action)) = p.picks.get(m.index) {
                    let action = action.boxed_clone();
                    // Run once the palette has closed and focus is back in the code view.
                    cx.defer_in(window, move |_, window, cx| {
                        window.dispatch_action(action, cx);
                    });
                }
            }
        }
    }

    /// Go to file…'s footer: the keys from the live keymap, with the Tour chip's while a tour
    /// is open.
    fn files_footer(
        &self,
        picker: &PickerState,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if picker.kind != PickerKind::Files || picker.add_to_page {
            return None;
        }
        let ctx = "Chorong Picker";
        let key = |a: &dyn Action| cho_layout_ui::keys::label_in(a, window, ctx);
        let mut items = vec![
            (key(&PickerConfirm), "open".to_string()),
            (key(&PickerOpenBeside), "right".to_string()),
            (key(&PickerOpenBelow), "below".to_string()),
            (key(&PickerOpenTab), "tab".to_string()),
        ];
        if self.features.on(Feature::PickerPreview) {
            items.push(if picker.peek_open {
                (key(&PickerPeekClose), "close preview".to_string())
            } else {
                (key(&PickerPeekOpen), "preview".to_string())
            });
        }
        if self.tour_open() {
            let what = if picker.tour_scope {
                "every file"
            } else {
                "tour files"
            };
            let k = match (key(&PickerTab), key(&PickerToggleTour)) {
                // Tab only puts the chip back; Backspace takes it off.
                (Some(t), Some(a)) if !picker.tour_scope => Some(format!("{t} / {a}")),
                (_, a) => a,
            };
            items.push((k, what.to_string()));
        }
        items.push((key(&PickerClose), "close".to_string()));
        let c = crate::pane_style::colors(&self.theme, cx);
        Some(cho_layout_ui::widgets::hint_footer(&c, items))
    }

    pub(super) fn render_picker(
        &self,
        picker: &PickerState,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let (muted, selected_bg) = (hsla(t.text_muted), hsla(t.element_selected));
        let accent = hsla(t.players.first().map_or(t.text, |p| p.cursor));
        let visible = picker.matches.len().min(PICKER_VISIBLE_ROWS);
        // Go to file…'s preview column, while it is open (→).
        let peek = (picker.kind == PickerKind::Files && picker.peek_open).then(|| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .children(self.render_peek())
        });
        let tall = peek.is_some();
        div()
            .id("picker-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            // Modal: the panes behind take no hover or scroll (the preview scrolls, not the
            // code under it).
            .occlude()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(70.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.picker = None;
                    cx.notify();
                }),
            )
            .child(
                // The list, and beside it the preview of the highlighted file
                // (`picker.preview`); never wider than the window.
                div()
                    // With the preview open: wider, and 70% of the window tall, so the
                    // preview shows a real piece of the file.
                    .when(tall, |d| {
                        d.w(px(PICKER_WIDTH + PEEK_WIDTH))
                            .max_w(gpui::relative(0.95))
                            .h(gpui::relative(0.7))
                    })
                    .flex()
                    .bg(hsla(t.panel_background))
                    .border_1()
                    .border_color(hsla(t.border))
                    .rounded_md()
                    .shadow_lg()
                    .overflow_hidden()
                    .text_size(px(13.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .w(px(PICKER_WIDTH))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .when(tall, |d| d.h_full())
                            .child(
                                div()
                                    .flex_none()
                                    .h(px(44.))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .text_size(px(14.))
                                    .border_b_1()
                                    .border_color(hsla(t.border))
                                    // Adding to a page from the tray's Files mode: the mode as a chip
                                    // before the query. Backspace on an empty query, or a click on it,
                                    // goes back to the tray.
                                    .children(self.files_mode_chip(picker, cx))
                                    .children(self.tour_scope_chip(picker, cx))
                                    .when(picker.query.is_empty(), |d| {
                                        d.child(div().text_color(muted).child(picker.placeholder()))
                                    })
                                    .child(self.query_text(&picker.query, picker.query_all))
                                    .child(div().w(px(1.5)).h(px(16.)).bg(hsla(t.text)))
                                    // The Tour chip is off: typing the start of "tour" shows the rest,
                                    // and Tab puts the chip back.
                                    .children(self.tour_completion(picker).map(|rest| {
                                        let text = if picker.query.trim().is_empty() {
                                            "\u{21e5} tour".to_string()
                                        } else {
                                            format!("{rest}  \u{21e5} tour")
                                        };
                                        div()
                                            .flex_none()
                                            .when(picker.query.trim().is_empty(), |d| {
                                                d.ml_auto().pl_3()
                                            })
                                            .text_color(muted)
                                            .child(text)
                                    })),
                            )
                            .when(picker.matches.is_empty(), |d| {
                                d.child(div().p_3().text_color(muted).child("No matches"))
                            })
                            .child(
                                crate::scroll::uniform_list(
                                    "picker",
                                    picker.matches.len(),
                                    cx.processor(
                                        move |this, range: std::ops::Range<usize>, _w, cx| {
                                            let Some(p) = &this.picker else {
                                                return Vec::new();
                                            };
                                            range
                                                .map(|ix| {
                                                    let m = &p.matches[ix];
                                                    let name = match p.kind {
                                                        PickerKind::Files => {
                                                            this.file_names[m.index].clone()
                                                        }
                                                        _ => p.labels[m.index].clone(),
                                                    };
                                                    let hint = p
                                                        .hints
                                                        .get(m.index)
                                                        .cloned()
                                                        .unwrap_or_default();
                                                    let files = p.kind == PickerKind::Files;
                                                    let highlights = m
                                                        .ranges
                                                        .iter()
                                                        .map(|r| {
                                                            (
                                                                r.clone(),
                                                                HighlightStyle {
                                                                    color: Some(accent),
                                                                    font_weight: Some(
                                                                        FontWeight::BOLD,
                                                                    ),
                                                                    ..Default::default()
                                                                },
                                                            )
                                                        })
                                                        .collect::<Vec<_>>();
                                                    let palette = p.kind == PickerKind::Palette;
                                                    div()
                                            .id(("result", ix))
                                            .w_full()
                                            .h(px(PICKER_ROW))
                                            .px_3()
                                            .rounded_md()
                                            .flex()
                                            .items_center()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .cursor_pointer()
                                            .when(ix == p.selected, |d| d.bg(selected_bg))
                                            .hover(|s| s.bg(selected_bg))
                                            .child(
                                                div().flex_1().min_w_0().overflow_hidden().child(
                                                    StyledText::new(name)
                                                        .with_highlights(highlights),
                                                ),
                                            )
                                            .when(palette && !hint.is_empty(), |d| {
                                                d.child(this.key_caps(&hint))
                                            })
                                            .when(!files && !palette && !hint.is_empty(), |d| {
                                                d.child(
                                                    div()
                                                        .flex_none()
                                                        .pl_3()
                                                        .text_size(px(12.))
                                                        .text_color(muted)
                                                        .child(hint),
                                                )
                                            })
                                            .on_click(cx.listener(
                                                move |this, e: &gpui::ClickEvent, window, cx| {
                                                    // Adding to a page, a click adds to the right as
                                                    // Enter does (the picker stays open); elsewhere
                                                    // Alt-click opens to the right, Alt-Shift below.
                                                    let place = OpenAt::of_click(&e.modifiers());
                                                    let adding = this
                                                        .picker
                                                        .as_ref()
                                                        .is_some_and(|p| p.add_to_page);
                                                    if let Some(p) = &mut this.picker {
                                                        p.selected = ix;
                                                    }
                                                    match (adding, place) {
                                                        (true, OpenAt::Here) => {
                                                            this.add_file_to_page(OpenAt::Right, cx)
                                                        }
                                                        (true, place) => {
                                                            this.add_file_to_page(place, cx)
                                                        }
                                                        (false, OpenAt::Here) => {
                                                            this.picker_open(ix, window, cx)
                                                        }
                                                        (false, place) => {
                                                            this.picker_place(place, window, cx)
                                                        }
                                                    }
                                                },
                                            ))
                                                })
                                                .collect()
                                        },
                                    ),
                                )
                                .map(|l| l.px(px(PICKER_INSET)))
                                .wrap(
                                    &self.picker_over,
                                    &self.picker_scroll,
                                    self.features.probe_if_on(Feature::Overscroll),
                                )
                                .w_full()
                                .when(!tall, |d| {
                                    d.h(px(visible as f32 * PICKER_ROW + 2. * PICKER_INSET))
                                })
                                // Tall: the list takes the height the preview needs.
                                .when(tall, |d| d.flex_1().min_h_0())
                                .py(px(PICKER_INSET)),
                            )
                            .children(self.files_footer(picker, window, cx)),
                    )
                    .children(peek),
            )
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        OpenPicker,
        PickerPrev,
        PickerNext,
        PickerConfirm,
        // Shift-Enter in the add-to-page file picker: the file goes below, not to the right.
        PickerConfirmBelow,
        PickerOpenBeside,
        PickerOpenBelow,
        PickerOpenTab,
        PickerClose,
        PickerToggleTour,
        PickerTab,
        PickerPeekOpen,
        PickerPeekClose,
    ]
);

pub(super) const COMMANDS: &[Command] = &[c("chorong::OpenPicker", "Go to File", Layer::Go, 10)];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, OpenPicker, |this, _w, cx| this.open_picker());
    let d = on!(d, cx, PickerPrev, |this, _w, cx| this.picker_move(-1));
    let d = on!(d, cx, PickerToggleTour, |this, _w, cx| this
        .toggle_picker_tour_scope());
    let d = on!(d, cx, PickerTab, |this, _w, cx| this.picker_tab());
    let d = on!(d, cx, PickerPeekOpen, |this, _w, cx| this
        .set_picker_peek(true));
    let d = on!(d, cx, PickerPeekClose, |this, _w, cx| this
        .set_picker_peek(false));
    let d = on!(d, cx, PickerNext, |this, _w, cx| this.picker_move(1));
    let d = on!(d, cx, PickerConfirmBelow, |this, _w, cx| this
        .add_file_to_page(OpenAt::Below, cx));
    // Cmd-Enter, Cmd-Shift-Enter and Alt-Enter: the file (or symbol) in a new code pane to
    // the right, below, or as a new tab (docs/public/user-guide.md, "Where files open").
    let d = on!(d, cx, PickerOpenBeside, |this, window, cx| this
        .picker_place(OpenAt::Right, window, cx));
    let d = on!(d, cx, PickerOpenBelow, |this, window, cx| this
        .picker_place(OpenAt::Below, window, cx));
    let d = on!(d, cx, PickerOpenTab, |this, window, cx| this.picker_place(
        OpenAt::Tab,
        window,
        cx
    ));
    let d = on!(d, cx, PickerConfirm, |this, window, cx| {
        if this.picker.as_ref().is_some_and(|p| p.add_to_page) {
            return this.add_file_to_page(OpenAt::Right, cx);
        }
        this.compare_confirm_typed();
        if let Some(selected) = this.picker.as_ref().map(|p| p.selected) {
            this.picker_open(selected, window, cx);
        }
    });
    on!(d, cx, PickerClose, |this, _w, cx| this.picker = None)
}

/// A binding label split into chord steps and, within each, its keys: macOS modifier
/// symbols one each (`⇧⌘P` → ⇧ ⌘ P), elsewhere at the dashes (`Ctrl-Shift-P`, and
/// `Ctrl--` for the minus key). Alternatives (`⌘P, ⌃⌘N`) keep only the first.
fn binding_steps(binding: &str) -> Vec<Vec<String>> {
    const MAC: [char; 5] = ['⌃', '⌥', '⇧', '⌘', '\u{1F310}'];
    let first = binding.split(", ").next().unwrap_or_default();
    first
        .split(' ')
        .filter(|s| !s.is_empty())
        .map(|step| {
            if step.chars().any(|c| MAC.contains(&c)) {
                let mut keys: Vec<String> = Vec::new();
                let mut rest = String::new();
                for c in step.chars() {
                    if MAC.contains(&c) && rest.is_empty() {
                        keys.push(c.to_string());
                    } else {
                        rest.push(c);
                    }
                }
                if !rest.is_empty() {
                    keys.push(rest);
                }
                keys
            } else {
                let mut keys: Vec<String> = Vec::new();
                for part in step.split('-') {
                    match (part.is_empty(), keys.last()) {
                        // `Ctrl--`: the empty piece after a dash is the minus key itself.
                        (true, Some(_)) if !keys.iter().any(|k| k == "-") => keys.push("-".into()),
                        (true, _) => {}
                        (false, _) => keys.push(part.to_string()),
                    }
                }
                if keys.is_empty() {
                    keys.push(step.to_string());
                }
                keys
            }
        })
        .collect()
}

#[cfg(test)]
mod key_caps_tests {
    use super::binding_steps;

    fn steps(b: &str) -> Vec<Vec<String>> {
        binding_steps(b)
    }

    fn v(steps: &[&[&str]]) -> Vec<Vec<String>> {
        steps
            .iter()
            .map(|s| s.iter().map(|k| k.to_string()).collect())
            .collect()
    }

    #[test]
    fn splits_mac_and_other_labels() {
        assert_eq!(steps("⇧⌘P"), v(&[&["⇧", "⌘", "P"]]));
        assert_eq!(steps("⌘K L"), v(&[&["⌘", "K"], &["L"]]));
        assert_eq!(steps("⌘K ⌘Left"), v(&[&["⌘", "K"], &["⌘", "Left"]]));
        assert_eq!(steps("Ctrl-Shift-P"), v(&[&["Ctrl", "Shift", "P"]]));
        assert_eq!(steps("Ctrl--"), v(&[&["Ctrl", "-"]]));
        assert_eq!(steps("F12"), v(&[&["F12"]]));
        assert_eq!(
            steps("⌘P, ⌃⌘N"),
            v(&[&["⌘", "P"]]),
            "first of the alternatives"
        );
    }
}

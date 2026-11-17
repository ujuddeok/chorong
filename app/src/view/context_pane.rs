//! The context pane: hover answers, definitions and the reference list. It is always live;
//! pinning makes a pane of its own (`pins.rs`).

use super::shell_host::OpenAt;
use super::*;
use cho_layout_ui::colors::{Colors, kind_glyph};
use cho_layout_ui::widgets::small_icon;
use std::ops::Range;
use std::rc::Rc;

/// Highlighted reference previews kept (a few screens of many symbols).
const PREVIEW_CACHE: usize = 4000;

/// One row of the context list after the head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RefRow {
    /// A file's header: its references follow.
    File(usize),
    /// A reference line; `id` counts the definitions first, as the keyboard selection does.
    Place {
        file: usize,
        place: usize,
        id: usize,
    },
    /// "… and N more not listed."
    Omitted(usize),
}

/// Syntax spans over a preview line, as theme keys.
pub(super) type Spans = Rc<Vec<(Range<usize>, &'static str)>>;

/// Reference previews' syntax spans by (path, line, text), and the lines waiting for them.
#[derive(Default)]
pub(super) struct PreviewSpans {
    done: std::collections::HashMap<(String, usize, String), Spans>,
    queued: Vec<(String, usize, String)>,
    busy: bool,
}

impl Chorong {
    /// Looks up the identifier under the caret in the index, off the UI thread. Called
    /// whenever the caret may have moved; does nothing while it stays on the same identifier.
    pub(super) fn refresh_context(&mut self, cx: &mut Context<Self>) {
        if self.pane_override {
            return;
        }
        let key = self.focus_key();
        if key == self.context.key {
            return;
        }
        let _t = self.features.time(if self.hover.active().is_some() {
            Feature::FollowHover
        } else {
            Feature::FollowCaret
        });
        let references = self
            .features
            .on(Feature::References)
            .then(|| self.features.probe(Feature::References));
        self.context.key = key.clone();
        self.context.lookup = Lookup::Idle;
        self.context.generation.next();
        let path = self.ed().doc.as_ref().map(|d| d.display_path.clone());
        let sem = self.doc_index().cloned();
        let (Some((_, start)), Some(sem), Some(root), Some(path)) =
            (key, sem, self.root.clone(), path)
        else {
            return;
        };
        self.context.lookup = Lookup::Loading;
        let (sem, generation) = (sem.clone(), self.context.generation);
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_spawn(async move {
                    context::gather(&sem, &root, &path, start, references.as_ref())
                })
                .await;
            this.update(cx, |this, cx| {
                if !this.context.generation.is(generation) {
                    return;
                }
                this.context.lookup = Lookup::Done(answer);
                this.refresh_context_panes(cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn place_row(
        &self,
        id: usize,
        place: &Place,
        show_path: bool,
        selected: bool,
        mark_col: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let (path, byte) = (place.path.clone(), place.byte);
        // Syntax colours from the theme, as in the code view (the spans were computed off
        // the UI thread); the occurrence itself in its kind colour, Medium, on top.
        // References are coloured once on screen (`preview_spans`); definitions in the gather.
        let lazy = (!show_path && place.spans.is_empty())
            .then(|| self.preview_spans(place, cx))
            .flatten();
        let spans = lazy
            .as_deref()
            .map_or(place.spans.as_slice(), Vec::as_slice);
        let runs = preview_runs(spans, place.mark.clone())
            .into_iter()
            .filter_map(|(r, key)| {
                let style = match key {
                    Some(k) => {
                        let s = self.theme.syntax_style(k)?;
                        HighlightStyle {
                            color: s.color.map(crate::document::hsla),
                            font_style: s.font_style.map(|f| match f {
                                cho_theme::FontStyle::Italic => gpui::FontStyle::Italic,
                                cho_theme::FontStyle::Oblique => gpui::FontStyle::Oblique,
                                cho_theme::FontStyle::Normal => gpui::FontStyle::Normal,
                            }),
                            font_weight: s.font_weight.map(|w| FontWeight(f32::from(w))),
                            ..Default::default()
                        }
                    }
                    None => HighlightStyle {
                        color: Some(mark_col),
                        font_weight: Some(FontWeight::MEDIUM),
                        ..Default::default()
                    },
                };
                Some((r, style))
            })
            .collect::<Vec<_>>();
        let preview = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .font_family(self.font.clone())
            .text_size(px(12.))
            .child(StyledText::new(place.preview.clone()).with_highlights(runs));
        let row = div()
            .id(("place", id))
            .mx(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            // Selected from the keyboard, so the pane has focus: fill plus a 1 px inset border.
            .when(selected, |d| {
                d.bg(c.selected).border_1().border_color(c.pane_focus)
            })
            .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
            .active(|s| s.bg(c.active))
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                this.prepare_place(OpenAt::of_click(&e.modifiers()), cx);
                this.navigate(&path, byte, cx);
                cx.notify();
            }));
        if show_path {
            // Definition: one card on the editor's background, a file row (icon, name,
            // folder, `:line` at the right) over the signature in the code font.
            let (folder, name) = split_path(&place.path);
            let header = div()
                .h(px(22.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .whitespace_nowrap()
                .overflow_hidden()
                .child(small_icon(crate::tree::file_icon(name), c.icon))
                .child(div().flex_none().child(name.to_string()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(11.))
                        .text_color(c.text_muted)
                        .child(folder.to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(c.text_muted)
                        .child(format!(":{}", place.line)),
                );
            row.mx(px(8.))
                .mt(px(2.))
                .rounded(px(6.))
                .overflow_hidden()
                .flex()
                .flex_col()
                .when(!selected, |d| {
                    d.bg(c.editor).border_1().border_color(c.border_variant)
                })
                .child(header)
                .child(
                    div()
                        .flex()
                        .px(px(8.))
                        .pt(px(4.))
                        .pb(px(6.))
                        .bg(c.editor)
                        .child(preview),
                )
        } else {
            // A reference preview row, indented under its file row.
            row.flex()
                .gap_2()
                .h(px(20.))
                .pl(px(10.))
                .pr(px(6.))
                .items_center()
                .child(
                    div()
                        .flex_none()
                        .min_w(px(30.))
                        .text_right()
                        .text_size(px(11.))
                        .text_color(c.line_number)
                        .child(place.line.to_string()),
                )
                .child(preview)
        }
    }

    /// The context pane: follows the caret and updates without being asked. A list that builds
    /// only the rows on screen: row 0 is the head ([`Self::context_head`]), then each
    /// reference file and line, so hundreds of references cost what a screenful does.
    pub(super) fn render_context(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let refs = self.reference_rows();
        let count = 1 + refs.len();
        let key = (self.context.generation, count);
        if self.context_list_key.get() != Some(key) {
            self.context_list.reset(count);
            self.context_list_key.set(Some(key));
        }
        if self.refs_scroll.take()
            && let Some(sel) = self.key_focus.reference_row()
        {
            let sel = sel.min(self.pane_places().len().saturating_sub(1));
            let row = refs
                .iter()
                .position(|r| matches!(r, RefRow::Place { id, .. } if *id == sel))
                .map_or(0, |i| i + 1);
            self.context_list.scroll_to_reveal_item(row);
        }
        *self.context_refs.borrow_mut() = Rc::new(refs);
        let list = crate::scroll::list(
            self.context_list.clone(),
            // Each row is held to the pane's width, so a long name is clipped rather than
            // widening its row (and pushing the row's buttons out of sight).
            cx.processor(|this, ix: usize, _window, cx| {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(this.context_row(ix, cx))
                    .into_any_element()
            }),
        )
        .map(|l| l.pb_2());
        let list = list
            .wrap(
                &self.context_over,
                &self.context_list,
                self.features.probe_if_on(Feature::Overscroll),
            )
            .flex_1()
            .min_h_0();
        self.context_shell(list, cx)
    }

    /// Looks the identifier at the caret up again, even if the caret did not move (the index
    /// or the lookup settings changed).
    pub(super) fn requery_context(&mut self, cx: &mut Context<Self>) {
        self.context.key = None;
        self.refresh_context(cx);
    }

    /// The index's answer for the focus, when it applies to the file shown: the index is
    /// ready, and the file is not at another commit without an index of its own.
    fn shown_symbol(&self) -> Option<&crate::context::SymbolContext> {
        let ready = matches!(self.index, IndexState::Ready(..));
        let at_rev = self
            .ed()
            .doc
            .as_ref()
            .is_some_and(|d| d.at_rev.is_some() && !self.ed().rev_index_ok);
        self.context.lookup.symbol().filter(|_| ready && !at_rev)
    }

    /// The answer whose definition and references the pane lists, when it lists them.
    fn shown_answer(&self) -> Option<&crate::context::SymbolContext> {
        self.pane_focus()?.ident.as_ref()?;
        self.shown_symbol()
    }

    /// The reference rows after the head: each file, then its lines.
    fn reference_rows(&self) -> Vec<RefRow> {
        let Some(a) = self.shown_answer() else {
            return Vec::new();
        };
        let mut rows = Vec::with_capacity(a.references.len() + a.reference_count + 1);
        let mut id = a.definitions.len();
        for (file, f) in a.references.iter().enumerate() {
            rows.push(RefRow::File(file));
            for place in 0..f.places.len() {
                rows.push(RefRow::Place { file, place, id });
                id += 1;
            }
        }
        if a.omitted > 0 {
            rows.push(RefRow::Omitted(a.omitted));
        }
        rows
    }

    /// One row of the list, built when it comes on screen.
    fn context_row(&self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        if ix == 0 {
            return div()
                .flex()
                .flex_col()
                .children(self.context_head(cx))
                .into_any_element();
        }
        let row = self.context_refs.borrow().get(ix - 1).copied();
        let (Some(row), Some(a)) = (row, self.shown_answer()) else {
            return div().into_any_element();
        };
        let c = crate::pane_style::colors(&self.theme, cx);
        match row {
            RefRow::File(file) => {
                let Some(f) = a.references.get(file) else {
                    return div().into_any_element();
                };
                let (folder, name) = split_path(&f.path);
                div()
                    .mx(px(4.))
                    .px(px(6.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .whitespace_nowrap()
                    .child(small_icon(crate::tree::file_icon(name), c.icon))
                    .child(div().flex_none().child(name.to_string()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(c.text_muted)
                            .child(folder.to_string()),
                    )
                    .child(count_pill(&c, f.places.len()))
                    .into_any_element()
            }
            RefRow::Place { file, place, id } => {
                let Some(p) = a.references.get(file).and_then(|f| f.places.get(place)) else {
                    return div().into_any_element();
                };
                let selected = self
                    .key_focus
                    .reference_row()
                    .map(|s| s.min(self.pane_places().len().saturating_sub(1)))
                    == Some(id);
                let mark_col = c.kind(&a.kind.to_lowercase());
                self.place_row(id, p, false, selected, mark_col, cx)
                    .into_any_element()
            }
            RefRow::Omitted(n) => div()
                .px(px(10.))
                .py_1()
                .text_color(c.text_muted)
                .child(format!("\u{2026} and {n} more not listed."))
                .into_any_element(),
        }
    }

    /// A reference preview's syntax spans, once highlighted. The first time a line is asked
    /// for it is queued and highlighted off the UI thread; the pane repaints when it is done.
    fn preview_spans(&self, place: &Place, cx: &mut Context<Self>) -> Option<Spans> {
        let key = (place.path.clone(), place.line, place.preview.clone());
        let mut p = self.previews.borrow_mut();
        if let Some(s) = p.done.get(&key) {
            return Some(s.clone());
        }
        if !p.queued.contains(&key) {
            p.queued.push(key);
        }
        if !p.busy {
            p.busy = true;
            drop(p);
            Self::highlight_queued(cx);
        }
        None
    }

    /// Highlights the queued preview lines in the background, batch by batch, until none
    /// are left.
    fn highlight_queued(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(batch) = this.update(cx, |this, _| {
                    std::mem::take(&mut this.previews.borrow_mut().queued)
                }) else {
                    return;
                };
                if batch.is_empty() {
                    this.update(cx, |this, _| this.previews.borrow_mut().busy = false)
                        .ok();
                    return;
                }
                let done = cx
                    .background_executor()
                    .spawn(async move {
                        batch
                            .into_iter()
                            .map(|k| {
                                let spans = crate::context::highlight_line(&k.0, &k.2);
                                (k, spans)
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                let alive = this.update(cx, |this, cx| {
                    let mut p = this.previews.borrow_mut();
                    // Bounded: a few screens of many symbols.
                    if p.done.len() > PREVIEW_CACHE {
                        p.done.clear();
                    }
                    for (k, v) in done {
                        p.done.insert(k, Rc::new(v));
                    }
                    drop(p);
                    this.refresh_context_panes(cx);
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// The pane's first row: the declaration, the symbol, and (with an index) the definition
    /// and the References header. Anything that stops the pane early ends here.
    fn context_head(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let c = crate::pane_style::colors(&self.theme, cx);
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let note = |text: String| {
            div()
                .px(px(10.))
                .py_1()
                .text_color(c.text_muted)
                .child(text)
                .into_any_element()
        };

        let Some(focus) = self.pane_focus() else {
            rows.push(note(
                "Click in the source: this pane follows the caret.".into(),
            ));
            return rows;
        };

        // What Tree-sitter knows: the declaration the caret is in.
        rows.push(section(&c, "Declaration", Some("you are in".into())));
        rows.push(match focus.decl {
            Some((kind, name, start, end)) => {
                let (prefix, last) = split_qualified(&name);
                div()
                    .mx(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .bg(c.editor)
                    .border_1()
                    .border_color(c.border_variant)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(
                                div()
                                    .mr(px(6.))
                                    .flex()
                                    .child(small_icon(kind_glyph(&kind), c.kind(&kind))),
                            )
                            .child(div().text_color(c.text_muted).child(prefix.to_string()))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(last.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .pl(px(20.))
                            .text_size(px(11.))
                            .text_color(c.text_muted)
                            .child(format!("{kind} \u{b7} lines {start}\u{2013}{end}")),
                    )
                    .into_any_element()
            }
            None => note("Not inside a declaration.".into()),
        });

        // The identifier under the caret, and what the index says about it.
        let following = if self.hover.active().is_some() {
            "under the pointer"
        } else {
            "at the caret"
        };
        rows.push(section(&c, "Symbol", Some(following.into())));
        let Some(ident) = focus.ident else {
            rows.push(note("No identifier at the caret.".into()));
            return rows;
        };

        let (sem_status, ready) = match &self.index {
            IndexState::Ready(_, s) => (Some(*s), true),
            _ => (None, false),
        };
        let at_rev = self
            .ed()
            .doc
            .as_ref()
            .and_then(|d| d.at_rev.clone())
            .filter(|_| !self.ed().rev_index_ok);
        // The index's answer, when there is one to show.
        let answer = self.shown_symbol();
        let graph_tip = self.tip_text(
            "Show graph: who uses it and what it uses",
            "chorong::ShowGraph",
        );
        // Symbol: glyph (when the index says what it is), name, kind word, Show graph.
        let kind = answer.map(|a| a.kind.to_lowercase());
        rows.push(
            div()
                .px(px(10.))
                .h(px(22.))
                .flex()
                .items_center()
                .whitespace_nowrap()
                .children(kind.as_deref().map(|k| {
                    div()
                        .mr(px(6.))
                        .flex()
                        .child(small_icon(kind_glyph(k), c.kind(k)))
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .font_weight(FontWeight::MEDIUM)
                        .child(ident.clone()),
                )
                .children(kind.clone().map(|k| {
                    div()
                        .flex_none()
                        .pl_2()
                        .text_size(px(11.))
                        .text_color(c.text_muted)
                        .child(k)
                }))
                .when(answer.is_some(), |d| {
                    d.child(
                        bordered_button(&c, "context-show-graph", "Show graph")
                            .ml_2()
                            .tooltip(tip(graph_tip))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_graph(None, cx);
                                cx.notify();
                            })),
                    )
                })
                .into_any_element(),
        );

        if let Some(rev) = at_rev {
            rows.push(note(format!(
                "No index for {rev}: this view is syntax-only. The current index describes the checkout and is not applied to other code."
            )));
            return rows;
        }
        if !ready {
            let text = match &self.index {
                IndexState::Loading => {
                    rows.push(note("Loading the index\u{2026}".into()));
                    return rows;
                }
                IndexState::Failed(e) => format!(
                    "Definition and references need an index. (The cached index could not be read: {e})"
                ),
                _ => "Definition and references need an index.".to_string(),
            };
            rows.push(self.needs_index_note(&c, text, cx));
            return rows;
        }
        if sem_status == Some(IndexStatus::Stale) {
            let warn = c.warning.unwrap_or(c.text);
            rows.push(
                div()
                    .mx(px(8.))
                    .mt_1()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(11.5))
                    .text_color(warn)
                    .bg(c.warning_bg.unwrap_or(c.element_bg))
                    .child(small_icon("triangle-alert", warn))
                    .child("Index is stale: results may be out of date.")
                    .into_any_element(),
            );
        }
        let a = match &self.context.lookup {
            Lookup::Loading => {
                rows.push(note("Looking up\u{2026}".into()));
                return rows;
            }
            Lookup::Done(Answer::Symbol(a)) => a,
            Lookup::Done(Answer::NoSymbol) => {
                rows.push(note("The index has no symbol here.".into()));
                return rows;
            }
            Lookup::Idle => return rows,
        };
        let selected = self
            .key_focus
            .reference_row()
            .map(|s| s.min(self.pane_places().len().saturating_sub(1)));
        let mark_col = c.kind(&a.kind.to_lowercase());
        let mut id = 0;
        let mut place_row = |rows: &mut Vec<gpui::AnyElement>, place: &Place, definition: bool| {
            rows.push(
                self.place_row(id, place, definition, selected == Some(id), mark_col, cx)
                    .into_any_element(),
            );
            id += 1;
        };
        rows.push(section(&c, "Definition", None));
        if a.definitions.is_empty() {
            rows.push(note("Defined outside the indexed project.".into()));
        }
        for place in &a.definitions {
            place_row(&mut rows, place, true);
            if let Some(label) = &place.mapped {
                rows.push(note(format!("{label} from the built declaration")));
            }
        }
        let files = a.references.len();
        let qualifier = format!(
            "{} in {files} file{}{}",
            a.reference_count,
            if files == 1 { "" } else { "s" },
            if self.key_focus.reference_row().is_some() {
                " \u{b7} keyboard"
            } else {
                ""
            }
        );
        rows.push(count_section(&c, "References", Some(qualifier)));
        if a.reference_count == 0 {
            rows.push(note("No references in the index.".into()));
        }
        rows
    }

    /// "Definition and references need an index": a dashed note with `cho index` in a key
    /// chip, then the Build index button.
    fn needs_index_note(
        &self,
        c: &cho_layout_ui::colors::Colors,
        text: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .mx(px(8.))
            .mt_1()
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_dashed()
            .border_color(c.border)
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .text_color(c.text_muted)
                    .child(
                        div()
                            .flex_none()
                            .pt(px(2.))
                            .child(small_icon("info", c.icon)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .child(text)
                            .child("Run")
                            .child(
                                div()
                                    .px(px(5.))
                                    .rounded(px(3.))
                                    .bg(c.element_bg)
                                    .font_family(self.font.clone())
                                    .text_size(px(11.))
                                    .child("cho index"),
                            ),
                    ),
            )
            .when(!matches!(self.index, IndexState::Loading), |d| {
                d.child(self.build_button(cx))
            })
            .into_any_element()
    }

    /// The pane's rows that open a place: definitions, then references, in display order.
    pub(super) fn pane_places(&self) -> Vec<&Place> {
        match self.shown_symbol() {
            Some(c) => c
                .definitions
                .iter()
                .chain(c.references.iter().flat_map(|f| f.places.iter()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Shift-F12 / `r`: the keyboard moves to the pane's reference list, on the first
    /// reference (or the definition when there are none).
    pub(super) fn focus_references(&mut self) {
        let first = match self.context.lookup.symbol() {
            Some(c) if c.reference_count > 0 => c.definitions.len(),
            _ => 0,
        };
        self.focus_to(KeyFocus::References { row: first });
        self.refs_scroll.set(true);
    }

    pub(super) fn references_step(&mut self, delta: isize) {
        let n = self.pane_places().len();
        if let KeyFocus::References { row: sel } = &mut self.key_focus
            && n > 0
        {
            *sel = (*sel).min(n - 1).saturating_add_signed(delta).min(n - 1);
            self.refs_scroll.set(true);
        }
    }

    /// Enter in the reference list: opens the selected row. The list keeps the keyboard, so
    /// the next reference is one key away.
    pub(super) fn references_open(&mut self, cx: &mut Context<Self>) {
        let places = self.pane_places();
        let Some(sel) = self
            .key_focus
            .reference_row()
            .filter(|_| !places.is_empty())
        else {
            return;
        };
        let place = places[sel.min(places.len() - 1)];
        let (path, byte) = (place.path.clone(), place.byte);
        self.navigate(&path, byte, cx);
    }

    /// The context pane's Build index button (or its progress, while a build runs).
    pub(super) fn build_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (label, clickable) = match (&self.job, &self.index) {
            (Some(j), _) => (self.job_text(j), true),
            (None, IndexState::Missing(IndexStatus::Failed)) if self.index_failure.is_some() => {
                ("Show the error".to_string(), true)
            }
            _ => ("Build index".to_string(), true),
        };
        let tip_text = if self.job.is_some() {
            self.tip_text(
                "Cancel indexing (the previous index is kept)",
                "chorong::CancelIndexing",
            )
        } else {
            self.tip_text("Build or refresh the index", "chorong::BuildIndex")
        };
        let c = crate::pane_style::colors(&self.theme, cx);
        div()
            .flex()
            .child(
                bordered_button(&c, "context-build-index", label)
                    .font_weight(FontWeight::MEDIUM)
                    .when(clickable, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| {
                            this.index_control(cx);
                            cx.notify();
                        }))
                    })
                    .when(!clickable, |d| d.text_color(c.text_muted))
                    .tooltip(tip(tip_text)),
            )
            .into_any_element()
    }

    pub(super) fn context_shell(
        &self,
        body: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id("context-pane")
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.pointer_region(super::focus::PointerRegion::ContextPane, *hovered, cx);
            }))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .child(body)
    }

    /// The Context pane header's split Pin button (drawn by the shell among the header's
    /// buttons, `PaneBodies::header_actions`): Pin here, and pin to the bottom bar.
    pub(super) fn context_pin_buttons(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let pin_tip = self.tip_text(
            "Pin here: keep this symbol in a pane under this one",
            "chorong::TogglePin",
        );
        let bottom_tip = self.tip_text(
            "Pin to bottom bar: keep this symbol in a bottom-bar tab",
            "chorong::PinContextToBottomBar",
        );
        div()
            .flex_none()
            .flex()
            .items_center()
            .text_color(c.text_muted)
            .child(
                div()
                    .id("context-pin")
                    .px(px(5.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_l(px(4.))
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .hover(|s| s.bg(c.hover).text_color(c.text))
                    .active(|s| s.bg(c.ghost_active))
                    .tooltip(tip(pin_tip))
                    .child(small_icon("pin", c.icon))
                    .child("Pin")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pin_context(cho_layout::PinPlace::Here, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("context-pin-bottom")
                    .px(px(4.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .rounded_r(px(4.))
                    .border_l_1()
                    .border_color(c.border)
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .hover(|s| s.bg(c.hover).text_color(c.text))
                    .active(|s| s.bg(c.ghost_active))
                    .tooltip(tip(bottom_tip))
                    .child("\u{2193}")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pin_context(cho_layout::PinPlace::BottomBar, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        FindReferences,
        ReferencesPrev,
        ReferencesNext,
        ReferencesOpen,
        // Cmd-Enter, Cmd-Shift-Enter, Alt-Enter: the reference in a new code pane to the
        // right, below, or as a new tab.
        ReferencesOpenRight,
        ReferencesOpenBelow,
        ReferencesOpenTab,
        ReferencesClose,
    ]
);

pub(super) const COMMANDS: &[Command] = &[c(
    "chorong::FindReferences",
    "Find References",
    Layer::Go,
    70,
)];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, FindReferences, |this, _w, cx| this
        .find_references(cx));
    let d = on!(d, cx, ReferencesPrev, |this, _w, cx| this
        .references_step(-1));
    let d = on!(d, cx, ReferencesNext, |this, _w, cx| this
        .references_step(1));
    let d = on!(d, cx, ReferencesOpen, |this, _w, cx| this
        .references_open(cx));
    let d = on!(d, cx, ReferencesOpenRight, |this, _w, cx| {
        this.prepare_place(OpenAt::Right, cx);
        this.references_open(cx)
    });
    let d = on!(d, cx, ReferencesOpenBelow, |this, _w, cx| {
        this.prepare_place(OpenAt::Below, cx);
        this.references_open(cx)
    });
    let d = on!(d, cx, ReferencesOpenTab, |this, _w, cx| {
        this.prepare_place(OpenAt::Tab, cx);
        this.references_open(cx)
    });
    on!(d, cx, ReferencesClose, |this, _w, cx| this
        .key_focus
        .leave(KeyFocus::References { row: 0 }))
}

/// A section header (docs/public/visual-design.md, "Panes and sections"): 22 px, 11 px
/// Semibold `text.muted`, 8 px of space above, an optional faint (`editor.line_number`)
/// qualifier after a `·`.
fn section(c: &Colors, title: &str, qualifier: Option<String>) -> gpui::AnyElement {
    section_with(c, title, qualifier, c.line_number)
}

/// A section header whose qualifier is a count (`14 in 3 files`): `text.muted`, not faint.
fn count_section(c: &Colors, title: &str, qualifier: Option<String>) -> gpui::AnyElement {
    section_with(c, title, qualifier, c.text_muted)
}

fn section_with(
    c: &Colors,
    title: &str,
    qualifier: Option<String>,
    qualifier_col: gpui::Hsla,
) -> gpui::AnyElement {
    div()
        .mt_2()
        .px(px(10.))
        .h(px(22.))
        .flex()
        .items_center()
        .gap_1()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_size(px(11.))
        .text_color(c.text_muted)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .children(qualifier.map(|q| div().text_color(qualifier_col).child(format!("\u{b7} {q}"))))
        .into_any_element()
}

/// A bordered button: `element.background`, 1 px `border`, `text`; hover and pressed fills.
fn bordered_button(
    c: &Colors,
    id: &'static str,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px_2()
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .bg(c.element_bg)
        .border_1()
        .border_color(c.border)
        .text_size(px(11.5))
        .text_color(c.text)
        .cursor_pointer()
        .hover(|s| s.bg(c.hover))
        .active(|s| s.bg(c.active))
        .child(label.into())
}

/// A count pill: `element.background`, `text.muted`.
fn count_pill(c: &Colors, n: usize) -> gpui::Div {
    div()
        .flex_none()
        .px(px(6.))
        .h(px(16.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .bg(c.element_bg)
        .text_size(px(10.5))
        .text_color(c.text_muted)
        .child(n.to_string())
}

/// Syntax spans with the occurrence `mark` laid over them: sorted, disjoint runs, each with
/// its theme key, or None for the mark itself. Spans the mark overlaps are cut around it.
fn preview_runs(
    spans: &[(Range<usize>, &'static str)],
    mark: Option<Range<usize>>,
) -> Vec<(Range<usize>, Option<&'static str>)> {
    let Some(m) = mark else {
        return spans.iter().map(|(r, k)| (r.clone(), Some(*k))).collect();
    };
    let mut out = Vec::with_capacity(spans.len() + 2);
    let mut placed = false;
    for (r, k) in spans {
        if r.end <= m.start || r.start >= m.end {
            if !placed && r.start >= m.end {
                out.push((m.clone(), None));
                placed = true;
            }
            out.push((r.clone(), Some(*k)));
            continue;
        }
        if r.start < m.start {
            out.push((r.start..m.start, Some(*k)));
        }
        if !placed {
            out.push((m.clone(), None));
            placed = true;
        }
        if r.end > m.end {
            out.push((m.end..r.end, Some(*k)));
        }
    }
    if !placed {
        out.push((m, None));
    }
    out
}

/// `crates/core/src/a.rs` -> (`crates/core/src`, `a.rs`).
pub(crate) fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// `Type::name` -> (`Type::`, `name`); `a.b.c` -> (`a.b.`, `c`); a plain name -> (``, name).
pub(crate) fn split_qualified(name: &str) -> (&str, &str) {
    let cut = match (name.rfind("::"), name.rfind('.')) {
        (Some(a), Some(b)) if b > a => b + 1,
        (Some(a), _) => a + 2,
        (None, Some(b)) => b + 1,
        (None, None) => 0,
    };
    name.split_at(cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_is_laid_over_the_syntax_spans() {
        let spans = [
            (0..3, "keyword"),
            (4..10, "function"),
            (10..11, "punctuation"),
        ];
        // No mark: the spans as they are.
        assert_eq!(preview_runs(&spans, None).len(), 3);
        // A mark exactly on a span replaces it.
        assert_eq!(
            preview_runs(&spans, Some(4..10)),
            [
                (0..3, Some("keyword")),
                (4..10, None),
                (10..11, Some("punctuation"))
            ]
        );
        // A mark inside a span cuts it; one between spans slots in.
        assert_eq!(
            preview_runs(&[(0..10, "string")], Some(2..4)),
            [
                (0..2, Some("string")),
                (2..4, None),
                (4..10, Some("string"))
            ]
        );
        assert_eq!(
            preview_runs(&[(0..1, "a"), (5..6, "b")], Some(2..4)),
            [(0..1, Some("a")), (2..4, None), (5..6, Some("b"))]
        );
        assert_eq!(preview_runs(&[], Some(0..2)), [(0..2, None)]);
    }

    #[test]
    fn splits_paths_and_qualified_names() {
        assert_eq!(
            split_path("crates/core/src/a.rs"),
            ("crates/core/src", "a.rs")
        );
        assert_eq!(split_path("a.rs"), ("", "a.rs"));
        assert_eq!(
            split_qualified("SourceBuffer::utf16_to_byte"),
            ("SourceBuffer::", "utf16_to_byte")
        );
        assert_eq!(split_qualified("Queue.push"), ("Queue.", "push"));
        assert_eq!(split_qualified("main"), ("", "main"));
    }
}

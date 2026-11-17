//! The source pane: folding, selection, marks, and painting the text.

use super::*;

impl Chorong {
    pub(super) fn toggle_fold(&mut self, fold: usize, cx: &mut Context<Self>) {
        let ed = self.ed_mut();
        let (Some(folds), Some(doc)) = (&mut ed.folds, &ed.doc) else {
            return;
        };
        folds.toggle(fold);
        // A caret hidden by the new fold moves to its signature line.
        if let Some(line) = ed.caret.map(|c| doc.buffer.line_of(c)) {
            let visible = folds.visible_line(line);
            if visible != line {
                let start = doc.buffer.line_range(visible).map_or(0, |r| r.start);
                ed.caret = Some(start);
                self.refresh_context(cx);
            }
        }
    }

    /// Up/Down: moves the caret to the neighbouring visible line, keeping its byte column
    /// where the line is long enough.
    /// Starts the background job of every code pane whose large JSON file is waiting for its
    /// folds (`LateFolds::Wanted`). Called from `Chorong::render`; the job adds the folds, and runs
    /// a Fold All or Unfold All pressed meanwhile, only if the pane still shows that document.
    pub(super) fn start_late_folds(&mut self, cx: &mut Context<Self>) {
        use super::editor_state::LateFolds;
        let wanted: Vec<_> = self
            .editors
            .iter_mut()
            .filter(|(_, ed)| ed.late_folds == LateFolds::Wanted)
            .filter_map(|(pane, ed)| {
                ed.late_folds = LateFolds::Running;
                Some((*pane, ed.epoch, ed.doc.clone()?))
            })
            .collect();
        for (pane, epoch, doc) in wanted {
            let cost = self.features.probe(Feature::JsonFolds);
            cx.spawn(async move |this, cx| {
                let found = cx
                    .background_spawn(async move {
                        let _t = cost.background();
                        doc.json_folds()
                    })
                    .await;
                this.update(cx, |this, cx| {
                    let Some(ed) = this.editors.get_mut(&pane) else {
                        return;
                    };
                    if ed.epoch != epoch || ed.late_folds != LateFolds::Running {
                        return;
                    }
                    ed.late_folds = LateFolds::None;
                    let fold_all = ed.fold_all_later.take();
                    let (Some(folds), Some(doc)) = (&ed.folds, &ed.doc) else {
                        return;
                    };
                    let mut folds = folds.arrived(found, fold_all);
                    // The caret stays on a shown line.
                    if let Some(caret) = ed.caret {
                        let line = doc.buffer.line_of(caret);
                        let visible = folds.visible_line(line);
                        if visible != line && fold_all.is_none() {
                            folds.reveal(line);
                        } else if visible != line {
                            ed.caret = doc.buffer.line_range(visible).map(|r| r.start);
                        }
                    }
                    ed.folds = Some(folds);
                    this.dirty_pane = Some(pane);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    pub(super) fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ed = self.ed_mut();
        let (Some(folds), Some(doc)) = (&ed.folds, &ed.doc) else {
            return;
        };
        let next = match ed.selected_line() {
            Some(line) => folds.step(folds.visible_line(line), delta).map(|n| {
                (
                    n,
                    ed.caret.unwrap_or(0) - doc.buffer.line_range(line).map_or(0, |r| r.start),
                )
            }),
            None => Some((0, 0)),
        };
        if let Some((line, col)) = next {
            let range = doc.buffer.line_range(line).unwrap_or(0..0);
            let byte = doc.buffer.clamp((range.start + col).min(range.end));
            let row = folds.row_near_line(line);
            let strategy = step_strategy(delta);
            ed.source_scroll.scroll_to_item(row, strategy);
            ed.caret = Some(byte);
            ed.selection = None;
            self.hover.left();
            self.touch_pane();
            self.refresh_context(cx);
            self.follow_link(cx);
        }
    }

    /// Left (`left`) or Right on the selected line, as in a tree: fold or unfold it, or move
    /// to the enclosing block's first line or into the body ([`crate::fold::Folds::left`]).
    pub(super) fn fold_key(&mut self, left: bool, cx: &mut Context<Self>) {
        let Some(line) = self.selected_line() else {
            return;
        };
        let key = self
            .ed()
            .folds
            .as_ref()
            .and_then(|f| if left { f.left(line) } else { f.right(line) });
        match key {
            Some(crate::fold::FoldKey::Toggle(f)) => self.toggle_fold(f, cx),
            Some(crate::fold::FoldKey::GoTo(l)) => {
                let byte = self
                    .ed()
                    .doc
                    .as_ref()
                    .and_then(|d| d.buffer.line_range(l))
                    .map_or(0, |r| r.start);
                self.set_caret(byte, true, cx);
            }
            None => return,
        }
        self.touch_pane();
    }

    pub(super) fn toggle_selected_fold(&mut self, cx: &mut Context<Self>) {
        let fold = self
            .selected_line()
            .zip(self.ed().folds.as_ref())
            .and_then(|(line, folds)| folds.fold_at(line));
        if let Some(fold) = fold {
            self.toggle_fold(fold, cx);
        }
    }

    /// A mouse press on a source line. `byte` is where the pointer is; `in_text` is false
    /// for the gutter. A double click on an identifier toggles its highlight group and
    /// selects the word; a triple click selects the line; Shift extends the selection; a
    /// drag from here selects text (`text_select.rs`).
    pub(super) fn press_source(
        &mut self,
        pane: PaneId,
        byte: usize,
        in_text: bool,
        clicks: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        self.activate_editor(pane, cx);
        self.focus_to(KeyFocus::Code);
        if matches!(self.chord, Chord::Held(_)) {
            self.chord = Chord::Idle;
        }
        // Shift-click extends the selection there is, or one from the old caret. Placing the
        // caret drops the selection, so it is taken first.
        let extend = (modifiers.shift && !modifiers.secondary()).then(|| {
            let ed = self.ed();
            let sel = ed.selection.clone().filter(|s| s.epoch == ed.epoch);
            (sel, ed.caret)
        });
        self.set_caret(byte, false, cx);
        self.touch_pane();
        if !(in_text && modifiers.secondary()) {
            self.press_select(byte, in_text, clicks, extend);
        }
        if in_text && modifiers.secondary() {
            // Cmd-click (Ctrl-click off macOS): go to definition; with Alt in a new pane to
            // the right, with Alt-Shift below.
            let place = super::shell_host::OpenAt::of_click(&modifiers);
            self.go_to_definition(Some(byte), place, cx);
        } else if in_text && !modifiers.shift && ClickKind::from_count(clicks) == ClickKind::Double
        {
            self.toggle_mark(byte, cx);
        }
        cx.notify();
    }

    /// Double-click: remove the group the identifier belongs to, or add one. With an index
    /// that answers, the group is exact (same SCIP symbol); otherwise it is every identifier
    /// with the same spelling inside the innermost enclosing declaration.
    pub(super) fn toggle_mark(&mut self, byte: usize, cx: &mut Context<Self>) {
        let Some(ident) = self
            .ed()
            .doc
            .as_ref()
            .and_then(|doc| identifier_at(&doc.analysis.identifiers, byte))
        else {
            return;
        };
        if self.ed_mut().marks.remove_containing(&ident) {
            if self.ed().marks.is_empty() {
                self.highlights_emptied(cx);
            }
            return;
        }
        let exact = self.features.on(Feature::HighlightExact);
        let Some(doc) = &self.ed().doc else { return };
        let Some(sem) = self.doc_index().filter(|_| exact) else {
            self.add_name_mark(&ident);
            let pane = self.active_editor;
            self.highlight_added(pane, cx);
            return;
        };
        let cost = self.features.probe(Feature::HighlightExact);
        let (sem, path, epoch) = (sem.clone(), doc.display_path.clone(), self.ed().epoch);
        let pane = self.active_editor;
        cx.spawn(async move |this, cx| {
            let probe = ident.clone();
            let exact = cx
                .background_spawn(async move {
                    let _t = cost.start();
                    let at = sem.symbol_at(&path, probe.start)?;
                    let occurrences = sem.occurrences_in(&at.id, &path);
                    let name = sem
                        .info(&at.id)
                        .map(|i| i.display_name.clone())
                        .unwrap_or_default();
                    // The declaration is marked only where the index says "definition".
                    let definitions: Vec<_> = sem
                        .document_occurrences(&path)
                        .into_iter()
                        .filter(|o| o.is_definition && o.id == at.id)
                        .map(|o| o.range)
                        .collect();
                    (!occurrences.is_empty()).then_some((
                        at.id.symbol,
                        name,
                        occurrences,
                        definitions,
                    ))
                })
                .await;
            this.update(cx, |this, cx| {
                // Only the pane that asked, and only while it still shows that document.
                if this.editors.get(&pane).map(|e| e.epoch) != Some(epoch) {
                    return;
                }
                this.with_pane(pane, |this| match exact {
                    Some((symbol, name, occurrences, definitions)) => {
                        let palette = this.marker_colours.len();
                        this.ed_mut().marks.add_symbol(
                            symbol,
                            name,
                            occurrences,
                            definitions,
                            palette,
                        );
                    }
                    // The index has nothing here: fall back to the (labelled) name match.
                    None => this.add_name_mark(&ident),
                });
                this.dirty_pane = Some(pane);
                this.highlight_added(pane, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn add_name_mark(&mut self, ident: &std::ops::Range<usize>) {
        let palette = self.marker_colours.len();
        let ed = self.ed_mut();
        let Some(doc) = &ed.doc else { return };
        let (scope, kind) = match enclosing(&doc.symbols, ident.start) {
            Some(s) => (s.range.clone(), s.kind.as_str()),
            None => (0..doc.buffer.len(), "file"),
        };
        ed.marks.add_name(
            &Source {
                identifiers: &doc.analysis.identifiers,
                bindings: &doc.analysis.bindings,
                text: doc.buffer.text(),
            },
            ident,
            scope,
            kind,
            palette,
        );
    }

    /// The minimap strip for `pane`, if it shows: `editor.minimap` is on, and the pane is
    /// maximised on its own, or one of two code panes on a focus page and wide enough, or
    /// `editor.minimap_always` is on and the pane is wide enough.
    pub(super) fn render_minimap(
        &self,
        pane: PaneId,
        focus: crate::minimap::Focus,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        use crate::minimap::{Colours, Focus, Key, RowSource, Shape, Tick};
        let ed = self.ed_of(pane);
        let (Some(doc), Some(folds)) = (&ed.doc, &ed.folds) else {
            return None;
        };
        let list_w = f32::from(ed.source_scroll.0.borrow().base_handle.bounds().size.width);
        let wide = || ed.minimap.wide_enough(list_w);
        let show = self.features.on(Feature::Minimap)
            && match focus {
                Focus::Alone => true,
                Focus::Pair => wide(),
                Focus::No => self.features.settings().editor_minimap_always && wide(),
            };
        ed.minimap.set_shown(show);
        if !show {
            return None;
        }
        let t = &self.theme;
        let (default, muted) = (hsla(t.editor_foreground), hsla(t.text_muted));
        let key = Key {
            doc: Arc::as_ptr(doc) as usize,
            epoch: ed.epoch,
            folds: folds.generation(),
            gutter: ed.gutter.as_ref().map_or(0, |g| Arc::as_ptr(g) as usize),
            colours: [default, muted].map(|c| {
                let r = c.to_rgb();
                u32::from_be_bytes([r.r, r.g, r.b, r.a].map(|v| (v * 255.) as u8))
            }),
        };
        let shape = ed.minimap.shape(key, || {
            let _t = self.features.time(Feature::Minimap);
            let gutter = ed.gutter.as_deref();
            Shape::build(
                folds.rows().iter().map(|row| match *row {
                    Row::Line(ix) => {
                        let line = &doc.lines[ix];
                        RowSource::Line {
                            text: &line.text,
                            highlights: &line.highlights,
                            change: gutter.and_then(|g| {
                                (g.changed(ix) || g.hollow(ix)).then(|| !g.changed(ix))
                            }),
                        }
                    }
                    Row::Placeholder(_) | Row::Note(_) => RowSource::Placeholder,
                }),
                default,
                muted,
            )
        });
        // Highlight groups: placed once per change of the groups or the shape.
        let ticks = ed.minimap.ticks(key, ed.marks.revision(), || {
            let _t = self.features.time(Feature::Minimap);
            let colours = &self.marker_colours;
            let mut ticks = Vec::new();
            for g in ed.marks.groups().iter().filter(|_| !colours.is_empty()) {
                let colour = hsla(colours[g.slot % colours.len()]);
                for r in &g.occurrences {
                    let ix = doc.buffer.line_of(r.start);
                    let (Some(row), Some(line)) = (folds.row_of_line(ix), doc.lines.get(ix)) else {
                        continue;
                    };
                    let Some(d) = line.display_range(r) else {
                        continue;
                    };
                    let col = |b: usize| line.text.get(..b).map_or(b, |s| s.chars().count());
                    let (start, end) = (col(d.start), col(d.end));
                    if start >= crate::minimap::MAX_COLS {
                        continue;
                    }
                    ticks.push(Tick {
                        row: row as u32,
                        col: start as u16,
                        len: (end - start).max(1) as u16,
                        colour,
                    });
                }
            }
            ticks
        });
        let c = crate::pane_style::colors(&self.theme, cx);
        let change = match ed.side {
            Side::After => c.success,
            Side::Before => c.error,
        }
        .unwrap_or(hsla(t.text));
        let text = hsla(t.text);
        let colours = Colours {
            background: hsla(t.editor_background),
            border: hsla(t.border),
            frame: text.opacity(0.08),
            frame_border: text.opacity(0.25),
            change,
        };
        Some(
            ed.minimap
                .element(
                    shape,
                    ticks,
                    ed.source_scroll.clone(),
                    colours,
                    self.features.probe(Feature::Minimap),
                )
                .into_any_element(),
        )
    }

    /// The overview lane for `pane`, if `editor.overview` is on and the pane has highlight
    /// groups or is comparing.
    pub(super) fn render_overview(
        &self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        use crate::overview::{Colours, Key, Shape};
        let ed = self.ed_of(pane);
        let (Some(doc), Some(folds)) = (&ed.doc, &ed.folds) else {
            return None;
        };
        // Only while there is something to point at: highlight groups, or a comparison's
        // changed lines. On its own the band and caret add little to a pane.
        if !self.features.on(Feature::Overview) || (ed.marks.is_empty() && ed.gutter.is_none()) {
            ed.overview.hide();
            return None;
        }
        let key = Key {
            doc: Arc::as_ptr(doc) as usize,
            epoch: ed.epoch,
            folds: folds.generation(),
            gutter: ed.gutter.as_ref().map_or(0, |g| Arc::as_ptr(g) as usize),
            marks: ed.marks.revision(),
            palette: self.marker_colours.len(),
        };
        let shape = ed.overview.shape(key, || {
            let _t = self.features.time(Feature::Overview);
            let colours = &self.marker_colours;
            // An occurrence inside a folded body is shown at the fold's signature row.
            let marks = ed
                .marks
                .groups()
                .iter()
                .filter(|_| !colours.is_empty())
                .flat_map(|g| {
                    let colour = hsla(colours[g.slot % colours.len().max(1)]);
                    g.occurrences.iter().map(move |r| (r.start, colour))
                })
                .map(|(byte, colour)| (folds.row_near_line(doc.buffer.line_of(byte)), colour));
            let changes = ed.gutter.as_deref().map_or_else(Vec::new, |g| {
                folds
                    .rows()
                    .iter()
                    .enumerate()
                    .filter_map(|(row, r)| match *r {
                        Row::Line(ix) if g.changed(ix) || g.hollow(ix) => {
                            Some((row as u32, !g.changed(ix)))
                        }
                        _ => None,
                    })
                    .collect()
            });
            Shape::build(folds.rows().len(), marks, changes)
        });
        let t = &self.theme;
        let c = crate::pane_style::colors(&self.theme, cx);
        let change = match ed.side {
            Side::After => c.success,
            Side::Before => c.error,
        }
        .unwrap_or(hsla(t.text));
        let colours = Colours {
            background: hsla(t.editor_background),
            border: hsla(t.border),
            band: hsla(t.text).opacity(0.08),
            caret: hsla(t.text).opacity(0.7),
            change,
        };
        let caret_row = ed.selected_line().map(|line| folds.row_near_line(line));
        Some(
            ed.overview
                .element(
                    shape,
                    caret_row,
                    ed.source_scroll.clone(),
                    colours,
                    self.features.probe(Feature::Overview),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_source(
        &self,
        pane: PaneId,
        focus: crate::minimap::Focus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let ed = self.ed_of(pane);
        let minimap = ed
            .loading
            .is_none()
            .then(|| self.render_minimap(pane, focus, cx))
            .flatten();
        let overview = ed
            .loading
            .is_none()
            .then(|| self.render_overview(pane, cx))
            .flatten();
        // The strips lie over the list's right edge; the list stops short of them.
        let strips_w = minimap.as_ref().map_or(0., |_| crate::minimap::WIDTH)
            + overview.as_ref().map_or(0., |_| crate::overview::WIDTH);
        let (Some(doc), Some(folds), None) = (&ed.doc, &ed.folds, &ed.loading) else {
            let hint = if ed.loading.is_some() {
                "Loading…"
            } else if self.files.is_empty() {
                "Open a file or folder: chorong-app <path>"
            } else {
                "Choose a file"
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(hsla(t.text_muted))
                .child(hint)
                .into_any_element();
        };
        div()
            .id("source")
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .bg(hsla(t.editor_background))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover_pane = pane;
                this.pointer_region(super::focus::PointerRegion::Source, *hovered, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(hsla(t.border))
                    .text_size(px(12.5))
                    .text_color(hsla(t.text_muted))
                    .child({
                        let file_tip =
                            self.tip_text("Show graph for this file", "chorong::ShowGraph");
                        let path = doc.display_path.clone();
                        div()
                            .id("source-path")
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .tooltip(tip(file_tip))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.activate_editor(pane, cx);
                                this.show_graph(
                                    Some(crate::graph::Scope::File { path: path.clone() }),
                                    cx,
                                );
                                cx.notify();
                            }))
                            .child(match &doc.at_rev {
                                Some(rev) => format!("{} @ {rev} (from git)", doc.display_path),
                                None => doc.display_path.clone(),
                            })
                    })
                    .children(self.blame_status(ed).map(|status| {
                        div()
                            .ml_auto()
                            .pl_4()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(status)
                    })),
            )
            .children(self.render_find_bar(pane, cx))
            .child(
                crate::scroll::uniform_list(
                    ("source", pane.0 as usize),
                    folds.rows().len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        let ed = this.ed_of(pane);
                        let (Some(doc), Some(folds)) = (&ed.doc, &ed.folds) else {
                            return Vec::new();
                        };
                        let advance = this.cell_width(window);
                        let t = &this.theme;
                        let (line_no, active_no, active_bg) = (
                            hsla(t.editor_line_number),
                            hsla(t.editor_active_line_number),
                            hsla(t.editor_active_line_background),
                        );
                        let (muted, hover_bg) = (hsla(t.text_muted), hsla(t.element_selected));
                        // Comparison marks: gutter bars and wedges, line tints, changed
                        // words and deletion rules (`change_marks.rs`).
                        let mark = this.change_style(ed, cx);
                        // Their cost, per visible line, for the settings page.
                        let line_probe = mark
                            .as_ref()
                            .filter(|m| m.lines)
                            .map(|_| this.features.probe(Feature::LineHighlight));
                        let word_probe = mark
                            .as_ref()
                            .filter(|m| m.words)
                            .map(|_| this.features.probe(Feature::ChangedWords));
                        let line_count = doc.lines.len();
                        // The blame column, while Toggle Blame is on (`blame.rs`).
                        let blame_width = this.blame_width(ed, advance);
                        let blame_view = match this.blame_of(ed) {
                            Some(blame::BlameState::Ready(v)) => Some(v.clone()),
                            _ => None,
                        };
                        let blame_colours = blame::BlameColours {
                            accent: hsla(t.ui.text_accent),
                            text: hsla(t.editor_foreground),
                            muted,
                            border: hsla(t.border),
                        };
                        let now = blame::now();
                        // The open step's lines (a tour stop or call-stack frame), or
                        // each of its highlighted chunks.
                        let step_lines = ed
                            .step_mark
                            .as_ref()
                            .filter(|m| m.applies_to(doc))
                            .map(|m| m.tinted())
                            .unwrap_or_default();
                        let step_tint = crate::pane_style::colors(&this.theme, cx).accent_faint;
                        // What the step or each highlight pins (an excerpt or
                        // from..to): a stronger tint inside the faint one.
                        let pinned = ed
                            .step_mark
                            .as_ref()
                            .filter(|m| m.applies_to(doc))
                            .map(|m| m.pinned_ranges())
                            .unwrap_or_default();
                        let pinned_tint = hsla(t.ui.text_accent.with_alpha(0x2c));
                        let caret_accent = hsla(t.ui.text_accent);
                        // The text selection: nothing to check unless there is one,
                        // and then only on the rows being drawn.
                        let sel = ed.selection_range();
                        let sel_colour = hsla(
                            t.players
                                .first()
                                .map_or(t.ui.text_accent.with_alpha(0x3d), |p| p.selection),
                        );
                        range
                            .map(|ix| match folds.rows()[ix] {
                                Row::Line(line_ix) => {
                                    let line = &doc.lines[line_ix];
                                    // Not where a tour step put the caret: its tint
                                    // shows there until the caret moves.
                                    let selected = ed.selected_line() == Some(line_ix)
                                        && (ed.step_caret.is_none() || ed.step_caret != ed.caret);
                                    let fold = folds.fold_at(line_ix);
                                    let chevron =
                                        fold.map(|f| if folds.is_collapsed(f) { "▸" } else { "▾" });
                                    // An opened fold shows ▾ in its number's place; a
                                    // folded one keeps its number and shows ▸ on hover.
                                    let open = fold.is_some_and(|f| !folds.is_collapsed(f));
                                    let text = StyledText::new(line.text.clone())
                                        .with_highlights(this.line_highlights(ed, line));
                                    let layout = text.layout().clone();
                                    // Markers sit in a layer behind the text, positioned by
                                    // display column; the text itself is never moved.
                                    let boxes = this.marker_boxes(ed, line, advance);
                                    // Over every tint and marker, behind the text.
                                    let selected_text = sel
                                        .as_ref()
                                        .and_then(|s| crate::selection::columns_on(line, s))
                                        .map(|c| {
                                            div()
                                                .absolute()
                                                .left(px(c.start as f32 * advance))
                                                .top_0()
                                                .w(px((c.end - c.start) as f32 * advance))
                                                .h(px(LINE_HEIGHT))
                                                .bg(sel_colour)
                                        });
                                    let above =
                                        ix.checked_sub(1).and_then(|p| match folds.rows()[p] {
                                            Row::Line(l) => Some(l),
                                            Row::Placeholder(_) | Row::Note(_) => None,
                                        });
                                    // One line tint: what a tour step pins, else the
                                    // open step's lines, else the comparison's change
                                    // tint, else (on the caret's line) the selection.
                                    let in_step = step_lines.iter().any(|r| {
                                        r.start < line.bytes.end.max(line.bytes.start + 1)
                                            && r.end > line.bytes.start
                                    });
                                    let words = mark.as_ref().map_or_else(Vec::new, |m| {
                                        let _t = word_probe.as_ref().map(|p| p.start());
                                        m.word_boxes(line_ix, line, advance)
                                    });
                                    let line_timer = line_probe.as_ref().map(|p| p.start());
                                    let in_pinned = pinned.iter().any(|r| {
                                        r.start < line.bytes.end.max(line.bytes.start + 1)
                                            && r.end > line.bytes.start
                                    });
                                    let under = if in_pinned {
                                        Some(pinned_tint)
                                    } else if in_step {
                                        Some(step_tint)
                                    } else {
                                        mark.as_ref().and_then(|m| m.tint(line_ix))
                                    };
                                    // The caret's line: the grey fill on a plain line;
                                    // inside a tint (a tour step, a chunk, a change)
                                    // the tint stays and a bar at the code's edge
                                    // marks it, so the tint has no grey hole.
                                    let tint = match under {
                                        None if selected => Some(active_bg),
                                        u => u,
                                    };
                                    let caret_bar = selected && under.is_some();
                                    // Deletion markers on the boundary above this line
                                    // and, for the last line, below it.
                                    let code_left = blame_width.unwrap_or(0.) + GUTTER_WIDTH;
                                    let caret_bar = caret_bar.then(|| {
                                        div()
                                            .absolute()
                                            .left(px(code_left))
                                            .top_0()
                                            .h(px(LINE_HEIGHT))
                                            .w(px(2.))
                                            .bg(caret_accent)
                                    });
                                    let rules: Vec<_> = mark
                                        .as_ref()
                                        .map(|m| {
                                            let mut v = Vec::new();
                                            if m.gutter.wedge_above(line_ix)
                                                && let Some(g) = m.gutter.gap(line_ix)
                                            {
                                                v.extend(
                                                    m.rule(pane, line_ix, *g, false, code_left, cx),
                                                );
                                            }
                                            if line_ix + 1 == line_count
                                                && m.gutter.wedge_at_end()
                                                && let Some(g) = m.gutter.gap(line_count)
                                            {
                                                v.extend(
                                                    m.rule(pane, line_ix, *g, true, code_left, cx),
                                                );
                                            }
                                            v
                                        })
                                        .unwrap_or_default();
                                    drop(line_timer);
                                    div()
                                        .id(("line", ix))
                                        .relative()
                                        // At least the list's width, so a tint covers
                                        // the whole visible line.
                                        .min_w_full()
                                        .flex()
                                        .h(px(LINE_HEIGHT))
                                        .whitespace_nowrap()
                                        .when_some(tint, |d, c| d.bg(c))
                                        .children(caret_bar)
                                        .children(this.flash_overlay(ed, line_ix, code_left, cx))
                                        .children(rules)
                                        .when_some(blame_width, |d, w| {
                                            d.child(blame::cell(
                                                blame_view.as_ref(),
                                                ix,
                                                line_ix,
                                                above,
                                                w,
                                                blame_colours,
                                                now,
                                            ))
                                        })
                                        .child(
                                            div()
                                                .flex_none()
                                                .relative()
                                                .w(px(GUTTER_WIDTH))
                                                .flex()
                                                .children(this.gutter_mark(ed, &line.bytes))
                                                .children(mark.as_ref().and_then(|m| {
                                                    // A bar: the side's colour for committed
                                                    // work, the theme's "modified" colour for
                                                    // uncommitted work.
                                                    let colour = m.bar(line_ix)?;
                                                    Some(
                                                        div()
                                                            .id(("change-bar", ix))
                                                            .absolute()
                                                            .left_0()
                                                            .top_0()
                                                            .w(px(4.))
                                                            .h(px(LINE_HEIGHT))
                                                            .bg(colour)
                                                            .when(
                                                                m.whitespace_only(line_ix),
                                                                |d| {
                                                                    d.tooltip(tip(
                                                                        "Whitespace only"
                                                                            .to_string(),
                                                                    ))
                                                                },
                                                            ),
                                                    )
                                                }))
                                                .children(mark.as_ref().and_then(|m| {
                                                    let g = &m.gutter;
                                                    let top = g.wedge_above(line_ix);
                                                    let end = line_ix + 1 == line_count
                                                        && g.wedge_at_end();
                                                    let hollow = g.wedge_is_hollow(if top {
                                                        line_ix
                                                    } else {
                                                        line_count
                                                    });
                                                    let (colour, filled) = m.wedge(hollow);
                                                    (top || end).then(|| {
                                                        // A triangle on the line boundary,
                                                        // pointing at the gap: the other side has
                                                        // lines here that this side lacks.
                                                        gpui::canvas(
                                                            |_, _, _| (),
                                                            move |b, _, window, _| {
                                                                let (x, y) = (
                                                                    b.origin.x,
                                                                    b.origin.y + b.size.height / 2.,
                                                                );
                                                                let mut p = if filled {
                                                                    gpui::PathBuilder::fill()
                                                                } else {
                                                                    gpui::PathBuilder::stroke(px(
                                                                        1.,
                                                                    ))
                                                                };
                                                                p.move_to(gpui::point(
                                                                    x,
                                                                    y - px(5.),
                                                                ));
                                                                p.line_to(gpui::point(
                                                                    x + px(8.),
                                                                    y,
                                                                ));
                                                                p.line_to(gpui::point(
                                                                    x,
                                                                    y + px(5.),
                                                                ));
                                                                p.close();
                                                                if let Ok(path) = p.build() {
                                                                    window.paint_path(path, colour);
                                                                }
                                                            },
                                                        )
                                                        .absolute()
                                                        .left(px(1.))
                                                        .when(top, |d| d.top(px(-5.)))
                                                        .when(!top, |d| d.top(px(LINE_HEIGHT - 5.)))
                                                        .w(px(9.))
                                                        .h(px(10.))
                                                    })
                                                }))
                                                // The uncommitted bracket and ticks, over the triangle.
                                                .children(
                                                    mark.as_ref().and_then(|m| {
                                                        m.bracket(line_ix, line_count)
                                                    }),
                                                )
                                                // The line number. An opened fold's line shows ▾ instead (its number in
                                                // the tooltip); a folded one swaps its number for ▸ on hover.
                                                .child(
                                                    div()
                                                        .id(("number", ix))
                                                        .group("gutter-number")
                                                        .flex_1()
                                                        .relative()
                                                        .pr(px(NUMBER_PAD))
                                                        .text_right()
                                                        .text_color(
                                                            // Uncommitted lines' numbers are yellow.
                                                            match mark.as_ref().and_then(|m| {
                                                                m.number_colour(line_ix)
                                                            }) {
                                                                Some(c) => c,
                                                                None if selected => active_no,
                                                                None => line_no,
                                                            },
                                                        )
                                                        .child(
                                                            div()
                                                                .when(open, |d| d.invisible())
                                                                .when(fold.is_some(), |d| {
                                                                    d.group_hover(
                                                                        "gutter-number",
                                                                        |s| s.invisible(),
                                                                    )
                                                                })
                                                                .child(format!("{}", line_ix + 1)),
                                                        )
                                                        .when_some(chevron, |d, glyph| {
                                                            d.cursor_pointer()
                                                                .when(open, |d| {
                                                                    d.tooltip(tip(format!(
                                                                        "Line {}: click to fold",
                                                                        line_ix + 1
                                                                    )))
                                                                })
                                                                .child(
                                                                    div()
                                                                        .absolute()
                                                                        .top_0()
                                                                        .right(px(NUMBER_PAD))
                                                                        .when(!open, |d| {
                                                                            d.invisible()
                                                                        })
                                                                        .group_hover(
                                                                            "gutter-number",
                                                                            |s| s.visible(),
                                                                        )
                                                                        .text_color(muted)
                                                                        .child(glyph),
                                                                )
                                                        })
                                                        // The chevron toggles a fold; it doesn't move the caret.
                                                        .when(fold.is_some(), |d| {
                                                            d.on_mouse_down(
                                                                MouseButton::Left,
                                                                |_, _, cx| cx.stop_propagation(),
                                                            )
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                if let Some(f) = fold {
                                                                    this.activate_editor(pane, cx);
                                                                    this.toggle_fold(f, cx);
                                                                    this.touch_pane();
                                                                    cx.stop_propagation();
                                                                    cx.notify();
                                                                }
                                                            },
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .relative()
                                                .flex_none()
                                                .children(words)
                                                .children(boxes)
                                                .children(selected_text)
                                                .child(text),
                                        )
                                        .on_mouse_move({
                                            let layout = layout.clone();
                                            cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                                                // A drag selecting text: the head follows the
                                                // pointer, mapped as a click is.
                                                if this.ed_of(pane).drag.is_some() {
                                                    if ev.pressed_button != Some(MouseButton::Left)
                                                    {
                                                        this.end_drag(pane, cx);
                                                    } else if let Some(doc) =
                                                        this.ed_of(pane).doc.clone()
                                                    {
                                                        let (byte, _) = text_select::hit_byte(
                                                            &layout,
                                                            ev.position,
                                                            &doc.lines[line_ix],
                                                        );
                                                        this.drag_to(pane, byte, cx);
                                                        return;
                                                    }
                                                }
                                                // Same pointer-to-byte mapping as a click; only a
                                                // pointer on an identifier's own characters counts.
                                                let ident = (ev.position.x
                                                    >= layout.bounds().left())
                                                .then(|| {
                                                    let (Ok(i) | Err(i)) =
                                                        layout.index_for_position(ev.position);
                                                    let doc = this.ed_of(pane).doc.as_ref()?;
                                                    let byte = doc.lines[line_ix].byte_at(i);
                                                    identifier_at(&doc.analysis.identifiers, byte)
                                                        .filter(|r| byte < r.end)
                                                })
                                                .flatten();
                                                this.hover_moved(pane, ident, cx);
                                            })
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                                // Pointer to byte: the layout maps the position to
                                                // a display offset; tabs are undone from there.
                                                let hit = this.ed_of(pane).doc.as_ref().map(|d| {
                                                    text_select::hit_byte(
                                                        &layout,
                                                        ev.position,
                                                        &d.lines[line_ix],
                                                    )
                                                });
                                                if let Some((byte, in_text)) = hit {
                                                    this.press_source(
                                                        pane,
                                                        byte,
                                                        in_text,
                                                        ev.click_count,
                                                        ev.modifiers,
                                                        cx,
                                                    );
                                                }
                                            }),
                                        )
                                }
                                Row::Note(n) => this.render_note_row(pane, n, ix, cx),
                                Row::Placeholder(f) => {
                                    let fold = folds.folds()[f];
                                    let label = format!("⋯ {} lines", fold.hidden_lines());
                                    // Highlights inside the fold show on its row.
                                    let hidden = doc
                                        .lines
                                        .get(fold.start + 1)
                                        .zip(doc.lines.get(fold.end.saturating_sub(1)))
                                        .map(|(a, b)| a.bytes.start..b.bytes.end);
                                    let inside =
                                        hidden.as_ref().and_then(|r| this.gutter_mark(ed, r));
                                    // Hidden lines inside the selection: the row is
                                    // painted as selected (a copy takes them).
                                    let covered = sel
                                        .as_ref()
                                        .zip(hidden.as_ref())
                                        .is_some_and(|(s, h)| crate::selection::covers(s, h));
                                    div()
                                        .id(("fold", ix))
                                        .when(covered, |d| d.bg(sel_colour))
                                        .on_mouse_move(cx.listener(
                                            move |this, ev: &MouseMoveEvent, _, cx| {
                                                if this.ed_of(pane).drag.is_some()
                                                    && ev.pressed_button == Some(MouseButton::Left)
                                                {
                                                    this.drag_over_fold(pane, f, cx);
                                                }
                                            },
                                        ))
                                        .flex()
                                        .items_center()
                                        .h(px(LINE_HEIGHT))
                                        .whitespace_nowrap()
                                        .when_some(blame_width, |d, w| {
                                            d.child(blame::cell(
                                                None,
                                                ix,
                                                0,
                                                None,
                                                w,
                                                blame_colours,
                                                now,
                                            ))
                                        })
                                        .child(
                                            div()
                                                .flex_none()
                                                .relative()
                                                .h_full()
                                                .w(px(GUTTER_WIDTH))
                                                .children(inside),
                                        )
                                        .child(
                                            div()
                                                .px_2()
                                                .rounded_sm()
                                                .text_color(muted)
                                                .bg(hover_bg)
                                                .cursor_pointer()
                                                .child(label),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.activate_editor(pane, cx);
                                            this.toggle_fold(f, cx);
                                            this.touch_pane();
                                            cx.notify();
                                        }))
                                }
                            })
                            .collect()
                    }),
                )
                .map(|l| {
                    l.with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                })
                .wrap(
                    &ed.source_over,
                    &ed.source_scroll,
                    self.features.probe_if_on(Feature::Overscroll),
                )
                .when(ed.drag.is_some(), |d| d.child(self.drag_listener(pane, cx)))
                .flex_1()
                .when(strips_w > 0., |d| d.pr(px(strips_w)))
                .font_family(self.font.clone())
                .text_size(px(FONT_SIZE))
                .line_height(px(LINE_HEIGHT))
                .text_color(hsla(t.editor_foreground)),
            )
            .when(strips_w > 0., |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(30.))
                        .bottom_0()
                        .right_0()
                        .w(px(strips_w))
                        .flex()
                        .children(minimap)
                        .children(overview),
                )
            })
            .into_any_element()
    }

    /// Width of one character cell of the source font.
    pub(super) fn cell_width(&self, window: &Window) -> f32 {
        let system = window.text_system();
        let id = system.resolve_font(&gpui::font(self.font.clone()));
        system
            .advance(id, px(FONT_SIZE), 'm')
            .map_or(FONT_SIZE * 0.6, |s| f32::from(s.width))
    }

    /// The gutter's highlight mark for `bytes` (a line, or a fold's hidden lines): a square
    /// bar at the gutter's outer edge in each present group's colour, one segment per group (up
    /// to three), wider where the group's declaration is, with a pixel of space above and below
    /// so marks on neighbouring lines stay apart. While comparing, the comparison bar keeps the
    /// edge and this one sits just inside it. `None` when no group is there.
    pub(super) fn gutter_mark(
        &self,
        ed: &EditorState,
        bytes: &std::ops::Range<usize>,
    ) -> Option<gpui::Div> {
        if ed.marks.is_empty()
            || self.marker_colours.is_empty()
            || !self.features.settings().highlight_gutter_marks
        {
            return None;
        }
        let slots = ed.marks.slots_in(bytes);
        if slots.is_empty() {
            return None;
        }
        let h = (LINE_HEIGHT - 2.) / slots.len().min(3) as f32;
        let left = if ed.gutter.is_some() { 6. } else { 0. };
        let palette = &self.marker_colours;
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(1.))
                .h(px(LINE_HEIGHT - 2.))
                .flex()
                .flex_col()
                .children(slots.into_iter().take(3).map(move |(slot, declaration)| {
                    div()
                        .flex_none()
                        .h(px(h))
                        .w(px(if declaration { 14. } else { 12. }))
                        .bg(hsla(palette[slot % palette.len()]))
                })),
        )
    }

    /// The highlight-group boxes on one line, absolutely positioned behind the text:
    /// x = display column times the cell width, padded without moving any character.
    pub(super) fn marker_boxes(
        &self,
        ed: &EditorState,
        line: &crate::document::DisplayLine,
        advance: f32,
    ) -> Vec<gpui::Div> {
        let landing = ed
            .landed
            .as_ref()
            .filter(|(caret, r)| {
                ed.caret == Some(*caret) && r.start < line.bytes.end && r.end > line.bytes.start
            })
            .and_then(|(_, r)| {
                let r = r.start.max(line.bytes.start)..r.end.min(line.bytes.end);
                let display = line.display_range(&r)?;
                let (x, w) = markers::extent(&line.text, &display, advance);
                let style = markers::style(self.theme.ui.text_accent, self.theme.appearance, true);
                Some(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(1.))
                        .w(px(w))
                        .h(px(LINE_HEIGHT - 2.))
                        .rounded(px(markers::RADIUS))
                        .border(px(style.outline_width))
                        .border_color(hsla(style.outline))
                        .bg(hsla(style.fill)),
                )
            });
        if ed.marks.is_empty() || self.marker_colours.is_empty() {
            return landing.into_iter().collect();
        }
        ed.marks
            .in_range(&line.bytes)
            .into_iter()
            .filter_map(|(slot, r, declaration)| {
                let colour = self.marker_colours[slot % self.marker_colours.len()];
                let style = markers::style(colour, self.theme.appearance, declaration);
                let display = line.display_range(&r)?;
                let (x, w) = markers::extent(&line.text, &display, advance);
                Some(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(1.))
                        .w(px(w))
                        .h(px(LINE_HEIGHT - 2.))
                        .rounded(px(markers::RADIUS))
                        .border(px(style.outline_width))
                        .border_color(hsla(style.outline))
                        .bg(hsla(style.fill)),
                )
            })
            .chain(landing)
            .collect()
    }

    /// Highlights for one source line: syntax, then the Critical Lens fade, lighter on the
    /// occurrences of highlight groups. Pure presentation; the text and positions never
    /// change. The groups' boxes are not here: they are drawn behind the text
    /// (`marker_boxes`).
    pub(super) fn line_highlights(
        &self,
        ed: &EditorState,
        line: &crate::document::DisplayLine,
    ) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
        if self.lens {
            // Kinds only once the index has answered for this file; until then (and with no
            // index at all) the plain lens, with no kind marks.
            let classified = self.lens_kinds_known(ed);
            let kinded: Vec<KindedCall> = line
                .calls
                .iter()
                .zip(&line.call_ids)
                .map(|(r, id)| {
                    let kind = classified
                        .then(|| ed.call_kinds.get(*id).copied().unwrap_or(CallKind::Unknown));
                    (r.clone(), kind)
                })
                .collect();
            // Highlight groups fade less: the reader chose them.
            let keep: Vec<_> = ed
                .marks
                .in_range(&line.bytes)
                .into_iter()
                .filter_map(|(_, r, _)| line.display_range(&r))
                .collect();
            crate::lens::unfade(
                lens_highlights(
                    line.text.len(),
                    &line.highlights,
                    &kinded,
                    hsla(self.theme.text_muted),
                ),
                &keep,
            )
        } else {
            line.highlights.clone()
        }
    }

    /// Whether the lens can show call kinds: an index is loaded and has classified this file.
    pub(super) fn lens_kinds_known(&self, ed: &EditorState) -> bool {
        self.doc_index_of(ed).is_some() && !ed.call_kinds.is_empty()
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        ToggleCriticalLens,
        ToggleHideTests,
        FoldAll,
        UnfoldAll,
        HighlightSymbol,
        NextOccurrence,
        PrevOccurrence,
        ClearHighlights,
        RemoveNewestHighlight,
        ToggleFold,
        FoldOrEnclosing,
        UnfoldOrStepIn,
        PrevDeclaration,
        NextDeclaration,
        SelectPrevLine,
        SelectNextLine,
        CopySelection,
        CopyWithLocation,
        SelectAll,
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c(
        "chorong::ToggleCriticalLens",
        "Toggle Critical Lens",
        Layer::View,
        120,
    ),
    c(
        "chorong::ToggleHideTests",
        "Toggle Hide Tests",
        Layer::View,
        130,
    ),
    c("chorong::FoldAll", "Fold All", Layer::View, 250),
    c(
        "chorong::FoldOrEnclosing",
        "Fold, or Go to Enclosing Block",
        Layer::Read,
        255,
    ),
    c(
        "chorong::UnfoldOrStepIn",
        "Unfold, or Step Into Block",
        Layer::Read,
        258,
    ),
    c("chorong::UnfoldAll", "Unfold All", Layer::View, 260),
    c(
        "chorong::HighlightSymbol",
        "Highlight Symbol Under Cursor",
        Layer::Read,
        270,
    ),
    c(
        "chorong::NextOccurrence",
        "Next Occurrence",
        Layer::Read,
        280,
    ),
    c(
        "chorong::PrevOccurrence",
        "Previous Occurrence",
        Layer::Read,
        290,
    ),
    c(
        "chorong::ClearHighlights",
        "Clear Highlights",
        Layer::Read,
        300,
    ),
    c(
        "chorong::RemoveNewestHighlight",
        "Remove Newest Highlight",
        Layer::Read,
        302,
    ),
    c(
        "chorong::ToggleFold",
        "Fold or Unfold Declaration",
        Layer::Read,
        310,
    ),
    c("chorong::CopySelection", "Copy", Layer::Read, 313),
    c(
        "chorong::CopyWithLocation",
        "Copy with Location",
        Layer::Read,
        314,
    ),
    c("chorong::SelectAll", "Select All", Layer::Read, 315),
    c(
        "chorong::PrevDeclaration",
        "Previous Declaration",
        Layer::Read,
        320,
    ),
    c(
        "chorong::NextDeclaration",
        "Next Declaration",
        Layer::Read,
        330,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, ToggleCriticalLens, |this, _w, cx| this.lens =
        !this.lens);
    let d = on!(d, cx, FoldAll, |this, _w, cx| this.fold_all(true, cx));
    let d = on!(d, cx, UnfoldAll, |this, _w, cx| this.fold_all(false, cx));
    let d = on!(d, cx, HighlightSymbol, |this, _w, cx| this
        .highlight_symbol(cx));
    let d = on!(d, cx, NextOccurrence, |this, _w, cx| this
        .step_highlight(true, cx));
    let d = on!(d, cx, PrevOccurrence, |this, _w, cx| this
        .step_highlight(false, cx));
    // Every group at once (the palette's Clear Highlights).
    let d = on!(d, cx, ClearHighlights, |this, _w, cx| {
        if this.ed().marks.has_own() {
            this.ed_mut().marks.clear_own();
            this.find_group_gone(cx);
            this.highlights_emptied(cx);
        }
    });
    // Esc: a selection first; then the newest group; again, soon after, the rest
    // (`clear_highlights`).
    let d = on!(d, cx, RemoveNewestHighlight, |this, window, cx| {
        if this.clear_selection() {
            // Only the selection goes.
        } else if this.shell.read(cx).escape_pending() {
            window.dispatch_action(Box::new(cho_layout_ui::actions::RestoreLayout), cx);
        } else {
            let had = !this.ed().marks.is_empty();
            this.clear_highlights();
            this.find_group_gone(cx);
            if had && this.ed().marks.is_empty() {
                this.highlights_emptied(cx);
            }
        }
    });
    let d = on!(d, cx, FoldOrEnclosing, |this, _w, cx| this
        .fold_key(true, cx));
    let d = on!(d, cx, UnfoldOrStepIn, |this, _w, cx| this
        .fold_key(false, cx));
    let d = on!(d, cx, ToggleFold, |this, _w, cx| this
        .toggle_selected_fold(cx));
    let d = on!(d, cx, PrevDeclaration, |this, _w, cx| this
        .step_declaration(false, cx));
    let d = on!(d, cx, NextDeclaration, |this, _w, cx| this
        .step_declaration(true, cx));
    let d = on!(d, cx, SelectPrevLine, |this, _w, cx| this
        .move_selection(-1, cx));
    let d = on!(d, cx, SelectNextLine, |this, _w, cx| this
        .move_selection(1, cx));
    let d = on!(d, cx, CopySelection, |this, _w, cx| this
        .copy_selection(false, cx));
    let d = on!(d, cx, CopyWithLocation, |this, _w, cx| this
        .copy_selection(true, cx));
    let d = on!(d, cx, SelectAll, |this, _w, cx| this.select_all(cx));
    unavailable!(
        d,
        cx,
        ToggleHideTests => "Hide Tests",
    )
}

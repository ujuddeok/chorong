//! Opening files and moving around: history, caret, targets, folds.

use super::*;

impl Chorong {
    /// Starts showing `path` in the active editor.
    pub(super) fn open(
        &mut self,
        path: PathBuf,
        target: Target,
        record: bool,
        cx: &mut Context<Self>,
    ) {
        self.open_in(self.active_editor, path, target, record, cx);
    }

    /// Starts showing `path` in editor pane `pane`. The file is read, highlighted and outlined
    /// on the background executor; `record` adds the result to the pane's back/forward
    /// history. The result applies only to that pane, and only if no newer open of it started.
    pub(super) fn open_in(
        &mut self,
        pane: PaneId,
        path: PathBuf,
        target: Target,
        record: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.editors.contains_key(&pane) {
            return;
        }
        // Another file in a column of a split: both columns load it and stay paired. The
        // column on the place's side (or this one) holds the place; the other follows.
        let other_file = self.ed_of(pane).doc.as_ref().is_none_or(|d| d.path != path);
        if let Some(link) = self.links.iter().copied().find(|l| l.has(pane))
            && other_file
            && self.compare.is_some()
        {
            let lead = match target.place.as_ref().map(|p| p.side) {
                Some(Side::Before) => link.before,
                Some(Side::After) => link.after,
                None => pane,
            };
            let partner = link.other(lead);
            self.open_one(lead, path.clone(), target, record, cx);
            self.open_one(partner, path, Target::default(), false, cx);
            if let Some(ed) = self.editors.get_mut(&lead) {
                ed.lead_pair = true;
            }
            return;
        }
        // Opening the same file elsewhere, or not comparing: the pair ends.
        self.unlink(pane, cx);
        self.open_one(pane, path, target, record, cx);
    }

    /// `open_in` for one pane, keeping any pair it is in.
    fn open_one(
        &mut self,
        pane: PaneId,
        path: PathBuf,
        target: Target,
        record: bool,
        cx: &mut Context<Self>,
    ) {
        // An external stop's card gives way to the file (Back, a jump, the next stop).
        if let Some(ed) = self.editors.get_mut(&pane)
            && ed
                .step_mark
                .as_ref()
                .is_some_and(|m| m.external.is_some() || m.action.is_some())
        {
            ed.step_mark = None;
        }
        self.with_pane(pane, |this| {
            if record {
                // Remember where the user was, so Back restores the exact caret.
                this.remember_caret();
            }
            this.ed_mut().generation.next();
            this.ed_mut().pending_place = None;
            // The same file reloads when the one on screen was read at another commit.
            if this
                .ed()
                .doc
                .as_ref()
                .is_some_and(|d| d.path == path && d.at_rev.is_none())
            {
                this.ed_mut().loading = None;
                this.apply_target(&target, cx);
                if record {
                    this.record_history();
                }
                return;
            }
            this.ed_mut().loading = Some(path.clone());
            this.ed_mut().error = None;
        });
        self.refresh_context(cx);
        if self.ed_of(pane).loading.is_none() {
            cx.notify();
            return;
        }
        let generation = self.ed_of(pane).generation;
        let display = self.display_of(&path);
        let theme = self.theme.clone();
        let json = self.features.on(Feature::JsonFolds);
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let load_path = path.clone();
            let result = cx
                .background_spawn(async move {
                    Document::load(load_path, root.as_deref(), display, &theme, json)
                })
                .await;
            this.update(cx, |this, cx| {
                // The pane closed, or a newer open of it has started since: drop this result.
                if !this.editor_is(pane, generation) {
                    return;
                }
                this.with_pane(pane, |this| {
                    this.ed_mut().loading = None;
                    match result {
                        Ok(doc) => {
                            this.install(doc);
                            this.prepare_pane(pane, cx);
                            this.refresh_uncommitted(cx);
                            this.refresh_call_kinds(cx);
                            this.apply_target(&target, cx);
                            if record {
                                this.record_history();
                            }
                            // While comparing, a file opens on the window's side (a column
                            // of a split on its own side); a place from the Changes list
                            // chose its side already.
                            let want = this.fixed_side(pane).unwrap_or(this.window_side);
                            if let Some(cmp) = this.compare.clone()
                                && want == Side::Before
                                && target.place.is_none()
                                && this.ed().side == Side::After
                            {
                                this.switch_pane(pane, Side::Before, cmp, cx);
                            }
                        }
                        Err(e) => this.ed_mut().error = Some(format!("{}: {e}", path.display())),
                    }
                });
                this.sync_editor_pane(pane, cx);
                this.refresh_context(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Runs `f` with `pane` as the active editor, so the commands it calls act on that pane,
    /// then restores the active editor. The context pane is not queried meanwhile and is
    /// refreshed for the real active editor afterwards.
    pub(super) fn with_pane<R>(&mut self, pane: PaneId, f: impl FnOnce(&mut Self) -> R) -> R {
        if pane == self.active_editor {
            return f(self);
        }
        let real = std::mem::replace(&mut self.active_editor, pane);
        let was = std::mem::replace(&mut self.pane_override, true);
        let r = f(self);
        self.pane_override = was;
        self.active_editor = real;
        r
    }

    /// The state of editor pane `pane` (the active editor's if it has none).
    pub(super) fn ed_of(&self, pane: PaneId) -> &EditorState {
        self.editors.get(&pane).unwrap_or_else(|| self.ed())
    }

    /// Whether pane `pane` still exists and `ticket` is the latest open of its editor.
    pub(super) fn editor_is(&self, pane: PaneId, ticket: Generation) -> bool {
        self.editors
            .get(&pane)
            .is_some_and(|e| e.generation.is(ticket))
    }

    /// Makes `doc` current in the active editor, restoring or choosing its fold state.
    pub(super) fn install(&mut self, doc: Document) {
        self.install_arc(Arc::new(doc));
    }

    /// `install` for a document that is already shared (the other side of a comparison).
    pub(super) fn install_arc(&mut self, doc: Arc<Document>) {
        // Fold state is remembered per file in the checkout; the same file read at another
        // commit (Before) has other lines, so it neither stores nor restores one.
        let remembered = self
            .ed()
            .doc
            .as_ref()
            .filter(|d| d.at_rev.is_none())
            .zip(self.ed().folds.as_ref())
            .map(|(old, folds)| (old.path.clone(), folds.collapsed().clone()));
        if let Some((path, collapsed)) = remembered {
            self.fold_memory.insert(path, collapsed);
        }
        let stored = match doc.at_rev {
            None => self.fold_memory.remove(&doc.path),
            Some(_) => None,
        };
        let collapsed = stored.unwrap_or_else(|| {
            let _t = self.features.time(Feature::FoldBodies);
            let fold = self.fold_by_default && self.features.on(Feature::FoldBodies);
            if fold {
                doc.folded_at_open.iter().copied().collect()
            } else {
                Default::default()
            }
        });
        let folds = Folds::new(doc.folds.clone(), doc.lines.len(), collapsed);
        // A large JSON file shows at once; its folds follow (`start_late_folds`).
        let late = doc.folds_later && self.features.on(Feature::JsonFolds);
        let ed = self.ed_mut();
        ed.folds = Some(folds);
        ed.late_folds = if late {
            super::editor_state::LateFolds::Wanted
        } else {
            super::editor_state::LateFolds::None
        };
        ed.fold_all_later = None;
        ed.doc = Some(doc);
        ed.error = None;
        ed.caret = None;
        ed.marks.clear();
        ed.call_kinds.clear();
        ed.rev_index_ok = false;
        ed.side = Side::After;
        ed.gutter = None;
        ed.prepared = None;
        ed.decls = None;
        ed.diff = None;
        ed.last_decl = None;
        ed.epoch += 1;
        ed.source_scroll.scroll_to_item(0, ScrollStrategy::Top);
        // A tour step's highlighted chunks: their note strips and highlight group.
        self.sync_note_rows();
        // An open find bar finds its query in the new document.
        self.refind_after_load();
        self.hover.left();
        if !self.pane_override {
            let mut generation = self.context.generation;
            generation.next();
            self.context = ContextState {
                generation,
                ..ContextState::default()
            };
            self.outline_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
    }

    /// Stores the caret in the current history entry, so Back restores it exactly.
    pub(super) fn remember_caret(&mut self) {
        let caret = self.ed().caret;
        self.ed_mut().history.set_current_caret(caret);
    }

    pub(super) fn record_history(&mut self) {
        let entry = self.ed().doc.as_ref().map(|doc| Entry {
            path: doc.path.clone(),
            caret: self.ed().caret,
        });
        if let Some(entry) = entry {
            self.ed_mut().history.push(entry);
        }
    }

    /// The zero-based line the caret is on.
    pub(super) fn selected_line(&self) -> Option<usize> {
        let (doc, caret) = (self.ed().doc.as_ref()?, self.ed().caret?);
        Some(doc.buffer.line_of(caret))
    }

    /// Places the caret at `byte`, opening any fold that hides it and keeping it in
    /// view when `reveal` is set. The context pane follows.
    pub(super) fn set_caret(&mut self, byte: usize, reveal: bool, cx: &mut Context<Self>) {
        let reveal = if reveal { Reveal::Center } else { Reveal::No };
        self.place_caret(byte, reveal, cx);
    }

    /// Puts the active pane's caret at `byte` and scrolls as `reveal` says: the one place a
    /// caret move scrolls the code.
    pub(super) fn place_caret(&mut self, byte: usize, reveal: Reveal, cx: &mut Context<Self>) {
        let ed = self.ed_mut();
        let Some(doc) = &ed.doc else {
            return;
        };
        let byte = doc.buffer.clamp(byte);
        let line = doc.buffer.line_of(byte);
        ed.caret = Some(byte);
        // A caret placed anywhere drops the text selection (a press starts a new one after).
        ed.selection = None;
        ed.drag = None;
        let centre = match reveal {
            Reveal::No => false,
            Reveal::Center => true,
            Reveal::IfHidden => !ed.line_in_view(line),
            Reveal::Keep(anchor) => {
                let top = ed.keep_view(&anchor, line);
                let caret_row = ed.folds.as_ref().map_or(line, |f| f.row_near_line(line));
                if crate::devlog::on() {
                    // The caret's (or top line's) screen place before and after, in rows.
                    let before = match anchor.at {
                        super::editor_state::Anchor::Caret { screen_y } => {
                            format!("caret_at={}", (screen_y / anchor.viewport.row_h).round())
                        }
                        super::editor_state::Anchor::Top { line, .. } => format!("top_line={line}"),
                    };
                    eprintln!(
                        "LAND {before} top_row={} caret_row={caret_row}",
                        top.map_or("centre".to_string(), |t| t.to_string()),
                    );
                }
                top.is_none()
            }
        };
        if centre && let Some(folds) = &mut ed.folds {
            folds.reveal(line);
            ed.source_scroll
                .scroll_to_item(folds.row_near_line(line), ScrollStrategy::Center);
        }
        self.hover.left();
        self.refresh_context(cx);
        self.follow_link(cx);
    }

    /// Outlines what the caret just jumped to: the identifier it is on, or else its line.
    fn mark_landing(&mut self) {
        let ed = self.ed_mut();
        let (Some(doc), Some(caret)) = (&ed.doc, ed.caret) else {
            return;
        };
        let ids = &doc.analysis.identifiers;
        let i = ids.partition_point(|r| r.start <= caret);
        let range = i
            .checked_sub(1)
            .map(|i| ids[i].clone())
            .filter(|r| r.contains(&caret))
            .or_else(|| doc.buffer.line_range(doc.buffer.line_of(caret)));
        ed.landed = range.map(|r| (caret, r));
    }

    /// Moves the caret to a target: an exact byte, a symbol's name (resolved from the
    /// outline, never guessed) or a zero-based line.
    pub(super) fn apply_target(&mut self, target: &Target, cx: &mut Context<Self>) {
        let Some(doc) = &self.ed().doc else { return };
        let from_symbol = target
            .symbol
            .as_deref()
            .zip(doc.language)
            .and_then(|(s, l)| match find_symbol(l, doc.buffer.text(), s) {
                SymbolMatch::Unique(sym) => Some(sym.name_range.start),
                SymbolMatch::Ambiguous(all) => Some(all[0].name_range.start),
                SymbolMatch::NotFound => None,
            });
        // A link's symbol that is gone at another commit is reported by the banner; its
        // line (from that other commit) is not used, and nothing similar is tried instead.
        let missing = target.symbol.is_some() && from_symbol.is_none();
        let moved = target.from_link && missing && !self.link_at_head();
        let from_line = target.line.filter(|_| !moved).map(|l| {
            let l = l.min(doc.lines.len().saturating_sub(1));
            doc.buffer.line_range(l).map_or(0, |r| r.start)
        });
        let display = doc.display_path.clone();
        let jump = target.byte.or(from_symbol).or(from_line);
        if let Some(byte) = jump {
            self.set_caret(byte, true, cx);
        }
        if missing
            && !moved
            && let Some(symbol) = &target.symbol
        {
            self.ed_mut().error = Some(format!("`{symbol}` not found in {display}"));
        }
        if let Some(place) = &target.place {
            self.apply_place(place, cx);
        }
        if target.step {
            let ed = self.ed_mut();
            ed.landed = None;
            ed.step_caret = ed.caret;
        } else if jump.is_some() || target.place.is_some() {
            self.ed_mut().step_caret = None;
            self.mark_landing();
        }
        if target.from_link {
            self.ed_mut().link_found = Some(!missing);
            self.refresh_link_banner();
        }
        self.reveal_step_region();
        if target.step {
            self.center_caret();
        }
    }

    /// Scrolls the caret's row to the middle of the pane. A step opens folds (its region, its
    /// chunks) after the caret is placed; rows added above it would otherwise push it out of
    /// view.
    pub(super) fn center_caret(&mut self) {
        let ed = self.ed_mut();
        let (Some(doc), Some(folds), Some(caret)) = (&ed.doc, &ed.folds, ed.caret) else {
            return;
        };
        let row = folds.row_near_line(doc.buffer.line_of(caret));
        ed.source_scroll.scroll_to_item(row, ScrollStrategy::Center);
    }

    /// The index for the document on screen. A file read at another commit gets one only
    /// when the cached index was built at exactly that commit on a clean tree: the
    /// checkout's index is never applied to other code.
    pub(super) fn doc_index(&self) -> Option<&Arc<Semantic>> {
        self.doc_index_of(self.ed())
    }

    /// `doc_index` for one editor pane's document.
    pub(super) fn doc_index_of(&self, ed: &EditorState) -> Option<&Arc<Semantic>> {
        let IndexState::Ready(sem, _) = &self.index else {
            return None;
        };
        match ed.doc.as_ref().and_then(|d| d.at_rev.as_ref()) {
            Some(_) if !ed.rev_index_ok => None,
            _ => Some(sem),
        }
    }

    /// An in-file jump (outline click): records history.
    pub(super) fn jump_to_byte(&mut self, byte: usize, cx: &mut Context<Self>) {
        self.remember_caret();
        self.set_caret(byte, true, cx);
        self.record_history();
    }

    /// Opens `path` (relative to the folder) at `byte`, recording history. Used by the
    /// context pane's definition and reference rows.
    pub(super) fn navigate(&mut self, rel: &str, byte: usize, cx: &mut Context<Self>) {
        let path = self.root.clone().unwrap_or_default().join(rel);
        let target = Target {
            byte: Some(byte),
            ..Target::default()
        };
        self.open(path, target, true, cx);
    }

    /// The code pane header's Back and Forward buttons, greyed when there is nowhere to go.
    pub(super) fn history_buttons(&self, pane: PaneId, cx: &mut Context<Self>) -> gpui::AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        let ed = self.ed_of(pane);
        let (back, forward) = (ed.history.can_back(), ed.history.can_forward());
        let paired = self.links.iter().any(|l| l.has(pane));
        let button = |id: &'static str, icon: &'static str, on: bool, tip_text: String| {
            div()
                .id((id, pane.0 as usize))
                .w(px(20.))
                .h(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .tooltip(tip(tip_text))
                .when(on, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(c.hover))
                        .active(|s| s.bg(c.ghost_active))
                })
                .when(!on, |d| d.opacity(0.35))
                .child(cho_layout_ui::widgets::small_icon(icon, c.icon))
        };
        div()
            .flex()
            .items_center()
            .children(self.uncommitted_chip(pane, cx))
            .children(self.split_link_button(pane, cx))
            // A split column's header is narrow: its side chip stays whole, and Back / Forward
            // stay on their keys.
            .when(!paired, |d| {
                d.child(
                    button(
                        "history-back",
                        "chevron-left",
                        back,
                        self.tip_text("Back", "chorong::Back"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_editor(pane, cx);
                        this.go_back(cx);
                        cx.notify();
                    })),
                )
                .child(
                    button(
                        "history-forward",
                        "chevron-right",
                        forward,
                        self.tip_text("Forward", "chorong::Forward"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_editor(pane, cx);
                        this.go_forward(cx);
                        cx.notify();
                    })),
                )
            })
            .into_any_element()
    }

    pub(super) fn go_back(&mut self, cx: &mut Context<Self>) {
        // Over an external or action stop's card, Back returns to the file under it.
        if self
            .ed()
            .step_mark
            .as_ref()
            .is_some_and(|m| m.external.is_some() || m.action.is_some())
        {
            self.ed_mut().step_mark = None;
            let pane = self.active_editor;
            self.redraw(Some(super::shell_host::Redraw::Editor(pane)), cx);
            return;
        }
        self.remember_caret();
        if let Some(e) = self.ed_mut().history.back() {
            self.restore(e, cx);
        }
    }

    pub(super) fn go_forward(&mut self, cx: &mut Context<Self>) {
        self.remember_caret();
        if let Some(e) = self.ed_mut().history.forward() {
            self.restore(e, cx);
        }
    }

    pub(super) fn restore(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let target = Target {
            byte: entry.caret,
            ..Target::default()
        };
        self.open(entry.path, target, false, cx);
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        OpenPalette,
        OpenSymbols,
        OpenProjectSymbols,
        FindInFile,
        GoToLine,
        GoToDefinition,
        Back,
        Forward,
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c("chorong::OpenPalette", "Command Palette", Layer::Go, 20),
    c(
        "chorong::OpenSymbols",
        "Go to Symbol in File",
        Layer::Go,
        30,
    ),
    c(
        "chorong::OpenProjectSymbols",
        "Go to Symbol in Project",
        Layer::Go,
        40,
    ),
    c("chorong::GoToLine", "Go to Line", Layer::Go, 50),
    c("chorong::GoToDefinition", "Go to Definition", Layer::Go, 60),
    c("chorong::Back", "Go Back", Layer::Go, 80),
    c("chorong::Forward", "Go Forward", Layer::Go, 90),
    c("chorong::FindInFile", "Find in File", Layer::Go, 100),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, OpenPalette, |this, _w, cx| this.open_palette());
    let d = on!(d, cx, OpenSymbols, |this, _w, cx| this.open_symbols());
    let d = on!(d, cx, GoToDefinition, |this, _w, cx| this.go_to_definition(
        None,
        super::shell_host::OpenAt::Here,
        cx
    ));
    let d = on!(d, cx, FindInFile, |this, _w, cx| this.open_find(cx));
    let d = on!(d, cx, Back, |this, _w, cx| this.go_back(cx));
    let d = on!(d, cx, Forward, |this, _w, cx| this.go_forward(cx));
    unavailable!(
        d,
        cx,
        OpenProjectSymbols => "Go to symbol in project",
        GoToLine => "Go to line",
    )
}

/// How a caret move scrolls the code ([`Chorong::place_caret`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Reveal {
    /// Not at all.
    No,
    /// Only when the caret's line is not wholly in view (a linked column following).
    IfHidden,
    /// The caret's line to the middle (a jump).
    Center,
    /// The view kept where it was (a Before/After flip): see [`EditorState::anchor`].
    Keep(super::editor_state::ViewAnchor),
}

//! Format version 2 on screen: several highlighted chunks in one editor pane, each with a
//! numbered note strip just above its first line (owner's option A), and tour focus pages.
//! The pure parts (arrangements, numbering, which chunk is next) are in `crate::tour_pages`.

use super::*;

impl Chorong {
    /// Puts a note strip row above the first line of each of the active editor's highlighted
    /// chunks, and makes the chunks a highlight group (`Step 3 highlights`) so `n` / `N` move
    /// through them; takes both away when its step has none (or another file is on screen).
    pub(super) fn sync_note_rows(&mut self) {
        let palette = self.marker_colours.len();
        let ed = self.ed_mut();
        let (lines, chunks, number) = match (&ed.doc, &ed.step_mark) {
            (Some(doc), Some(mark)) if mark.applies_to(doc) => (
                mark.highlights
                    .iter()
                    .map(|h| doc.buffer.line_of(h.range.start))
                    .collect(),
                // `n` / `N` go to each chunk's own text: chunks that share a declaration
                // would merge into one stop if their declarations were used.
                mark.highlights.iter().map(|h| h.range.clone()).collect(),
                mark.number.clone(),
            ),
            _ => (Vec::new(), Vec::new(), String::new()),
        };
        if let Some(folds) = &mut ed.folds {
            folds.set_notes(lines);
        }
        let has_step = ed
            .marks
            .groups()
            .iter()
            .any(|g| matches!(g.mode, crate::marks::Mode::Step(_)));
        if has_step || !chunks.is_empty() {
            ed.marks.set_step(&number, chunks, palette);
        }
    }

    /// The note strip row `n` of `pane`'s editor: the chunk's number and the author's note
    /// (one line, the whole note as a tooltip), indented to the code.
    pub(super) fn render_note_row(
        &self,
        pane: PaneId,
        n: usize,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let c = crate::pane_style::colors(&self.theme, cx);
        let ed = self.ed_of(pane);
        let chunks: Vec<(usize, Option<String>)> = match (&ed.doc, &ed.folds, &ed.step_mark) {
            (Some(doc), Some(folds), Some(mark)) => {
                let line = folds.note_lines().get(n).copied();
                mark.highlights
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| Some(doc.buffer.line_of(h.range.start)) == line)
                    .map(|(i, h)| (i + 1, h.label.clone()))
                    .collect()
            }
            _ => Vec::new(),
        };
        let numbers = chunks
            .iter()
            .map(|(k, _)| k.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        // The labels on one line, emphasis, code and links styled; each on its own line in
        // the tooltip.
        let labels = || chunks.iter().filter_map(|(_, t)| t.as_deref());
        let text = crate::note_md::Text::join(labels(), " \u{b7} ");
        let whole = crate::note_md::Text::join(labels(), "\n");
        div()
            .id(("note-row", ix))
            .h(px(LINE_HEIGHT))
            .pl(px(GUTTER_WIDTH))
            .pr_2()
            .py(px(1.))
            .flex()
            .child(
                div()
                    .h_full()
                    .px(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .rounded(px(4.))
                    .bg(c.accent_faint)
                    .border_1()
                    .border_color(c.text_accent.opacity(0.35))
                    .font_family(c.ui_font.clone())
                    .text_size(px(11.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c.text_accent)
                            .child(numbers),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(c.text)
                            .child(super::note_text_in(&text, &c)),
                    ),
            )
            .when(!text.text.is_empty(), |d| d.tooltip(super::note_tip(whole)))
    }
}

// ---- Tour focus pages ----

use super::investigation_view::EditorAt;
use super::shell_host::Redraw;
use crate::tour_pages::{ActionCard, DiffAt, PaneSpec, View};
use cho_investigation::Kind;
use cho_layout::{Node, PageOrigin, PaneContent, PaneKind, PaneSource, Target as LayoutTarget};
use std::collections::HashMap;

/// A tour opened as focus pages: one page per step, made when first shown.
#[derive(Default)]
pub(crate) struct TourPages {
    /// Each built page's tree as Chorong laid it out, by step id: a page whose tree differs now
    /// was rearranged by the reader (kept for the session; Reset page restores it).
    built: HashMap<String, Node>,
    /// Panes Chorong opened for the pages, closed again when the focus layout ends.
    created: Vec<PaneId>,
    /// What each card pane (broken anchor, external node, graph until the graph pane lands)
    /// shows, by its placeholder name.
    pub cards: HashMap<String, PaneSpec>,
    /// The tour rail is collapsed to badges.
    pub rail_collapsed: bool,
    /// The rail is shown at all: a comparison row opened from its pane (Enter) has none;
    /// opened as a tour it has one.
    pub rail: bool,
    /// Graph cards whose graph pane was given its spec (again after the index changes).
    graphs_set: std::collections::HashSet<String>,
}

impl Chorong {
    /// The tour's steps as the pages need them: the view each step asks for, else the
    /// default page.
    fn step_view(step: &crate::investigation::Step) -> View {
        if let Some(card) = &step.extras.action {
            return crate::tour_pages::action_view(card.clone(), step.extras.view.as_ref());
        }
        if let Some(v) = &step.extras.view {
            return v.clone();
        }
        let at = step.symbol.clone().unwrap_or_default();
        let broken = (step.label == crate::investigation::Label::Broken && !step.is_external())
            .then(|| step.reason.clone().unwrap_or_default());
        let about = step.extras.external.as_ref().map(|what| match &step.note {
            Some(n) => format!("{what}. {n}"),
            None => format!("{what}."),
        });
        crate::tour_pages::default_view(
            &step.path,
            step.caret(),
            step.region.clone(),
            &at,
            about.as_deref().map(|a| (step.title.as_str(), Some(a))),
            broken.as_deref(),
        )
    }

    /// The step id each focus page was made for (`None` for the reader's own pages).
    fn page_steps(&self, cx: &App) -> Vec<Option<String>> {
        self.shell
            .read(cx)
            .workspace()
            .focus()
            .map(|f| {
                f.pages
                    .iter()
                    .map(|p| p.origin.as_ref().and_then(|o| o.tour_step.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A graph node clicked while the tour has pages: when it is one of the tour's stops (same
    /// file, inside its region), the active editor shows it as that stop. Returns whether it did.
    pub(crate) fn tour_node_clicked(
        &mut self,
        loc: &super::graph_pane::CodeLoc,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(st) = self.inv.as_ref().filter(|s| s.pages.is_some()) else {
            return false;
        };
        let at = loc.range.as_ref().map(|r| r.start);
        let Some(step) = st
            .steps
            .iter()
            .find(|s| {
                s.path == loc.path
                    && match (&s.region, at) {
                        (Some(g), Some(b)) => g.contains(&b),
                        (Some(_), None) => s.line == Some(loc.line),
                        _ => false,
                    }
            })
            .cloned()
        else {
            return false;
        };
        let editor = EditorAt {
            path: step.path.clone(),
            caret: step.caret(),
            region: step.region.clone(),
            highlights: Vec::new(),
            cell: None,
            diff: None,
        };
        self.show_step_in_active(&step, editor, cx);
        cx.notify();
        true
    }

    /// The index changed: graph cards resolve their symbol again when next drawn.
    pub(super) fn tour_graphs_stale(&mut self) {
        if let Some(p) = self.inv.as_mut().and_then(|s| s.pages.as_mut()) {
            p.graphs_set.clear();
        }
    }

    /// Each tour page's tree, by step id.
    fn page_roots(&self, cx: &App) -> HashMap<String, Option<Node>> {
        let ws = self.shell.read(cx).workspace();
        ws.focus()
            .map(|f| {
                f.pages
                    .iter()
                    .filter_map(|p| {
                        let id = p.origin.as_ref()?.tour_step.clone()?;
                        Some((id, p.root.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Open Tour as Focus Pages: a focus layout with one page per step, at the current step.
    pub(super) fn open_tour_pages(&mut self, cx: &mut Context<Self>) {
        if self
            .inv
            .as_ref()
            .is_some_and(|s| *s.kind() == Kind::Comparison)
        {
            self.notify_text("This is a comparison: use Open Comparison as Tour", cx);
            return;
        }
        self.open_tour_pages_with(true, cx);
    }

    /// Open Comparison as Tour: one page per row, both sides side by side, with the rail.
    pub(super) fn open_comparison_tour(&mut self, cx: &mut Context<Self>) {
        if self
            .inv
            .as_ref()
            .is_none_or(|s| *s.kind() != Kind::Comparison)
        {
            self.notify_text("No comparison is open: use Open Investigation\u{2026}", cx);
            return;
        }
        if self.shell.read(cx).workspace().in_focus()
            && self.inv.as_ref().is_some_and(|s| s.pages.is_some())
        {
            // The row's page is open already: show the rail.
            if let Some(p) = self.inv.as_mut().and_then(|s| s.pages.as_mut()) {
                p.rail = true;
            }
            self.redraw_step_headers(cx);
            return;
        }
        self.open_tour_pages_with(true, cx);
    }

    /// The tour's (or comparison's) focus pages, at the current step: one page per step it
    /// reaches. With `rail`, the stops are listed beside every page.
    pub(super) fn open_tour_pages_with(&mut self, rail: bool, cx: &mut Context<Self>) {
        let Some(st) = self
            .inv
            .as_ref()
            .filter(|s| matches!(s.kind(), Kind::Tour | Kind::Comparison))
        else {
            self.notify_text("No tour is open: use Open Investigation\u{2026}", cx);
            return;
        };
        if st.at.is_none() || st.steps.is_empty() {
            self.notify_text("The tour has no steps to show yet", cx);
            return;
        }
        if self.shell.read(cx).workspace().in_focus() {
            self.notify_text(
                "Restore the layout first (Esc), then open the tour's pages",
                cx,
            );
            return;
        }
        let steps = st.steps.clone();
        let current = st.current;
        let key = crate::investigation::source_key(&st.source);
        let commit = pin_commit(st);
        let first = self.active_editor;
        let now = crate::investigation::now_secs();
        let mut pages = TourPages {
            rail,
            ..TourPages::default()
        };
        // A step the chosen branch does not reach has no page: paging skips it.
        let current = steps
            .iter()
            .take(current)
            .filter(|s| s.extras.not_reached.is_none())
            .count();
        let origins: Vec<PageOrigin> = steps
            .iter()
            .filter(|s| s.extras.not_reached.is_none())
            .map(|s| {
                let view = Self::step_view(s);
                let names = crate::tour_pages::card_names(&view, &s.number);
                let panes = view
                    .panes
                    .iter()
                    .zip(names)
                    .map(|(spec, name)| match name {
                        Some(name) => {
                            pages.cards.insert(name.clone(), spec.clone());
                            PaneSource::Placeholder { name }
                        }
                        None => spec_source(spec, &key, &commit),
                    })
                    .collect();
                PageOrigin {
                    tour_step: Some(s.id.clone()),
                    template: Some(view.arrangement.name().to_string()),
                    panes,
                    arrangement: view.arrangement,
                }
            })
            .collect();
        let result = self.shell.update(cx, |shell, cx| {
            shell.update_workspace(cx, |ws| {
                ws.batch(|ws| {
                    ws.maximize(first, now)?;
                    for o in origins {
                        ws.new_page(Some(o))?;
                    }
                    // The page the maximised pane made is not a step: its pane waits in the
                    // tray.
                    ws.close_page(0, now)?;
                    ws.go_to_page(current)
                })
            })
        });
        if let Err(e) = result {
            self.notify_text(format!("Could not open the tour's pages: {e}"), cx);
            return;
        }
        if let Some(st) = &mut self.inv {
            st.pages = Some(pages);
        }
        self.sync_panes(cx);
        self.tour_pages_follow(cx);
    }

    /// On tour pages, stepping the tour shows the step's page. Returns whether it did.
    pub(super) fn tour_page_show(&mut self, ix: usize, cx: &mut Context<Self>) -> bool {
        let Some(st) = self.inv.as_ref().filter(|s| s.pages.is_some()) else {
            return false;
        };
        if !self.shell.read(cx).workspace().in_focus() {
            return false;
        }
        let Some(id) = st.steps.get(ix).map(|s| s.id.clone()) else {
            return false;
        };
        let pages = self.page_steps(cx);
        let Some(page) = pages.iter().position(|p| p.as_deref() == Some(id.as_str())) else {
            return false;
        };
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| ws.go_to_page(page));
        });
        self.tour_pages_follow(cx);
        true
    }

    /// After any layout change while a tour has pages: paging steps the tour (a tour page
    /// shown makes its step current), a tour page shown for the first time is built, and when
    /// the focus layout ends the panes Chorong opened for it close and the step stays.
    pub(super) fn tour_pages_follow(&mut self, cx: &mut Context<Self>) {
        if self.inv.as_ref().is_none_or(|s| s.pages.is_none()) {
            return;
        }
        let (in_focus, page, step_id, empty) = {
            let ws = self.shell.read(cx).workspace();
            match ws.focus() {
                Some(f) => (
                    true,
                    f.current,
                    f.page().origin.as_ref().and_then(|o| o.tour_step.clone()),
                    f.page().root.is_none(),
                ),
                None => (false, 0, None, false),
            }
        };
        if !in_focus {
            self.tour_pages_ended(cx);
            return;
        }
        let _ = page;
        let Some(id) = step_id else { return };
        let Some(st) = &mut self.inv else { return };
        let Some(ix) = st.steps.iter().position(|s| s.id == id) else {
            return;
        };
        let moved = st.current != ix;
        if moved {
            st.current = ix;
            st.selected = ix;
            let src = st.source.clone();
            if let Some(store) = crate::investigation::store() {
                let _ = store.set_place(&src, &id, crate::investigation::now_secs());
            }
        }
        if empty {
            self.build_tour_page(ix, cx);
        }
        // Only on a change: a redraw syncs the panes again, which comes back here.
        if moved || empty {
            self.redraw(Some(Redraw::Steps), cx);
            self.redraw_step_headers(cx);
            self.redraw_chrome(cx);
        }
    }

    /// Lays the current page out from step `ix`'s view, by its arrangement (`reset_page`):
    /// a fresh editor per declared editor pane (so other pages keep theirs), each at its
    /// anchor with its highlights; the shared panes (Context, Outline, Call stack, tree) come
    /// to the page; cards for broken anchors and external nodes.
    fn build_tour_page(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(st) = &self.inv else { return };
        let Some(step) = st.steps.get(ix).cloned() else {
            return;
        };
        let root = st.root.clone();
        let commit = pin_commit(st);
        let view = Self::step_view(&step);
        let pins: Vec<cho_layout::Pin> = view
            .panes
            .iter()
            .filter_map(|p| spec_pin(p, &commit))
            .collect();
        let row = step.extras.row.clone();
        let editors: Vec<EditorAt> = view
            .panes
            .iter()
            .filter_map(|p| match p {
                PaneSpec::Editor {
                    path,
                    caret,
                    region,
                    highlights,
                    ..
                } => Some(EditorAt {
                    path: path.clone(),
                    caret: *caret,
                    region: region.clone(),
                    highlights: highlights.clone(),
                    cell: None,
                    diff: None,
                }),
                PaneSpec::Diff(d) => Some(EditorAt {
                    path: d.path.clone(),
                    caret: None,
                    region: None,
                    highlights: Vec::new(),
                    cell: None,
                    diff: Some(d.clone()),
                }),
                _ => None,
            })
            .enumerate()
            // A comparison row's editors are its sides', in order.
            .map(|(k, mut e)| {
                e.cell = row
                    .as_ref()
                    .and_then(|r| Some((r.clone(), *r.editor_cells.get(k)?)));
                e
            })
            .collect();
        let available = move |s: &PaneSource| match s {
            PaneSource::Editor { path } => root.join(path).is_file(),
            PaneSource::Pin { .. } | PaneSource::Graph { .. } => false,
            _ => true,
        };
        let roots_before = self.page_roots(cx);
        let result = self.shell.update(cx, |shell, cx| {
            shell.update_workspace(cx, |ws| lay_out_tour_page(ws, &editors, &pins, available))
        });
        let (created, root, missing, page_editors) = match result {
            Ok(r) => r,
            Err(e) => {
                self.notify_text(format!("Could not lay out this page: {e}"), cx);
                return;
            }
        };
        if crate::devlog::on() {
            eprintln!(
                "INV page step={} arrangement={} panes={} cards={} missing={}",
                step.id,
                view.arrangement.name(),
                root.as_ref().map_or(0, |r| r.panes().len()),
                view.panes
                    .iter()
                    .filter(|p| p.card_title().is_some())
                    .count(),
                missing.len()
            );
        }
        // A shared pane (Context, Outline…) this page took from another tour page changed that
        // page too: Chorong did that, not the reader, so it is not "rearranged".
        let roots_after = self.page_roots(cx);
        if let Some(pages) = self.inv.as_mut().and_then(|s| s.pages.as_mut()) {
            pages.created.extend(created);
            for (id, now) in &roots_after {
                if roots_before.get(id) != Some(now)
                    && *id != step.id
                    && pages.built.contains_key(id)
                    && let Some(now) = now
                {
                    pages.built.insert(id.clone(), now.clone());
                }
            }
            if let Some(r) = root {
                pages.built.insert(step.id.clone(), r);
            }
        }
        self.sync_panes(cx);
        // Editors were declared in order, and the page keeps that order.
        for (pane, at) in page_editors.into_iter().zip(editors) {
            self.with_pane(pane, |this| this.show_step_in_active(&step, at, cx));
        }
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(|(_, s)| s.label()).collect();
            self.notify_text(format!("Not available: {}", names.join(", ")), cx);
        }
        cx.notify();
    }

    /// Reset page on a tour page: lays it out again from the step's view. Returns whether
    /// the current page is a tour page (otherwise the layout's own reset applies).
    pub(super) fn reset_tour_page(&mut self, cx: &mut Context<Self>) -> bool {
        if self.inv.as_ref().is_none_or(|s| s.pages.is_none()) {
            return false;
        }
        let on_page: Vec<PaneId> = {
            let ws = self.shell.read(cx).workspace();
            match ws.focus() {
                Some(f)
                    if f.page()
                        .origin
                        .as_ref()
                        .is_some_and(|o| o.tour_step.is_some()) =>
                {
                    f.on_screen()
                }
                _ => return false,
            }
        };
        let created: Vec<PaneId> = self
            .inv
            .as_ref()
            .and_then(|s| s.pages.as_ref())
            .map(|p| p.created.clone())
            .unwrap_or_default();
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| {
                ws.batch(|ws| {
                    for p in &on_page {
                        if created.contains(p) {
                            ws.close(*p)?;
                        } else {
                            ws.move_pane(*p, LayoutTarget::Tray)?;
                        }
                    }
                    Ok(())
                })
            });
        });
        if let Some(pages) = self.inv.as_mut().and_then(|s| s.pages.as_mut()) {
            pages.created.retain(|p| !on_page.contains(p));
        }
        self.sync_panes(cx);
        self.tour_pages_follow(cx);
        true
    }

    /// The focus layout ended: the panes Chorong opened for the pages close, except the editor
    /// you were reading in, which keeps your place; the step stays current.
    fn tour_pages_ended(&mut self, cx: &mut Context<Self>) {
        let Some(pages) = self.inv.as_mut().and_then(|s| s.pages.take()) else {
            return;
        };
        for key in &pages.graphs_set {
            self.graph_pane_drop(key);
        }
        let keep = self.active_editor;
        let close: Vec<PaneId> = pages.created.into_iter().filter(|p| *p != keep).collect();
        if crate::devlog::on() {
            let step = self
                .inv
                .as_ref()
                .and_then(|s| s.steps.get(s.current))
                .map(|s| s.id.clone())
                .unwrap_or_default();
            eprintln!("INV pages ended step={step} closed={}", close.len());
        }
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| {
                for p in &close {
                    if ws.content(*p).is_some() {
                        let _ = ws.close(*p);
                    }
                }
                Ok(())
            });
        });
        self.sync_panes(cx);
        self.redraw(Some(Redraw::Steps), cx);
    }

    /// An action step's card on a tour page.
    fn action_card_body(
        &mut self,
        card: &Arc<ActionCard>,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let st = self.inv.as_ref()?;
        let step = st.steps.iter().find(|s| {
            s.extras
                .action
                .as_ref()
                .is_some_and(|a| Arc::ptr_eq(a, card))
        })?;
        let heading = format!("{} \u{b7} step {} of {}", card.name, step.n, st.steps.len());
        let number = step.number.clone();
        // Ids apart from editor panes' (theirs are below 1 << 20).
        let scope = (1 << 20) + step.n;
        Some(self.render_action_card(card, &number, heading, scope, cx))
    }

    /// A card pane on a tour page: an anchor that did not resolve, a node that is not in the
    /// code, or a graph (a placeholder until the graph pane lands).
    pub(super) fn render_tour_card(
        &mut self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let spec = self.inv.as_ref()?.pages.as_ref()?.cards.get(name)?.clone();
        if let PaneSpec::Graph {
            of,
            direction,
            depth,
            path,
            at,
        } = &spec
        {
            if !self
                .inv
                .as_ref()
                .and_then(|s| s.pages.as_ref())
                .is_some_and(|p| p.graphs_set.contains(name))
            {
                // The exact SCIP symbol at the anchor; with no index the pane says what it
                // needs (Build index).
                let scope = match (&self.root, path, at) {
                    (Some(root), Some(path), Some(at)) => self.symbol_scope(&root.join(path), *at),
                    _ => None,
                };
                let (symbol, label) = match scope {
                    Some(crate::graph::Scope::Symbol { symbol, label }) => (symbol, label),
                    _ => (of.clone(), of.clone()),
                };
                let direction = match direction.as_str() {
                    "in" => cho_graph::Direction::In,
                    "both" => cho_graph::Direction::Both,
                    _ => cho_graph::Direction::Out,
                };
                let spec = super::graph_pane::GraphSpec::neighbourhood(
                    symbol,
                    label,
                    direction,
                    *depth as usize,
                );
                self.graph_pane_set(name, spec, cx);
                if let Some(p) = self.inv.as_mut().and_then(|s| s.pages.as_mut()) {
                    p.graphs_set.insert(name.to_string());
                }
            }
            return Some(self.render_graph_pane(name, cx));
        }
        let c = crate::pane_style::colors(&self.theme, cx);
        let error = c.error.unwrap_or(c.text_accent);
        let body = super::action_card::card_shell(460., 6.);
        let body = match spec {
            PaneSpec::Broken { asked, reason } => broken_card(body, &asked, &reason, error, &c),
            PaneSpec::Missing { side, reason } => missing_card(body, &side, &reason, &c),
            PaneSpec::Action(card) => return self.action_card_body(&card, cx),
            PaneSpec::DiffUnresolved { diff, reason } => {
                diff_unresolved_card(body, &diff, &reason, error, &c)
            }
            PaneSpec::External { title, note } => {
                return Some(external_card(
                    &title,
                    "",
                    note.map(|n| div().child(n).into_any_element()),
                    None,
                    &[],
                    &c,
                ));
            }
            _ => return None,
        };
        Some(div().size_full().child(body).into_any_element())
    }
}

/// The card for an editor anchor that did not resolve.
fn broken_card(
    body: Div,
    asked: &str,
    reason: &str,
    error: gpui::Hsla,
    c: &cho_layout_ui::colors::Colors,
) -> Div {
    body.border(px(1.5))
        .border_dashed()
        .border_color(error)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_color(error)
                .font_weight(FontWeight::MEDIUM)
                .child(small_icon("triangle-alert", error))
                .child("Anchor not found"),
        )
        .child(format!("The tour asked for an editor at {asked}."))
        .child(
            div()
                .text_color(c.text_muted)
                .child(format!("{reason}. Nothing was put in its place.")),
        )
}

/// The card for a comparison side that has no step.
fn missing_card(body: Div, side: &str, reason: &str, c: &cho_layout_ui::colors::Colors) -> Div {
    body.border(px(1.5))
        .border_dashed()
        .border_color(c.border)
        .bg(c.panel)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .font_weight(FontWeight::MEDIUM)
                .child(div().font_weight(FontWeight::BOLD).child("\u{2205}"))
                .child(format!("{side}: no step for this row")),
        )
        .child(
            div()
                .italic()
                .text_color(c.text_muted)
                .child(format!("{reason} (the author's words)")),
        )
}

/// The card for a diff pane whose revision did not resolve.
fn diff_unresolved_card(
    body: Div,
    diff: &DiffAt,
    reason: &str,
    error: gpui::Hsla,
    c: &cho_layout_ui::colors::Colors,
) -> Div {
    body.border(px(1.5))
        .border_dashed()
        .border_color(error)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_color(error)
                .font_weight(FontWeight::MEDIUM)
                .child(small_icon("git-compare", error))
                .child("Diff not shown"),
        )
        .child(format!(
            "The tour asked for the changes to {}, {}.",
            diff.path,
            diff.sides()
        ))
        .child(div().text_color(c.text_muted).child(format!(
            "{reason}. Chorong does not guess another revision."
        )))
}

/// Lays a tour page out in one undoable batch: opens its fresh editors and pins, resets the
/// page from its arrangement, closes the fresh editors that found no place and returns
/// (panes created, the page's layout root, sources left out, the page's editors in order).
#[allow(clippy::type_complexity)]
fn lay_out_tour_page(
    ws: &mut cho_layout::Workspace,
    editors: &[EditorAt],
    pins: &[cho_layout::Pin],
    available: impl Fn(&PaneSource) -> bool,
) -> cho_layout::Result<(
    Vec<PaneId>,
    Option<cho_layout::Node>,
    Vec<(usize, PaneSource)>,
    Vec<PaneId>,
)> {
    ws.batch(|ws| {
        let page = ws.focus().map(|f| f.current).unwrap_or(0);
        let mut fresh = Vec::new();
        for e in editors {
            fresh.push(ws.open(
                PaneContent::Editor {
                    path: e.path.clone(),
                    caret: e.caret.unwrap_or(0),
                },
                Some(LayoutTarget::Tray),
            )?);
        }
        for pin in pins {
            fresh.push(ws.open(
                PaneContent::PinnedContext(pin.clone()),
                Some(LayoutTarget::Tray),
            )?);
        }
        let report = ws.reset_page(page, available)?;
        let on_page = ws.focus().map(|f| f.on_screen()).unwrap_or_default();
        // Fresh editors that found no place (a file that is gone) close again.
        for p in &fresh {
            if !on_page.contains(p) {
                ws.close(*p)?;
            }
        }
        if let Some(first) = on_page.first() {
            let _ = ws.activate(*first);
        }
        let mut created: Vec<PaneId> = fresh.into_iter().filter(|p| on_page.contains(p)).collect();
        created.extend(report.created);
        // The page's editors in the order declared (reset keeps that order).
        let page_editors: Vec<PaneId> = on_page
            .iter()
            .copied()
            .filter(|p| ws.kind(*p) == Some(PaneKind::Editor))
            .collect();
        let root = ws.focus().and_then(|f| f.root()).cloned();
        Ok((created, root, report.missing, page_editors))
    })
}

/// A node that is not in the code, as a card: its label, what it is, the author's note,
/// evidence and runtime values, and "not in the code". On a tour page in place of an
/// editor, and in an editor while such a stop is current.
pub(crate) fn external_card(
    title: &str,
    what: &str,
    note: Option<gpui::AnyElement>,
    evidence: Option<crate::tour_pages::Evidence>,
    runtime: &[String],
    c: &cho_layout_ui::colors::Colors,
) -> gpui::AnyElement {
    use super::investigation_view::{evidence_tag, runtime_chip};
    let kind = if what.is_empty() {
        "not in the code".to_string()
    } else {
        format!("not in the code \u{b7} {what}")
    };
    div()
        .size_full()
        .child(
            super::action_card::card_shell(460., 6.)
                .border_1()
                .border_dashed()
                .border_color(c.border)
                .bg(c.panel)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .font_weight(FontWeight::MEDIUM)
                        .child(small_icon("box", c.icon))
                        .child(title.to_string())
                        .children(evidence.map(|e| evidence_tag(e, c))),
                )
                .child(
                    div()
                        .italic()
                        .text_size(px(11.5))
                        .text_color(c.text_muted)
                        .child(kind),
                )
                .when(!runtime.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .children(runtime.iter().map(|v| runtime_chip(v, c))),
                    )
                })
                .children(note),
        )
        .into_any_element()
}

/// The commit a page's pins are taken at: the pinned commit when that code is shown, else
/// the checkout's HEAD.
fn pin_commit(st: &super::investigation_view::InvState) -> String {
    match &st.at {
        Some(crate::investigation::CodeAt::Pinned(rev)) => cho_git::short(rev).to_string(),
        _ => cho_git::rev_parse(&st.root, "HEAD")
            .map(|c| cho_git::short(&c).to_string())
            .unwrap_or_else(|| "unknown".into()),
    }
}

/// The pin a Context pane naming a symbol shows on a page.
fn spec_pin(spec: &PaneSpec, commit: &str) -> Option<cho_layout::Pin> {
    match spec {
        PaneSpec::Context {
            symbol: Some(symbol),
            path: Some(path),
            at: Some(at),
        } => Some(cho_layout::Pin {
            symbol: symbol.clone(),
            symbol_kind: "named by the tour".into(),
            path: path.clone(),
            byte_offset: *at,
            commit: commit.to_string(),
            content_hash: None,
        }),
        _ => None,
    }
}

/// The layout's record of a declared pane that is not a card, for Reset page and Reopen
/// focus layout.
fn spec_source(spec: &PaneSpec, investigation: &str, commit: &str) -> PaneSource {
    if let Some(pin) = spec_pin(spec, commit) {
        return PaneSource::Pin {
            symbol: pin.symbol,
            commit: pin.commit,
        };
    }
    match spec {
        PaneSpec::Editor { path, .. } => PaneSource::Editor { path: path.clone() },
        PaneSpec::Diff(d) => PaneSource::Editor {
            path: d.path.clone(),
        },
        PaneSpec::Outline { .. } => PaneSource::Outline,
        PaneSpec::FileTree => PaneSource::FileTree,
        PaneSpec::CallStack => PaneSource::CallStack {
            investigation: investigation.to_string(),
        },
        // Context, and cards (named by the caller).
        _ => PaneSource::Context,
    }
}

use cho_layout_ui::widgets::small_icon;

actions!(chorong, [OpenTourPages]);

pub(super) const COMMANDS: &[Command] = &[c(
    "chorong::OpenTourPages",
    "Open Tour as Focus Pages",
    Layer::Steps,
    357,
)];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    on!(d, cx, OpenTourPages, |this, _w, cx| this
        .open_tour_pages(cx))
}

/// The tour rail's width, open and collapsed to badges.
const RAIL_W: f32 = 210.;
const RAIL_COLLAPSED_W: f32 = 34.;

impl Chorong {
    /// The page strip's chip for page `i` while the tour has pages: the step's badge (done,
    /// current, to do, broken), a dot when you rearranged it, and your own pages dashed after
    /// a divider.
    pub(super) fn tour_page_chip(
        &self,
        i: usize,
        ws: &cho_layout::Workspace,
    ) -> Option<cho_layout_ui::shell::PageChip> {
        use cho_layout_ui::shell::{ChipLook, PageChip};
        let st = self.inv.as_ref()?;
        let pages = st.pages.as_ref()?;
        let f = ws.focus()?;
        let step_of = |p: usize| {
            f.pages
                .get(p)?
                .origin
                .as_ref()?
                .tour_step
                .as_deref()
                .and_then(|id| st.steps.iter().position(|s| s.id == id))
        };
        let page = f.pages.get(i)?;
        let Some(ix) = step_of(i) else {
            let after_tour = i > 0 && step_of(i - 1).is_some();
            return Some(PageChip {
                label: (i + 1).to_string().into(),
                look: if i == f.current {
                    ChipLook::Current
                } else {
                    ChipLook::Own
                },
                edited: false,
                divider_before: after_tour,
                tooltip: Some(
                    format!("Page {}, your own (stepping the tour skips it)", i + 1).into(),
                ),
            });
        };
        let step = &st.steps[ix];
        let state = crate::investigation::stop_state(step, ix, st.current);
        let edited = match (pages.built.get(&step.id), &page.root) {
            (Some(built), Some(now)) => built != now,
            _ => false,
        };
        let look = match state {
            _ if i == f.current => ChipLook::Current,
            crate::investigation::StopState::Done => ChipLook::Done,
            crate::investigation::StopState::Broken if !step.no_code() => ChipLook::Broken,
            _ => ChipLook::ToDo,
        };
        let mut tip = format!("Step {} \u{b7} {}", step.number, step.list_name());
        if edited {
            tip.push_str(" \u{b7} rearranged by you: Reset page restores it");
        }
        Some(PageChip {
            label: step.number.clone().into(),
            look,
            edited,
            divider_before: false,
            tooltip: Some(tip.into()),
        })
    }

    /// One stop of the tour rail: its badge, name and (on a comparison) result.
    fn render_rail_row(
        ix: usize,
        s: &crate::investigation::Step,
        current_ix: usize,
        collapsed: bool,
        c: &cho_layout_ui::colors::Colors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        use super::investigation_view::{BadgeLook, step_badge};
        let state = crate::investigation::stop_state(s, ix, current_ix);
        let off = s.extras.not_reached.is_some();
        let look = if off || (s.no_code() && state == crate::investigation::StopState::Broken) {
            BadgeLook::ToDo
        } else {
            BadgeLook::from(state)
        };
        let current = ix == current_ix;
        let name = match &s.extras.not_reached {
            Some(why) => format!("{} \u{b7} not reached: {why}", s.list_name()),
            None => s.list_name().to_string(),
        };
        div()
            .id(("rail-stop", ix))
            .when(off, |d| d.opacity(0.55).italic())
            .h(px(28.))
            .px(px(if collapsed { 0. } else { 6. }))
            .flex()
            .items_center()
            .when(collapsed, |d| d.justify_center())
            .gap(px(6.))
            .rounded(px(4.))
            .cursor_pointer()
            .when(current && !collapsed, |d| d.bg(c.selected))
            .hover(|d| d.bg(c.hover))
            .child(step_badge(s.number.clone(), look, c))
            .when(!collapsed, |d| {
                d.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .when(current, |d| d.font_weight(FontWeight::MEDIUM))
                        .child(name.clone()),
                )
            })
            .when(collapsed || off, |d| d.tooltip(tip(name.clone())))
            // A comparison row's result, by its glyph.
            .when_some(s.extras.row.clone().filter(|_| !collapsed), |d, r| {
                d.child(div().flex_1()).child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(match r.outcome {
                            crate::comparison::Outcome::Differs => {
                                c.warning.unwrap_or(c.text_muted)
                            }
                            _ => c.text_muted,
                        })
                        .child(format!("{} {}", r.outcome.glyph(), r.outcome.word())),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.step_clicked(ix, cx)))
    }

    /// The tour rail beside every page while the tour has pages: its stops, a click going to
    /// the stop's page; collapsed, a column of badges.
    pub(super) fn render_tour_rail(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let st = self.inv.as_ref()?;
        let collapsed = st.pages.as_ref().filter(|p| p.rail)?.rail_collapsed;
        let c = crate::pane_style::colors(&self.theme, cx);
        let toggle = div()
            .id("tour-rail-toggle")
            .flex_none()
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|d| d.bg(c.hover))
            .child(small_icon(
                if collapsed {
                    "chevron-right"
                } else {
                    "chevron-left"
                },
                c.icon,
            ))
            .tooltip(tip(if collapsed {
                "Show the tour"
            } else {
                "Collapse the tour to badges"
            }
            .to_string()))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(p) = this.inv.as_mut().and_then(|s| s.pages.as_mut()) {
                    p.rail_collapsed = !p.rail_collapsed;
                }
                this.redraw_step_headers(cx);
            }));
        let choice = if collapsed {
            None
        } else {
            self.render_rail_choice(cx)
        };
        let rows = st
            .steps
            .iter()
            .enumerate()
            .map(|(ix, s)| Self::render_rail_row(ix, s, st.current, collapsed, &c, cx));
        Some(
            div()
                .flex_none()
                .w(px(if collapsed { RAIL_COLLAPSED_W } else { RAIL_W }))
                .h_full()
                .flex()
                .flex_col()
                .gap(px(2.))
                .p(px(if collapsed { 4. } else { 6. }))
                .bg(c.panel)
                .border_r_1()
                .border_color(c.border)
                .text_size(px(12.))
                .text_color(c.text)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .when(collapsed, |d| d.justify_center())
                        .gap_1()
                        .pb(px(4.))
                        .when(!collapsed, |d| {
                            d.child(small_icon("map", c.icon))
                                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child("Tour"))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(c.text_muted)
                                        .child(st.position().unwrap_or_default()),
                                )
                        })
                        .child(toggle),
                )
                .children(choice)
                .child(
                    self.scroll_areas
                        .get("tour-rail-stops")
                        .vertical(
                            "tour-rail-stops",
                            self.features.probe_if_on(Feature::Overscroll),
                            div().flex().flex_col().gap(px(1.)).children(rows),
                        )
                        .flex_1()
                        .min_h_0(),
                )
                .into_any_element(),
        )
    }
}

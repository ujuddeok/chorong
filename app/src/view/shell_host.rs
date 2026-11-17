//! The viewer's side of the layout shell (`cho_layout_ui::shell::Shell`): what each pane shows,
//! the default arrangement, and the saved layout per repository.
//!
//! Every Editor pane has its own `EditorState`; `sync_panes` keeps them in step with the layout
//! and picks the active editor, which the Context and Outline panes, the status bar and the
//! commands follow.

use super::*;
use cho_layout::{DockId, Node, PaneContent, PaneId, PaneKind, Pin, Workspace};
use cho_layout_ui::shell::{Badge, BadgeTone, PageChip, PaneBodies};
use gpui::{AnyElement, WeakEntity};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Left dock width and right dock width in the default arrangement.
const LEFT_WIDTH: f32 = 260.;
const RIGHT_WIDTH: f32 = 330.;
/// The Outline's share of the right dock's height.
const OUTLINE_SHARE: f32 = 0.38;

/// How long the caret rests before the saved layout records it.
const CARET_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(800);

/// What a change redraws, besides the status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Redraw {
    /// One editor pane's state changed.
    Editor(PaneId),
    /// Only what the Context pane and pins show (an index answer, highlighted previews).
    Context,
    /// Only the Changes list (its rows, selection or keyboard focus).
    Changes,
    /// Only the Tour and Call stack panes, and the file tree a tour filters.
    Steps,
}

/// The panes `what` redraws: the editor pane, plus Context, Outline and Highlights when it is
/// the active editor (they follow it); or the Context pane and every pin; or the Changes
/// panes.
pub(super) fn panes_to_redraw(ws: &Workspace, what: Redraw, active: PaneId) -> Vec<PaneId> {
    let followers = |k: PaneKind| match what {
        Redraw::Editor(pane) => {
            pane == active
                && matches!(
                    k,
                    PaneKind::Context | PaneKind::Outline | PaneKind::Highlights
                )
        }
        Redraw::Context => matches!(k, PaneKind::Context | PaneKind::PinnedContext),
        Redraw::Changes => k == PaneKind::Changes,
        Redraw::Steps => matches!(
            k,
            PaneKind::Tour | PaneKind::CallStack | PaneKind::Comparison | PaneKind::FileTree
        ),
    };
    let mut panes: Vec<PaneId> = match what {
        Redraw::Editor(pane) => vec![pane],
        Redraw::Context | Redraw::Changes | Redraw::Steps => Vec::new(),
    };
    panes.extend(
        ws.panes()
            .iter()
            .filter(|(_, c)| followers(c.kind()))
            .map(|(id, _)| *id),
    );
    panes
}

/// The default arrangement: Tree on the left (260), one editor, Context above Outline on the
/// right (330, Outline about 38%), an empty bottom bar.
pub(super) fn default_workspace() -> Workspace {
    let mut ws = Workspace::with_defaults();
    let editor = ws
        .open(
            PaneContent::Editor {
                path: String::new(),
                caret: 0,
            },
            None,
        )
        .expect("the editor area accepts an editor");
    // Sizes are set through the model's own operations, which clamp them.
    let _ = ws.resize_dock(DockId::Left, LEFT_WIDTH);
    let _ = ws.resize_dock(DockId::Right, RIGHT_WIDTH);
    if let Some(Node::Split(s)) = &ws.arrangement().dock(DockId::Right).root {
        let id = s.id;
        // Context and Outline start at equal shares; move the handle so Outline has 38%.
        let _ = ws.resize_split(id, 0, 0.5 - OUTLINE_SHARE);
    }
    let _ = ws.activate(editor);
    ws.clear_undo();
    ws
}

/// A layout saved for this repository, if it loads and is usable; else the default. A file
/// that is missing, damaged, fails the model's invariants or has no editor pane gives the
/// default (a repaired file, with warnings, is used as repaired).
pub(super) fn load_workspace(state_dir: Option<&Path>, key: Option<&str>) -> Workspace {
    let (Some(dir), Some(key)) = (state_dir, key) else {
        return default_workspace();
    };
    if !dir.join(cho_layout::layout_file_name(key)).is_file() {
        return default_workspace();
    }
    let ws = cho_layout::load(dir, key).workspace;
    if ws.check_invariants().is_err() || pane_of(&ws, PaneKind::Editor).is_none() {
        return default_workspace();
    }
    ws
}

/// The viewer's pane contents, drawn by the `Chorong` view.
pub(super) struct ChorongBodies {
    pub cho: WeakEntity<Chorong>,
}

fn note(text: &str, muted: gpui::Hsla) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .text_size(px(12.5))
        .text_color(muted)
        .child(text.to_string())
        .into_any_element()
}

impl PaneBodies for ChorongBodies {
    fn body(
        &self,
        pane: PaneId,
        content: &PaneContent,
        ws: &Workspace,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(cho) = self.cho.upgrade() else {
            return div().into_any_element();
        };
        let kind = content.kind();
        cho.update(cx, |o, cx| {
            let muted = hsla(o.theme.text_muted);
            let inner = match kind {
                PaneKind::FileTree => o.render_sidebar(cx),
                PaneKind::Editor => div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .children(o.render_banner(pane, cx))
                    .children(o.render_step_note(pane, cx))
                    .child(match o.render_external_stop(pane, cx) {
                        Some(card) => card,
                        None => o
                            .render_source(
                                pane,
                                crate::minimap::Focus::of(
                                    ws.focus().map(|f| f.on_screen()).as_deref(),
                                    &pane,
                                    |p| ws.kind(*p) == Some(PaneKind::Editor),
                                ),
                                cx,
                            )
                            .into_any_element(),
                    })
                    .into_any_element(),
                PaneKind::Context => o.render_context(cx).into_any_element(),
                PaneKind::Changes => o.render_changes(cx),
                PaneKind::Tour => o.render_tour(cx),
                PaneKind::CallStack => o.render_callstack(cx),
                PaneKind::Comparison => o.render_comparison(cx),
                PaneKind::Highlights => o.render_highlights(cx),
                PaneKind::Graph => o.render_graph_pane_content(content, cx),
                PaneKind::Outline => o.render_outline(cx).into_any_element(),
                PaneKind::PinnedContext => match content {
                    PaneContent::PinnedContext(pin) => o.render_pin(pane, pin, ws, cx),
                    _ => div().into_any_element(),
                },
                PaneKind::Placeholder => match content {
                    PaneContent::Placeholder { name } => o
                        .render_tour_card(name, cx)
                        .unwrap_or_else(|| note("Not available in the viewer yet", muted)),
                    _ => note("Not available in the viewer yet", muted),
                },
                _ => note("Not available in the viewer yet", muted),
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .font_family(cx.global::<cho_fonts::Fonts>().ui.clone())
                .text_color(hsla(o.theme.text))
                .child(inner)
                .into_any_element()
        })
    }

    fn page_chip(&self, index: usize, ws: &Workspace, cx: &App) -> Option<PageChip> {
        self.cho.upgrade()?.read(cx).tour_page_chip(index, ws)
    }

    fn focus_side(
        &self,
        _ws: &Workspace,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.cho
            .upgrade()?
            .update(cx, |o, cx| o.render_tour_rail(cx))
    }

    fn qualifier(&self, pane: PaneId, content: &PaneContent, cx: &App) -> String {
        let Some(cho) = self.cho.upgrade() else {
            return String::new();
        };
        let o = cho.read(cx);
        match content.kind() {
            PaneKind::FileTree => {
                let name = o
                    .root
                    .as_deref()
                    .and_then(Path::file_name)
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match o.tree_filter_qualifier() {
                    Some(f) => format!("{name} \u{b7} {f}"),
                    None => name,
                }
            }
            PaneKind::Outline => o
                .ed()
                .doc
                .as_ref()
                .map(|d| d.outline.len().to_string())
                .unwrap_or_default(),
            PaneKind::Highlights => match o.ed().marks.groups().len() {
                0 => String::new(),
                n => n.to_string(),
            },
            PaneKind::Tour => o.tour_qualifier(),
            PaneKind::CallStack => o.callstack_qualifier(),
            PaneKind::Comparison => o.comparison_qualifier(),
            PaneKind::Graph => o.graph_pane_qualifier(content),
            PaneKind::Context if o.hover.active().is_some() => "hovering".into(),
            PaneKind::Context if o.ed().doc.is_some() => "at caret".into(),
            PaneKind::Editor => o
                .editors
                .get(&pane)
                .and_then(|e| e.doc.as_ref())
                .and_then(|d| d.language)
                .map(|l| l.name().to_string())
                .unwrap_or_default(),
            PaneKind::PinnedContext => match content {
                PaneContent::PinnedContext(pin) => o.pin_qualifier(pane, pin),
                _ => String::new(),
            },
            _ => String::new(),
        }
    }

    /// While comparing: which side of the comparison this editor shows.
    fn header_actions(
        &self,
        pane: PaneId,
        content: &PaneContent,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let cho = self.cho.upgrade()?;
        match content.kind() {
            PaneKind::Context => Some(cho.update(cx, |o, cx| o.context_pin_buttons(cx))),
            PaneKind::Changes => Some(cho.update(cx, |o, cx| o.changes_refresh_button(cx))),
            PaneKind::Highlights => Some(cho.update(cx, |o, cx| o.highlights_header_actions(cx))),
            PaneKind::Editor => Some(cho.update(cx, |o, cx| o.history_buttons(pane, cx))),
            PaneKind::FileTree => cho.update(cx, |o, cx| o.tour_files_chip(cx)),
            kind @ (PaneKind::Tour | PaneKind::CallStack | PaneKind::Comparison) => {
                cho.update(cx, |o, cx| o.step_header_actions(kind, cx))
            }
            _ => None,
        }
    }

    fn badge(&self, pane: PaneId, content: &PaneContent, cx: &App) -> Option<Badge> {
        if content.kind() != PaneKind::Editor {
            return None;
        }
        let o = self.cho.upgrade()?.read(cx);
        let cmp = o.compare.as_ref()?;
        let ed = o.editors.get(&pane)?;
        ed.doc.as_ref()?;
        Some(match ed.side {
            Side::Before => Badge {
                text: format!("Before \u{b7} {}", cho_git::short(&cmp.base)),
                tone: BadgeTone::Bad,
            },
            Side::After => Badge {
                text: "After".into(),
                tone: BadgeTone::Good,
            },
        })
    }

    /// Code panes draw anew when a window-wide way of drawing the comparison marks changes.
    fn body_key(&self, _pane: PaneId, content: &PaneContent, _ws: &Workspace) -> u64 {
        match content.kind() {
            PaneKind::Editor => {
                super::change_marks::MARKS_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
            }
            _ => 0,
        }
    }

    /// A code pane's chain icon: whether it is half of a split, and linked.
    fn header_key(&self, pane: PaneId, content: &PaneContent, cx: &App) -> u64 {
        if content.kind() != PaneKind::Editor {
            return 0;
        }
        let Some(o) = self.cho.upgrade() else {
            return 0;
        };
        let o = o.read(cx);
        let link = match o.links.iter().find(|l| l.has(pane)) {
            None => 0,
            Some(l) if l.linked => 1,
            Some(_) => 2,
        };
        // The uncommitted chip: its count, and Highlight Uncommitted.
        let runs = o
            .editors
            .get(&pane)
            .and_then(|e| e.gutter.as_ref())
            .filter(|_| o.compare.is_some())
            .map_or(0, |g| g.uncommitted_runs().len() as u64);
        link | (u64::from(o.highlight_uncommitted) << 2) | (runs << 3)
    }

    /// The Changes pane's menu: expand or collapse every file. An investigation's panes
    /// (Tour, Call stack, Comparison): Close investigation.
    fn menu_items(
        &self,
        _pane: PaneId,
        content: &PaneContent,
        _cx: &App,
    ) -> Vec<(gpui::SharedString, Box<dyn gpui::Action>)> {
        if matches!(
            content.kind(),
            PaneKind::Tour | PaneKind::CallStack | PaneKind::Comparison
        ) {
            return vec![(
                "Close investigation".into(),
                Box::new(super::investigation_view::CloseInvestigation),
            )];
        }
        if content.kind() != PaneKind::Changes {
            return Vec::new();
        }
        vec![
            (
                "Expand all".into(),
                Box::new(super::changes::ExpandAllChanges),
            ),
            (
                "Collapse all".into(),
                Box::new(super::changes::CollapseAllChanges),
            ),
        ]
    }

    fn qualifier_warns(&self, pane: PaneId, content: &PaneContent, cx: &App) -> bool {
        content.kind() == PaneKind::PinnedContext
            && self
                .cho
                .upgrade()
                .is_some_and(|o| o.read(cx).pin_is_stale(pane))
    }

    fn pin(&self, _n: usize, cx: &App) -> Option<Pin> {
        self.cho.upgrade()?.read(cx).layout_pin()
    }
}

/// Whether `pane` is on screen: on the current focus page in a focus layout; otherwise the
/// active tab of a group that is not minimised, in the code area or a dock that is shown.
/// Parked panes (the tray, a hidden tab) are not.
pub(super) fn pane_on_screen(ws: &Workspace, pane: PaneId) -> bool {
    if let Some(f) = ws.focus() {
        return f.on_screen().contains(&pane);
    }
    let Some(group) = ws.group_of(pane).and_then(|g| ws.group(g)) else {
        return false;
    };
    if group.minimized || group.active_pane() != Some(pane) {
        return false;
    }
    match ws.arrangement().locate(pane) {
        Some((cho_layout::Region::Editor, _)) => true,
        Some((cho_layout::Region::Dock(d), _)) => ws.arrangement().dock(d).visible,
        None => false,
    }
}

impl Chorong {
    /// Builds the shell for the folder `root`: the saved layout for that repository (or the
    /// default), saved back off the UI thread as it changes, and the frame overlay when the
    /// `debug.frame_overlay` setting is on. `CHORONG_FRAME_LOG=FILE` writes one line per frame.
    pub(super) fn new_shell(
        theme: &Theme,
        root: Option<&Path>,
        focus: &FocusHandle,
        features: &Features,
        cx: &mut Context<Self>,
    ) -> Entity<Shell> {
        let show_stats = features.on(Feature::FrameOverlay);
        let state_dir = features.store().config_dir().map(|d| d.join("layout"));
        // The repository key: its canonical path, which the saver only ever uses through a
        // hash (`cho_layout::layout_file_name`), so no path appears in a file name.
        let key = root
            .and_then(|r| r.canonicalize().ok())
            .map(|r| r.to_string_lossy().into_owned());
        let ws = load_workspace(state_dir.as_deref(), key.as_deref());
        let options = ShellOptions {
            frame_log: Self::frame_log_path(features),
            state_dir: state_dir.filter(|_| key.is_some()),
            state_key: key.unwrap_or_default(),
            show_stats,
            autosave: features.on(Feature::LayoutAutosave),
            metrics: Some(features.metrics().clone()),
        };
        let bodies = std::rc::Rc::new(ChorongBodies {
            cho: cx.weak_entity(),
        });
        let focus = focus.clone();
        cx.new(|cx| Shell::new(ws, theme, options, bodies, focus, cx))
    }

    /// The frame log file: `CHORONG_FRAME_LOG`, while `debug.frame_log` is on.
    pub(super) fn frame_log_path(features: &Features) -> Option<PathBuf> {
        std::env::var_os("CHORONG_FRAME_LOG")
            .map(PathBuf::from)
            .filter(|_| features.on(Feature::FrameLog))
    }

    /// What the layout's "pin context" actions pin: the declaration or identifier under the
    /// caret or pointer, `None` when there is nothing there. A pin of the checkout records a
    /// fingerprint of the file's text, so it notices uncommitted changes too; a pin of a file
    /// at another commit compares with that commit.
    pub(super) fn layout_pin(&self) -> Option<Pin> {
        let doc = self.ed().doc.as_ref()?;
        let at = self.context_focus()?;
        let focus = self.pane_focus()?;
        // Name the symbol the pin's references are for: the index's answer for this spot
        // when the Context pane has it, else the identifier, else the declaration.
        let indexed = match self.context.lookup.symbol() {
            Some(c) if !c.name.is_empty() => {
                let kind = c.kind.to_lowercase();
                Some((
                    if kind.is_empty() {
                        "symbol".into()
                    } else {
                        kind
                    },
                    c.name.clone(),
                ))
            }
            _ => None,
        };
        let (symbol_kind, symbol) = match (indexed, focus.ident, focus.decl) {
            (Some(named), ..) => named,
            (None, Some(ident), _) => ("identifier".to_string(), ident),
            (None, None, Some((kind, name, ..))) => (kind, name),
            (None, None, None) => return None,
        };
        let commit = match (&doc.at_rev, &self.root) {
            (Some(rev), _) => rev.clone(),
            (None, Some(root)) => cho_git::rev_parse(root, "HEAD")
                .map(|c| cho_git::short(&c))
                .unwrap_or_else(|| "unknown".into()),
            (None, None) => "unknown".into(),
        };
        Some(Pin {
            symbol,
            symbol_kind,
            path: layout_path(&doc.display_path),
            byte_offset: at,
            commit,
            content_hash: doc
                .at_rev
                .is_none()
                .then(|| cho_layout::content_hash(doc.buffer.text().as_bytes())),
        })
    }

    /// An Editor pane's recorded path follows the file it shows (repository-relative, `/`).
    pub(super) fn sync_editor_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let Some(doc) = self.editors.get(&pane).and_then(|e| e.doc.as_ref()) else {
            return;
        };
        let path = layout_path(&doc.display_path);
        self.shell.update(cx, |shell, cx| {
            let same = matches!(shell.workspace().content(pane),
                Some(PaneContent::Editor { path: p, .. }) if *p == path);
            if !same {
                let _ = shell.update_workspace(cx, |ws| ws.set_editor_position(pane, &path, 0));
            }
        });
    }

    /// Reconciles the editor states with the layout and follows focus: a new Editor pane gets
    /// a state (a copy of the pane it was split from, or its recorded file loaded in the
    /// background), a closed one loses its state, and the pane that most recently had focus is
    /// the active editor. Idempotent: runs on every layout change.
    pub(super) fn sync_panes(&mut self, cx: &mut Context<Self>) {
        let (layout, focused, recent) = {
            let ws = self.shell.read(cx).workspace();
            let layout: BTreeMap<PaneId, String> = ws
                .panes()
                .iter()
                .filter_map(|(id, c)| match c {
                    PaneContent::Editor { path, .. } => Some((*id, path.clone())),
                    _ => None,
                })
                .collect();
            (layout, ws.focused(), ws.recent_panes().to_vec())
        };
        let shell = self.shell.clone();
        self.sync_pins(shell.read(cx).workspace());
        let carets = self.recorded_carets(cx);
        let known: BTreeMap<PaneId, String> = self
            .editors
            .iter()
            .map(|(id, e)| {
                let shown = e
                    .doc
                    .as_ref()
                    .map(|d| layout_path(&d.display_path))
                    .or_else(|| {
                        e.loading
                            .as_deref()
                            .map(|p| layout_path(&self.display_of(p)))
                    });
                (*id, shown.unwrap_or_default())
            })
            .collect();
        let plan = editor_state::plan(&layout, &known, self.active_editor);
        for pane in plan.drop {
            self.editors.remove(&pane);
        }
        // Closing either pane of a linked split ends the pair.
        let alive: Vec<PaneId> = self.editors.keys().copied().collect();
        self.links = compare::links_among(&self.links, &alive);
        let mut to_load = Vec::new();
        for add in plan.add {
            match add.copy_from.and_then(|p| self.editors.get(&p)) {
                Some(src) => {
                    let copy = src.fork();
                    if copy.doc.is_none() && !add.path.is_empty() {
                        to_load.push((add.pane, add.path.clone()));
                    }
                    self.editors.insert(add.pane, copy);
                }
                None => {
                    self.editors.insert(add.pane, EditorState::default());
                    if !add.path.is_empty() {
                        to_load.push((add.pane, add.path));
                    }
                }
            }
        }
        let ids: BTreeSet<PaneId> = layout.keys().copied().collect();
        let before = self.active_editor;
        if let Some(active) =
            editor_state::choose_active(before, focused, self.last_focused, &recent, &ids)
        {
            self.active_editor = active;
        }
        self.last_focused = focused;
        if self.active_editor != before {
            self.dirty_all = true;
            self.hover.left();
            self.refresh_context(cx);
        }
        for (pane, rel) in to_load {
            let path = self.root.clone().unwrap_or_default().join(rel);
            // A pane restored from a saved layout reopens at its recorded caret.
            let target = Target {
                byte: carets.get(&pane).copied().filter(|&c| c > 0),
                ..Target::default()
            };
            self.open_in(pane, path, target, false, cx);
        }
        // A pane `b` reached while it was parked flips now that it is shown again.
        self.apply_wanted_sides(cx);
        // A split shows the same document in a new pane state: its blame starts now, as this
        // path does not always notify.
        self.ensure_blame(cx);
        // Panes were added or dropped, or the active one changed: bodies and the status bar
        // follow. (No notify when nothing changed, or this would loop.)
        if self.active_editor != before {
            cx.notify();
        }
        self.tour_pages_follow(cx);
    }

    /// The caret the layout records for each editor pane.
    fn recorded_carets(&self, cx: &App) -> BTreeMap<PaneId, usize> {
        self.shell
            .read(cx)
            .workspace()
            .panes()
            .iter()
            .filter_map(|(id, c)| match c {
                PaneContent::Editor { caret, .. } => Some((*id, *caret)),
                _ => None,
            })
            .collect()
    }

    /// Editor panes whose caret differs from the one the layout records, with the recorded
    /// path. Only panes showing the recorded file count.
    fn moved_carets(&self, cx: &App) -> Vec<(PaneId, String, usize)> {
        let ws = self.shell.read(cx).workspace();
        self.editors
            .iter()
            .filter_map(|(id, e)| {
                let (doc, caret) = (e.doc.as_ref()?, e.caret?);
                match ws.content(*id) {
                    Some(PaneContent::Editor { path, caret: saved })
                        if *saved != caret && *path == layout_path(&doc.display_path) =>
                    {
                        Some((*id, path.clone(), caret))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// Writes moved carets into the layout after the caret has rested, so the saved layout
    /// reopens each pane where it was without redrawing the layout on every caret move.
    pub(super) fn schedule_caret_save(&mut self, cx: &mut Context<Self>) {
        if self.caret_save.is_some() || self.moved_carets(cx).is_empty() {
            return;
        }
        self.caret_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CARET_SAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                this.caret_save = None;
                let moved = this.moved_carets(cx);
                if moved.is_empty() {
                    return;
                }
                this.shell.update(cx, |shell, cx| {
                    let _ = shell.update_workspace(cx, |ws| {
                        for (pane, path, caret) in &moved {
                            ws.set_editor_position(*pane, path, *caret)?;
                        }
                        Ok(())
                    });
                });
            });
        }));
    }

    /// Declares that the change being made affects only the active editor pane, so the next
    /// refresh redraws it (and Context and Outline) instead of every pane.
    pub(super) fn touch_pane(&mut self) {
        self.dirty_pane = Some(self.active_editor);
    }

    /// Redraws the pane bodies `what` names ([`panes_to_redraw`]), or every pane when it is
    /// `None`.
    pub(super) fn redraw(&mut self, what: Option<Redraw>, cx: &mut Context<Self>) {
        let active = self.active_editor;
        self.shell.update(cx, |shell, cx| match what {
            None => shell.refresh_all_panes(cx),
            Some(what) => {
                for p in panes_to_redraw(shell.workspace(), what, active) {
                    shell.refresh_pane(p, cx);
                }
            }
        });
    }

    /// Redraws the Context pane and the pinned context panes, and nothing else: for a change
    /// only they show (an index answer or highlighted preview lines arriving).
    pub(super) fn refresh_context_panes(&mut self, cx: &mut Context<Self>) {
        self.redraw(Some(Redraw::Context), cx);
    }

    /// Makes `pane` the active editor, in the layout too (so Layout mode starts from it).
    pub(super) fn activate_editor(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if self.active_editor == pane || !self.editors.contains_key(&pane) {
            return;
        }
        self.active_editor = pane;
        self.dirty_all = true;
        self.hover.left();
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| ws.activate(pane));
        });
        self.refresh_context(cx);
    }

    /// Files given together with the one the active code pane shows (`cho a.rs b.rs …`): a
    /// focus layout with that pane on the page (unless one is already open) and a code pane for
    /// each of these in the tray, where the tray picker, Go to Pane and Ctrl-Tab reach them.
    /// A file already open in some pane is not opened twice. Each loads in the background.
    pub(super) fn open_files_in_tray(&mut self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        if files.is_empty() {
            return;
        }
        let first = self.active_editor;
        let paths: Vec<String> = files.iter().map(|f| self.display_of(f)).collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| open_in_tray(ws, first, &paths, now));
        });
        self.sync_panes(cx);
    }

    /// The path as the layout and the UI show it: relative to the folder when inside it.
    pub(super) fn display_of(&self, path: &Path) -> String {
        self.root
            .as_deref()
            .and_then(|r| path.strip_prefix(r).ok())
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// Shows or hides the left dock (the old sidebar toggle). With no File Tree on screen
    /// (closed, in the tray, or a focus layout is open) it shows the File Tree instead.
    pub(super) fn toggle_left_dock(&mut self, cx: &mut Context<Self>) {
        let ws = self.shell.read(cx).workspace();
        if ws.in_focus() || pane_of(ws, PaneKind::FileTree).is_none() {
            return self.show_tool_pane(PaneContent::FileTree, cx);
        }
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| ws.toggle_dock(DockId::Left));
        });
    }

    /// Gets ready to open something at `place` (docs/public/user-guide.md, "Where files
    /// open"): for anything but [`OpenAt::Here`], adds an empty code pane there and makes it the
    /// active one, so the next open, jump or reference lands in it. Beside means beside the
    /// active code pane, or in a focus layout beside the focused pane. Where no pane can be
    /// added, the active pane is left as it is.
    pub(super) fn prepare_place(&mut self, place: OpenAt, cx: &mut Context<Self>) {
        if place == OpenAt::Here {
            return;
        }
        let active = self.active_editor;
        let pane = self.shell.update(cx, |shell, cx| {
            shell
                .update_workspace(cx, |ws| {
                    let near = match ws.in_focus() {
                        true => ws.focused(),
                        false => Some(active),
                    };
                    new_code_pane(ws, near, place)
                })
                .ok()
        });
        if let Some(pane) = pane {
            self.sync_panes(cx);
            self.active_editor = pane;
        }
    }

    /// Opens `path` at `place` ([`Self::prepare_place`]), at the top of the file.
    pub(super) fn open_at(
        &mut self,
        path: std::path::PathBuf,
        place: OpenAt,
        cx: &mut Context<Self>,
    ) {
        self.prepare_place(place, cx);
        self.open(path, Target::default(), true, cx);
    }

    /// Opens an empty code pane (see [`new_editor`]); Go to File then fills it.
    pub(super) fn new_editor_pane(&mut self, cx: &mut Context<Self>) {
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| new_editor(ws).map(|_| ()));
        });
    }

    /// Shows the pane of `content`'s kind wherever it is (its dock, its focus page, the
    /// tray), or opens a new one in its default place when it was closed.
    pub(super) fn show_tool_pane(&mut self, content: PaneContent, cx: &mut Context<Self>) {
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| show_pane(ws, content));
        });
    }

    /// Cmd-K R: rebuilds the current focus page from the tour step that made it. Files the
    /// step declares are opened when they exist; other panes it names that are not open are
    /// reported as missing. Pages made by hand cannot be reset.
    pub(super) fn reset_focus_page(&mut self, cx: &mut Context<Self>) {
        if self.reset_tour_page(cx) {
            return;
        }
        let root = self.root.clone().unwrap_or_default();
        let available = move |s: &cho_layout::PaneSource| match s {
            cho_layout::PaneSource::Editor { path } => root.join(path).is_file(),
            _ => false,
        };
        let result = self.shell.update(cx, |shell, cx| {
            let Some(current) = shell.workspace().focus().map(|f| f.current) else {
                return Err(cho_layout::LayoutError::NotInFocus);
            };
            shell.update_workspace(cx, |ws| ws.reset_page(current, available))
        });
        match result {
            Ok(report) if !report.missing.is_empty() => {
                let names: Vec<String> = report.missing.iter().map(|(_, s)| s.label()).collect();
                self.notify_text(
                    format!("Page reset; not available: {}", names.join(", ")),
                    cx,
                );
            }
            Ok(_) => {}
            Err(cho_layout::LayoutError::NotInFocus) => {
                self.notify_text("Reset page works in a focus layout (Cmd-K M)", cx)
            }
            Err(e) => self.notify_text(format!("Cannot reset this page: {e}"), cx),
        }
    }

    /// Folds the Outline pane to its header, or unfolds it. An Outline that is not on screen
    /// (closed, in a hidden dock or in the tray) is shown instead.
    pub(super) fn toggle_outline_pane(&mut self, cx: &mut Context<Self>) {
        let ws = self.shell.read(cx).workspace();
        let visible = |p: PaneId| {
            ws.group_of(p).is_some()
                && match ws.arrangement().locate(p) {
                    Some((cho_layout::Region::Dock(d), _)) if !ws.in_focus() => {
                        ws.arrangement().dock(d).visible
                    }
                    _ => true,
                }
        };
        match pane_of(ws, PaneKind::Outline).filter(|p| visible(*p)) {
            Some(pane) => self.shell.update(cx, |shell, cx| {
                let _ = shell.update_workspace(cx, |ws| ws.toggle_minimize(pane));
            }),
            None => self.show_tool_pane(PaneContent::Outline, cx),
        }
    }
}

/// Where a file, jump or reference opens (docs/public/user-guide.md, "Where files open").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenAt {
    /// In the active code pane, replacing what it shows (Enter, a click).
    Here,
    /// A new tab beside the active code pane's (Alt-Enter).
    Tab,
    /// A new code pane to the right (Cmd-Enter, Alt-click).
    Right,
    /// A new code pane below (Cmd-Shift-Enter, Alt-Shift-click).
    Below,
}

impl OpenAt {
    /// For a click: Alt opens to the right, Alt-Shift below, otherwise here.
    pub(crate) fn of_click(m: &gpui::Modifiers) -> OpenAt {
        match (m.alt, m.shift) {
            (true, true) => OpenAt::Below,
            (true, false) => OpenAt::Right,
            _ => OpenAt::Here,
        }
    }
}

/// Opens an empty code pane and focuses it: a new tab in the code area, or in a focus
/// layout beside the focused pane on the current page (the page's only pane when it is empty).
fn new_editor(ws: &mut Workspace) -> cho_layout::Result<PaneId> {
    let near = ws.in_focus().then(|| ws.focused()).flatten();
    new_code_pane(ws, near, OpenAt::Right)
}

/// Opens an empty code pane at `place` next to `near`'s group and focuses it: to its right,
/// below it, or as a tab in it. Outside a focus layout with nothing to stand beside it is a
/// tab in the code area; in a focus layout, the page's only pane when the page is empty.
fn new_code_pane(
    ws: &mut Workspace,
    near: Option<PaneId>,
    place: OpenAt,
) -> cho_layout::Result<PaneId> {
    use cho_layout::{Edge, Target as Lt};
    let content = PaneContent::Editor {
        path: String::new(),
        caret: 0,
    };
    let group = near.and_then(|p| ws.group_of(p));
    let target = group.and_then(|group| match place {
        OpenAt::Right => Some(Lt::Beside {
            group,
            edge: Edge::Right,
        }),
        OpenAt::Below => Some(Lt::Beside {
            group,
            edge: Edge::Bottom,
        }),
        OpenAt::Tab => Some(Lt::Tabs {
            group,
            index: usize::MAX,
        }),
        OpenAt::Here => None,
    });
    let pane = ws.open(content, target)?;
    if ws.in_focus() && target.is_none() {
        ws.place_on_empty_page(pane)?;
    }
    ws.reveal(pane)?;
    Ok(pane)
}

/// Shows the pane of `content`'s kind, opening one in its default place if there is none.
pub(super) fn show_pane(ws: &mut Workspace, content: PaneContent) -> cho_layout::Result<()> {
    let pane = match pane_of(ws, content.kind()) {
        Some(p) => p,
        None => ws.open(content, None)?,
    };
    ws.reveal(pane)
}

/// [`Chorong::open_files_in_tray`] on the layout: maximise `first` into a focus layout when none
/// is open, then a code pane in the tray for each path no pane shows yet.
pub(super) fn open_in_tray(
    ws: &mut Workspace,
    first: PaneId,
    paths: &[String],
    now: u64,
) -> cho_layout::Result<()> {
    if !ws.in_focus() {
        ws.maximize(first, now)?;
    }
    for path in paths {
        let open = ws
            .panes()
            .values()
            .any(|c| matches!(c, PaneContent::Editor { path: p, .. } if p == path));
        if !open {
            ws.open(
                PaneContent::Editor {
                    path: path.clone(),
                    caret: 0,
                },
                Some(cho_layout::Target::Tray),
            )?;
        }
    }
    Ok(())
}

/// The first pane of `kind`, anywhere in the layout.
/// A document's path as the saved layout records it (forward slashes on every platform).
fn layout_path(display_path: &str) -> String {
    display_path.replace('\\', "/")
}

pub(super) fn pane_of(ws: &Workspace, kind: PaneKind) -> Option<PaneId> {
    ws.panes()
        .iter()
        .find(|(_, c)| c.kind() == kind)
        .map(|(id, _)| *id)
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        ToggleOutline,
        ToggleFileTree,
        NewCodePane,
        ShowContextPane,
        ResetFocusPage
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c(
        "chorong::ToggleOutline",
        "Toggle Outline Panel",
        Layer::View,
        140,
    ),
    c(
        "chorong::ShowContextPane",
        "Show Context Pane",
        Layer::View,
        145,
    ),
    c(
        "chorong::ToggleFileTree",
        "Toggle File Tree",
        Layer::View,
        150,
    ),
    c("chorong::NewCodePane", "New Code Pane", Layer::View, 160),
    // The layout shell's own actions (cho-layout-ui handles them; see design/layout-keyboard.md).
    c("chorong::FocusPane", "Go to Pane\u{2026}", Layer::Go, 94),
    c(
        "chorong::OpenSwitcher",
        "Switch to Recent Pane",
        Layer::Go,
        96,
    ),
    c(
        "chorong::MovePaneTo",
        "Move Pane to\u{2026}",
        Layer::View,
        151,
    ),
    c(
        "chorong::BringFromTray",
        "Add to Page from Tray or Files\u{2026}",
        Layer::View,
        152,
    ),
    c("chorong::GrowDock", "Grow Dock", Layer::View, 153),
    c("chorong::ShrinkDock", "Shrink Dock", Layer::View, 154),
    c(
        "chorong::ResetDockSize",
        "Reset Dock Size",
        Layer::View,
        155,
    ),
    // Focus layouts: the shell handles these (`bind_actions`); they are listed here so the
    // palette, the cheat sheet and the key table show them.
    c(
        "chorong::ToggleMaximizePane",
        "Maximise or Restore Pane",
        Layer::View,
        156,
    ),
    c("chorong::NewFocusPage", "New Focus Page", Layer::View, 157),
    c(
        "chorong::NextFocusPage",
        "Next Focus Page",
        Layer::View,
        158,
    ),
    c(
        "chorong::PrevFocusPage",
        "Previous Focus Page",
        Layer::View,
        159,
    ),
    c(
        "chorong::ResetFocusPage",
        "Reset Focus Page to Its Tour Step",
        Layer::View,
        161,
    ),
    c(
        "chorong::ReopenFocusLayout",
        "Reopen Focus Layout\u{2026}",
        Layer::View,
        162,
    ),
    c(
        "chorong::UndoLayout",
        "Undo Layout Change",
        Layer::View,
        163,
    ),
    c(
        "chorong::RedoLayout",
        "Redo Layout Change",
        Layer::View,
        164,
    ),
    // The pane menu's and the dock toggles' actions, so every layout change can be typed in
    // the palette (they act on the focused pane).
    c("chorong::ClosePane", "Close Pane", Layer::View, 172),
    c(
        "chorong::MinimizePane",
        "Minimise or Restore Pane",
        Layer::View,
        173,
    ),
    c(
        "chorong::SplitPaneRight",
        "Split Pane Right",
        Layer::View,
        174,
    ),
    c(
        "chorong::SplitPaneDown",
        "Split Pane Down",
        Layer::View,
        175,
    ),
    c(
        "chorong::SplitPaneLeft",
        "Split Pane Left",
        Layer::View,
        176,
    ),
    c("chorong::SplitPaneUp", "Split Pane Up", Layer::View, 177),
    c(
        "chorong::SendPaneToTray",
        "Send Pane to Tray",
        Layer::View,
        178,
    ),
    c(
        "chorong::MovePaneToLeftDock",
        "Move Pane to Left Dock",
        Layer::View,
        179,
    ),
    c(
        "chorong::MovePaneToRightDock",
        "Move Pane to Right Dock",
        Layer::View,
        181,
    ),
    c(
        "chorong::MovePaneToBottomBar",
        "Move Pane to Bottom Bar",
        Layer::View,
        182,
    ),
    c(
        "chorong::MovePaneToEditorArea",
        "Move Pane to Code Area",
        Layer::View,
        183,
    ),
    c(
        "chorong::ToggleLeftDock",
        "Toggle Left Dock",
        Layer::View,
        184,
    ),
    c(
        "chorong::ToggleRightDock",
        "Toggle Right Dock",
        Layer::View,
        186,
    ),
    c(
        "chorong::ToggleBottomBar",
        "Toggle Bottom Bar",
        Layer::View,
        187,
    ),
    c(
        "chorong::ToggleAllDocks",
        "Toggle All Docks",
        Layer::View,
        188,
    ),
    c(
        "chorong::EnterLayoutMode",
        "Enter Layout Mode",
        Layer::View,
        189,
    ),
    c("chorong::FocusPaneLeft", "Go to Pane Left", Layer::Go, 192),
    c(
        "chorong::FocusPaneRight",
        "Go to Pane Right",
        Layer::Go,
        193,
    ),
    c("chorong::FocusPaneUp", "Go to Pane Up", Layer::Go, 194),
    c("chorong::FocusPaneDown", "Go to Pane Down", Layer::Go, 195),
    c(
        "chorong::ToggleFrameStats",
        "Toggle Frame Stats",
        Layer::View,
        197,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    // From the layout shell: Tab in Bring from tray…, or New Focus Page with an empty tray.
    let d = on!(
        d,
        cx,
        cho_layout_ui::actions::AddFileToPage,
        |this, _w, cx| this.open_add_to_page_picker()
    );
    let d = on!(d, cx, ToggleOutline, |this, _w, cx| this
        .toggle_outline_pane(cx));
    let d = on!(d, cx, ToggleFileTree, |this, _w, cx| this
        .toggle_left_dock(cx));
    let d = on!(d, cx, ShowContextPane, |this, _w, cx| this
        .show_tool_pane(PaneContent::Context, cx));
    let d = on!(d, cx, NewCodePane, |this, _w, cx| this.new_editor_pane(cx));
    on!(d, cx, ResetFocusPage, |this, _w, cx| this
        .reset_focus_page(cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_workspace_is_valid_and_shaped_as_designed() {
        let ws = default_workspace();
        ws.check_invariants().unwrap();
        let kinds: Vec<_> = ws.panes().values().map(PaneContent::kind).collect();
        for k in [
            PaneKind::FileTree,
            PaneKind::Editor,
            PaneKind::Context,
            PaneKind::Outline,
        ] {
            assert_eq!(kinds.iter().filter(|x| **x == k).count(), 1, "{k:?}");
        }
        assert_eq!(kinds.len(), 4);
        let a = ws.arrangement();
        assert_eq!(a.left.size, LEFT_WIDTH);
        assert_eq!(a.right.size, RIGHT_WIDTH);
        assert!(a.bottom.root.is_none());
        let Some(Node::Split(s)) = &a.right.root else {
            panic!("right dock is a split");
        };
        assert!((s.children[1].weight - OUTLINE_SHARE).abs() < 1e-4);
        assert!(!ws.undo_stack().can_undo());
        assert_eq!(pane_of(&ws, PaneKind::Editor), ws.focused());
    }

    #[test]
    fn panes_in_the_tray_hidden_tabs_and_hidden_docks_are_off_screen() {
        let editor = |path: &str| PaneContent::Editor {
            path: path.into(),
            caret: 0,
        };
        // Normal layout: the code pane and the docks' active panes are on screen.
        let mut ws = default_workspace();
        let first = pane_of(&ws, PaneKind::Editor).unwrap();
        let tree = pane_of(&ws, PaneKind::FileTree).unwrap();
        assert!(pane_on_screen(&ws, first) && pane_on_screen(&ws, tree));
        // A second tab in the same group hides the first.
        let tab = ws
            .open(
                editor("b.rs"),
                Some(cho_layout::Target::Tabs {
                    group: ws.group_of(first).unwrap(),
                    index: usize::MAX,
                }),
            )
            .unwrap();
        ws.activate(tab).unwrap();
        assert!(pane_on_screen(&ws, tab) && !pane_on_screen(&ws, first));
        // A hidden dock hides its panes.
        ws.toggle_dock(cho_layout::DockId::Left).unwrap();
        assert!(!pane_on_screen(&ws, tree));
        // Focus layout: only the current page's panes; the tray's are parked.
        let mut ws = default_workspace();
        let first = pane_of(&ws, PaneKind::Editor).unwrap();
        open_in_tray(&mut ws, first, &["c.rs".into()], 1).unwrap();
        let parked = ws.tray()[0];
        assert!(pane_on_screen(&ws, first) && !pane_on_screen(&ws, parked));
        ws.swap_from_tray(parked).unwrap();
        assert!(pane_on_screen(&ws, parked) && !pane_on_screen(&ws, first));
    }

    #[test]
    fn closed_tool_panes_come_back_in_both_layouts() {
        for kind in [PaneKind::FileTree, PaneKind::Context, PaneKind::Outline] {
            let content = match kind {
                PaneKind::FileTree => PaneContent::FileTree,
                PaneKind::Context => PaneContent::Context,
                _ => PaneContent::Outline,
            };
            // Normal layout: closed, then shown again in its dock.
            let mut ws = default_workspace();
            ws.close(pane_of(&ws, kind).unwrap()).unwrap();
            assert!(pane_of(&ws, kind).is_none());
            show_pane(&mut ws, content.clone()).unwrap();
            let p = pane_of(&ws, kind).expect("reopened");
            assert!(ws.group_of(p).is_some(), "{kind:?} on screen");
            assert_eq!(ws.focused(), Some(p));
            ws.check_invariants().unwrap();

            // Focus layout: maximise the editor, close the pane, show it: it joins the page.
            let mut ws = default_workspace();
            ws.maximize(pane_of(&ws, PaneKind::Editor).unwrap(), 1)
                .unwrap();
            ws.close(pane_of(&ws, kind).unwrap()).unwrap();
            show_pane(&mut ws, content).unwrap();
            let p = pane_of(&ws, kind).expect("reopened in focus");
            assert!(ws.group_of(p).is_some(), "{kind:?} on the focus page");
            ws.check_invariants().unwrap();
        }
    }

    #[test]
    fn files_given_together_open_in_focus_with_the_rest_in_the_tray() {
        let mut ws = default_workspace();
        let first = pane_of(&ws, PaneKind::Editor).unwrap();
        let paths = ["src/b.rs".to_string(), "src/c.rs".to_string()];
        open_in_tray(&mut ws, first, &paths, 1).unwrap();
        assert!(ws.in_focus());
        assert_eq!(
            ws.focused(),
            Some(first),
            "the first file stays on the page"
        );
        let tray: Vec<String> = ws
            .tray()
            .into_iter()
            .filter_map(|p| match ws.content(p) {
                Some(PaneContent::Editor { path, .. }) => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tray, paths);
        ws.check_invariants().unwrap();
        // Again, already in focus: nothing is opened twice.
        let before = ws.panes().len();
        open_in_tray(&mut ws, first, &paths, 2).unwrap();
        assert_eq!(ws.panes().len(), before);
    }

    #[test]
    fn a_new_code_pane_opens_empty_in_both_layouts() {
        let mut ws = default_workspace();
        let first = pane_of(&ws, PaneKind::Editor).unwrap();
        let p = new_editor(&mut ws).unwrap();
        assert_ne!(p, first);
        assert_eq!(ws.focused(), Some(p));
        assert!(ws.group_of(p).is_some());
        assert!(matches!(ws.content(p), Some(PaneContent::Editor { path, .. }) if path.is_empty()));
        ws.check_invariants().unwrap();

        // In focus: beside the focused pane, on the current page.
        let mut ws = default_workspace();
        ws.maximize(pane_of(&ws, PaneKind::Editor).unwrap(), 1)
            .unwrap();
        let p = new_editor(&mut ws).unwrap();
        let f = ws.focus().unwrap();
        assert_eq!(f.page_of(p), Some(f.current));
        ws.check_invariants().unwrap();
    }

    #[test]
    fn placements_split_right_and_below_or_add_a_tab_in_both_layouts() {
        use cho_layout::{Axis, Node, Region};
        // The normal layout: beside the active code pane's group, in the code area.
        let mut ws = default_workspace();
        let first = pane_of(&ws, PaneKind::Editor).unwrap();
        let right = new_code_pane(&mut ws, Some(first), OpenAt::Right).unwrap();
        let below = new_code_pane(&mut ws, Some(first), OpenAt::Below).unwrap();
        let tab = new_code_pane(&mut ws, Some(first), OpenAt::Tab).unwrap();
        ws.check_invariants().unwrap();
        assert_eq!(ws.group_of(tab), ws.group_of(first));
        assert_ne!(ws.group_of(right), ws.group_of(first));
        assert_ne!(ws.group_of(below), ws.group_of(first));
        let Some(Node::Split(s)) = ws.arrangement().root(Region::Editor).clone() else {
            panic!("the code area is split");
        };
        assert_eq!(s.axis, Axis::Row, "right of the first group");
        for p in [right, below, tab] {
            assert_eq!(ws.arrangement().locate(p).unwrap().0, Region::Editor);
        }

        // In focus: beside the focused pane, on the current page.
        let mut ws = default_workspace();
        let first = pane_of(&ws, PaneKind::Editor).unwrap();
        ws.maximize(first, 1).unwrap();
        let below = new_code_pane(&mut ws, Some(first), OpenAt::Below).unwrap();
        let f = ws.focus().unwrap();
        assert_eq!(f.page_of(below), Some(f.current));
        let Some(Node::Split(s)) = f.root().cloned() else {
            panic!("the page is split");
        };
        assert_eq!(s.axis, Axis::Column, "below the first pane");
        ws.check_invariants().unwrap();
    }

    #[test]
    fn a_missing_or_damaged_layout_gives_the_default() {
        let dir = std::env::temp_dir().join(format!("cho-layout-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = "/some/repo";
        assert_eq!(
            load_workspace(Some(&dir), Some(key)).to_json(),
            default_workspace().to_json()
        );
        std::fs::write(dir.join(cho_layout::layout_file_name(key)), "{ nope").unwrap();
        assert_eq!(
            load_workspace(Some(&dir), Some(key)).to_json(),
            default_workspace().to_json()
        );
        // A saved layout round-trips.
        let mut ws = default_workspace();
        ws.toggle_dock(DockId::Left).unwrap();
        cho_layout::save(&dir, key, &ws).unwrap();
        assert!(
            !load_workspace(Some(&dir), Some(key))
                .arrangement()
                .left
                .visible
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An editor change redraws that pane, with Context and Outline only when it is the
    /// active editor; a context answer redraws Context and the pins, never an editor or Outline.
    #[test]
    fn a_change_redraws_only_the_panes_that_show_it() {
        let mut ws = default_workspace();
        let editor = pane_of(&ws, PaneKind::Editor).unwrap();
        let pin = ws
            .pin_context(a_pin(), cho_layout::PinPlace::BottomBar)
            .unwrap();
        let of = |k: PaneKind, ws: &Workspace| {
            ws.panes()
                .iter()
                .find(|(_, c)| c.kind() == k)
                .map(|(id, _)| *id)
                .unwrap()
        };
        let (context, outline) = (of(PaneKind::Context, &ws), of(PaneKind::Outline, &ws));
        let sorted = |mut v: Vec<PaneId>| {
            v.sort();
            v
        };
        assert_eq!(
            sorted(panes_to_redraw(&ws, Redraw::Editor(editor), editor)),
            sorted(vec![editor, context, outline])
        );
        let other = PaneId(999);
        assert_eq!(
            panes_to_redraw(&ws, Redraw::Editor(other), editor),
            vec![other]
        );
        assert_eq!(
            sorted(panes_to_redraw(&ws, Redraw::Context, editor)),
            sorted(vec![context, pin])
        );
    }

    fn a_pin() -> Pin {
        Pin {
            symbol: "SourceBuffer::utf16_to_byte".into(),
            symbol_kind: "method".into(),
            path: "src/buffer.rs".into(),
            byte_offset: 120,
            commit: "04e63f8".into(),
            content_hash: Some(cho_layout::content_hash(b"fn a() {}\n")),
        }
    }

    /// Pin here goes under the live Context pane; pin to bottom bar is a bottom-bar tab; pins
    /// move between the right dock and the bottom bar; closing one unpins it, and undo brings
    /// back the same pin.
    #[test]
    fn pins_open_where_designed_move_and_unpin() {
        use cho_layout::{PinPlace, Region, Target};
        let mut ws = default_workspace();
        let context = ws
            .panes()
            .iter()
            .find(|(_, c)| c.kind() == PaneKind::Context)
            .map(|(id, _)| *id)
            .unwrap();
        let here = ws.pin_context(a_pin(), PinPlace::Here).unwrap();
        let bottom = ws.pin_context(a_pin(), PinPlace::BottomBar).unwrap();
        ws.check_invariants().unwrap();
        let a = ws.arrangement();
        assert_eq!(a.locate(here).unwrap().0, Region::Dock(DockId::Right));
        let right = a.dock(DockId::Right).root.as_ref().unwrap().panes();
        let at = |p| right.iter().position(|x| *x == p).unwrap();
        assert_eq!(at(here), at(context) + 1, "directly under Context");
        assert_eq!(a.locate(bottom).unwrap().0, Region::Dock(DockId::Bottom));
        // Pins move between the right dock and the bottom bar.
        ws.move_pane(here, Target::Dock(DockId::Bottom)).unwrap();
        ws.move_pane(bottom, Target::Dock(DockId::Right)).unwrap();
        ws.check_invariants().unwrap();
        assert_eq!(
            ws.arrangement().locate(here).unwrap().0,
            Region::Dock(DockId::Bottom)
        );
        assert_eq!(
            ws.arrangement().locate(bottom).unwrap().0,
            Region::Dock(DockId::Right)
        );
        // Closing unpins; undo brings back the same pin with its commit and fingerprint.
        ws.close(here).unwrap();
        assert!(ws.content(here).is_none());
        ws.undo().unwrap();
        assert_eq!(ws.content(here), Some(&PaneContent::PinnedContext(a_pin())));
    }

    /// Pins (with their fingerprint) and focus history come back after a restart.
    #[test]
    fn pins_and_focus_history_survive_a_restart() {
        use cho_layout::PinPlace;
        let dir = std::env::temp_dir().join(format!("cho-pins-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = "/some/pinned/repo";
        let mut ws = default_workspace();
        let pin = ws.pin_context(a_pin(), PinPlace::BottomBar).unwrap();
        let editor = pane_of(&ws, PaneKind::Editor).unwrap();
        ws.maximize(editor, 100).unwrap();
        ws.new_page(None).unwrap();
        ws.move_pane_to_page(pin, 1).unwrap();
        ws.restore(160).unwrap();
        cho_layout::save(&dir, key, &ws).unwrap();
        let back = load_workspace(Some(&dir), Some(key));
        assert_eq!(
            back.content(pin),
            Some(&PaneContent::PinnedContext(a_pin()))
        );
        assert_eq!(back.history().len(), 1);
        assert_eq!(back.history()[0].pages.len(), 2);
        assert_eq!(back.history(), ws.history());
        std::fs::remove_dir_all(&dir).ok();
    }
}

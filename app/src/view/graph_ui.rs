//! Show graph in the viewer: the popover that chooses what the graph starts from and how
//! far it reaches, the background build, and the page opened in the browser. The engine is
//! `crate::graph` (and through it `cho-graph`); see `docs/public/graph.md`, "In the viewer".

use super::*;
use crate::graph::{self, GraphOptions, Scope};
use cho_graph::{EdgeKind, RefGraph};

/// The Show graph popover.
pub(crate) struct GraphPopover {
    pub scopes: Vec<Scope>,
    pub selected: usize,
}

impl Chorong {
    /// The declaration at `(file, byte)` as a graph scope: its exact SCIP symbol from the
    /// index, never a name match.
    pub(super) fn symbol_scope(&self, path: &std::path::Path, byte: usize) -> Option<Scope> {
        let sem = self.doc_index()?;
        let rel = path
            .strip_prefix(self.root.as_deref()?)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        let at = sem.symbol_at(&rel, byte)?;
        let label = sem
            .info(&at.id)
            .map(|i| i.display_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| at.id.symbol.clone());
        Some(Scope::Symbol {
            symbol: at.id.symbol,
            label,
        })
    }

    /// Show graph (Cmd-K G, the palette, the context pane): opens the popover with what the
    /// graph can start from here. `preferred` (a file from the tree, say) is selected first.
    pub(super) fn show_graph(&mut self, preferred: Option<Scope>, cx: &mut Context<Self>) {
        if !self.features.on(Feature::Graph) {
            self.notify_text("Show graph is off (the graph.enabled setting)", cx);
            return;
        }
        let Some(root) = self.root.clone() else {
            self.notify_text("Open a folder to show a graph", cx);
            return;
        };
        if self.ed().doc.as_ref().is_some_and(|d| d.at_rev.is_some()) {
            self.notify_text(
                "Show graph describes the checkout; go back to the checkout first",
                cx,
            );
            return;
        }
        if !matches!(self.index, IndexState::Ready(..)) {
            self.notify_text(
                "Show graph needs an index: use Build Index (status bar or command palette)",
                cx,
            );
            return;
        }
        let mut scopes = Vec::new();
        if let Some(s) = self
            .focus_key()
            .and_then(|(path, byte)| self.symbol_scope(&path, byte))
        {
            scopes.push(s);
        }
        if let Some(doc) = &self.ed().doc {
            scopes.push(Scope::File {
                path: doc.display_path.clone(),
            });
            scopes.push(Scope::Package {
                name: doc.display_path.clone(),
            });
        }
        if let Some((base, label)) = graph::branch_base(&root) {
            scopes.push(Scope::Branch { base, label });
        }
        let mut selected = 0;
        if let Some(p) = preferred {
            match scopes.iter().position(|s| *s == p) {
                Some(i) => selected = i,
                None => {
                    scopes.insert(0, p);
                }
            }
        }
        if scopes.is_empty() {
            self.notify_text("Open a file, or put the cursor on a symbol, first", cx);
            return;
        }
        self.picker = None;
        self.overlay = Some(Overlay::Graph(GraphPopover { scopes, selected }));
        cx.notify();
    }

    fn popover(&mut self) -> Option<&mut GraphPopover> {
        match &mut self.overlay {
            Some(Overlay::Graph(p)) => Some(p),
            _ => None,
        }
    }

    pub(super) fn graph_step(&mut self, delta: isize) {
        if let Some(p) = self.popover() {
            let last = p.scopes.len().saturating_sub(1);
            p.selected = p.selected.saturating_add_signed(delta).min(last);
        }
    }

    pub(super) fn graph_options(&mut self, change: impl FnOnce(&mut GraphOptions)) {
        change(&mut self.graph_opts);
        self.graph_opts.depth = self.graph_opts.depth.clamp(1, graph::MAX_DEPTH);
        if let Err(e) = self.graph_opts.save() {
            eprintln!("chorong-app: {e}");
        }
    }

    /// Enter in the popover: builds the selected scope.
    pub(super) fn graph_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Graph(p)) = self.overlay.take() else {
            return;
        };
        if let Some(scope) = p.scopes.into_iter().nth(p.selected) {
            self.build_graph(scope, cx);
        }
    }

    /// Builds the page in the background (the viewer never waits on it), writes it to the
    /// cache directory, opens it in the browser and shows where it is.
    pub(super) fn build_graph(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if !self.features.on(Feature::Graph) {
            self.notify_text("Show graph is off (the graph.enabled setting)", cx);
            return;
        }
        let (IndexState::Ready(sem, _), Some(root)) = (&self.index, self.root.clone()) else {
            return;
        };
        if self.graph_building {
            self.notify_text("A graph is already being built", cx);
            return;
        }
        let sem = sem.clone();
        let cached = self
            .ref_graph
            .as_ref()
            .filter(|(s, _)| Arc::ptr_eq(s, &sem))
            .map(|(_, g)| g.clone());
        let opts = self.graph_opts.clone();
        let cost = self.features.probe(Feature::Graph);
        let dark = self.theme.appearance == cho_theme::Appearance::Dark;
        self.graph_building = true;
        let pane = self.active_editor;
        self.notify_text(format!("Building the graph: {}\u{2026}", scope.label()), cx);
        cx.spawn(async move |this, cx| {
            let sem2 = sem.clone();
            let result = cx
                .background_spawn(async move {
                    let _t = cost.background();
                    let rg = cached.unwrap_or_else(|| Arc::new(RefGraph::build(&sem2)));
                    let scope = match scope {
                        // The package of a file is only known once the graph is built.
                        Scope::Package { name: file } => {
                            let pkg = rg
                                .files
                                .iter()
                                .find(|f| f.path == file)
                                .map(|f| rg.package_name(f.package).to_string())
                                .ok_or_else(|| format!("{file} is not in the index"));
                            match pkg {
                                Ok(name) => Scope::Package { name },
                                Err(e) => return (rg, Err(e)),
                            }
                        }
                        other => other,
                    };
                    let built = graph::build(&rg, &root, &scope, &opts, dark);
                    (rg, built)
                })
                .await;
            this.update(cx, |this, cx| {
                this.graph_building = false;
                let (rg, built) = result;
                this.ref_graph = Some((sem, rg));
                match built {
                    Ok(b) => {
                        let url = format!("file://{}", b.path.display());
                        cx.open_url(&url);
                        let shown = graph::tilde(&b.path);
                        this.notify_text(format!("Graph opened in the browser: {}", b.summary), cx);
                        if let Some(ed) = this.editors.get_mut(&pane) {
                            ed.banner = Some(Banner {
                                text: format!("{} \u{b7} written to {shown}", b.summary),
                                detail: None,
                                actions: vec![
                                    ("Open again".into(), BannerAction::OpenUrl(url)),
                                    (
                                        "Copy path".into(),
                                        BannerAction::Copy(b.path.display().to_string()),
                                    ),
                                    ("Dismiss".into(), BannerAction::Dismiss),
                                ],
                                quiet: true,
                            });
                        }
                    }
                    Err(e) => this.notify_text(format!("Show graph: {e}"), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "Rebuild graph" on a link from a graph page: a fresh neighbourhood of the linked
    /// declaration at the current commit, with the remembered options.
    pub(super) fn rebuild_graph_from_link(&mut self, cx: &mut Context<Self>) {
        let Some(symbol) = self.ed().link.as_ref().and_then(|l| l.symbol.clone()) else {
            self.show_graph(None, cx);
            return;
        };
        let found = self.ed().doc.as_ref().and_then(|doc| {
            let lang = doc.language?;
            match find_symbol(lang, doc.buffer.text(), &symbol) {
                SymbolMatch::Unique(s) => Some((doc.path.clone(), s.name_range.start)),
                _ => None,
            }
        });
        match found.and_then(|(path, byte)| self.symbol_scope(&path, byte)) {
            Some(scope) => self.build_graph(scope, cx),
            None => self.notify_text(
                format!("`{symbol}` is not a single declaration in the index at this commit"),
                cx,
            ),
        }
    }

    pub(super) fn render_graph_popover(
        &self,
        p: &GraphPopover,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let (muted, sel) = (hsla(t.text_muted), hsla(t.element_selected));
        let o = &self.graph_opts;
        let key = |action: &str| self.overlay_keys.get(action).cloned().unwrap_or_default();
        let cap = |k: String| (!k.is_empty()).then(|| self.key_cap(k));
        let hint = |k: String| (!k.is_empty()).then(|| self.button_key(k));
        let chip = |id: SharedString, label: String, on: bool| {
            div()
                .id(id)
                .px_2()
                .rounded_sm()
                .border_1()
                .border_color(hsla(t.border))
                .cursor_pointer()
                .when(on, |d| {
                    d.bg(sel)
                        .text_color(hsla(t.text))
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!on, |d| d.text_color(muted))
                .hover(|s| s.bg(sel))
                .child(label)
        };
        let heading = |label: &str, k: String| {
            div()
                .flex()
                .gap_2()
                .text_size(px(11.5))
                .text_color(muted)
                .child(label.to_string())
                .children(cap(k))
        };
        let mut scopes = div().flex().flex_col();
        for (i, s) in p.scopes.iter().enumerate() {
            let label = match s {
                Scope::Package { .. } => "Package of this file".to_string(),
                other => other.label(),
            };
            scopes = scopes.child(
                div()
                    .id(("graph-scope", i))
                    .px_2()
                    .py(px(3.))
                    .rounded_sm()
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .when(i == p.selected, |d| d.bg(sel))
                    .hover(|s| s.bg(sel))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.popover() {
                            p.selected = i;
                        }
                        cx.notify();
                    })),
            );
        }
        let dir = o.direction;
        let mut directions = div().flex().gap_2();
        for (i, (label, value)) in [
            ("Both", cho_graph::Direction::Both),
            ("Who uses it", cho_graph::Direction::In),
            ("What it uses", cho_graph::Direction::Out),
        ]
        .into_iter()
        .enumerate()
        {
            directions = directions.child(
                chip(format!("graph-dir-{i}").into(), label.into(), dir == value).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.graph_options(|o| o.direction = value);
                        cx.notify();
                    }),
                ),
            );
        }
        let depth = div()
            .flex()
            .gap_2()
            .items_center()
            .child(
                chip("graph-shallower".into(), "\u{2212}".into(), false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.graph_options(|o| o.depth = o.depth.saturating_sub(1));
                        cx.notify();
                    },
                )),
            )
            .child(format!(
                "{} hop{}",
                o.depth,
                if o.depth == 1 { "" } else { "s" }
            ))
            .child(
                chip("graph-deeper".into(), "+".into(), false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.graph_options(|o| o.depth += 1);
                        cx.notify();
                    },
                )),
            );
        let mut kinds = div().flex().flex_wrap().gap_2();
        for k in EdgeKind::ALL {
            kinds = kinds.child(
                chip(
                    format!("graph-kind-{}", k.as_str()).into(),
                    k.as_str().into(),
                    o.kind_on(k),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.graph_options(|o| o.toggle_kind(k));
                    cx.notify();
                })),
            );
        }
        let share = chip(
            "graph-share".into(),
            if o.share {
                "For sharing: no branch, dirty state or worktree name".into()
            } else {
                "For this machine: with branch and dirty state".into()
            },
            o.share,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.graph_options(|o| o.share = !o.share);
            cx.notify();
        }));
        let build = div()
            .id("graph-build")
            .px_3()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(hsla(t.border))
            .bg(sel)
            .text_color(hsla(t.text))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .child("Show graph")
            .children(hint(key("chorong::GraphBuild")))
            .on_click(cx.listener(|this, _, _, cx| {
                this.graph_confirm(cx);
                cx.notify();
            }));
        // A declaration can also be drawn in a graph pane (graph_pane.rs).
        let in_pane = matches!(p.scopes.get(p.selected), Some(Scope::Symbol { .. })).then(|| {
            self.plain_button("graph-in-pane", "In a pane", false)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.graph_confirm_in_pane(cx);
                    cx.notify();
                }))
        });
        let close = self
            .plain_button("graph-close", "Close", false)
            .flex()
            .items_center()
            .gap_2()
            .children(hint(key("chorong::GraphClose")))
            .on_click(cx.listener(|this, _, _, cx| {
                this.overlay = None;
                cx.notify();
            }));
        let card = self
            .card(520.)
            .p_4()
            .gap_3()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Show graph"))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(muted)
                            .child("built from the index, opened in your browser"),
                    ),
            )
            .child(heading("Start from", String::new()))
            .child(scopes)
            .child(heading("Direction", key("chorong::GraphDirection")))
            .child(directions)
            .child(heading("Depth", key("chorong::GraphDeeper")))
            .child(depth)
            .child(heading("Edge kinds (all when none is off)", String::new()))
            .child(kinds)
            .child(heading("Links and header", key("chorong::GraphShare")))
            .child(share)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(build)
                    .children(in_pane)
                    .child(div().flex_1())
                    .child(close),
            );
        self.layer(card, cx)
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        ShowGraph,
        GraphPrev,
        GraphNext,
        GraphBuild,
        GraphClose,
        GraphDirection,
        GraphDeeper,
        GraphShallower,
        GraphShare,
    ]
);

pub(super) const COMMANDS: &[Command] = &[c("chorong::ShowGraph", "Show Graph", Layer::View, 170)];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, ShowGraph, |this, _w, cx| this.show_graph(None, cx));
    let d = on!(d, cx, GraphPrev, |this, _w, cx| this.graph_step(-1));
    let d = on!(d, cx, GraphNext, |this, _w, cx| this.graph_step(1));
    let d = on!(d, cx, GraphBuild, |this, _w, cx| this.graph_confirm(cx));
    let d = on!(d, cx, GraphClose, |this, _w, cx| this.overlay = None);
    let d = on!(d, cx, GraphDirection, |this, _w, cx| this
        .graph_options(|o| o.cycle_direction()));
    let d = on!(d, cx, GraphDeeper, |this, _w, cx| this
        .graph_options(|o| o.depth += 1));
    let d = on!(d, cx, GraphShallower, |this, _w, cx| this
        .graph_options(|o| o.depth = o.depth.saturating_sub(1)));
    on!(d, cx, GraphShare, |this, _w, cx| this
        .graph_options(|o| o.share = !o.share))
}

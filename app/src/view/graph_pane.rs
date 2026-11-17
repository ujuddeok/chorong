//! The graph pane (`PaneKind::Graph`): an interim, static diagram drawn natively in a pane, for
//! tour pages and investigation diagrams until the graph work replaces it
//! (`design/investigations-viewer.md`, "Diagrams"; `docs/public/graph.md`, "Graph pane").
//!
//! `cho-graph` lays the diagram out deterministically ([`cho_graph::layout`]); this module
//! draws that layout: groups and nodes are elements, edges are painted paths. Verified edges
//! are solid, claimed dashed, broken dotted in the error colour; nodes not in the code have a
//! dashed outline. Fit and 1:1 only, no pan or zoom gestures; a click on a node opens its
//! declaration; Open in browser writes today's HTML page (or, for a diagram with no index
//! source, the SVG) and opens it.
//!
//! The drawing is static: nothing animates, and the layout runs once per spec, on the
//! background executor (`graph.enabled`; timed like Show graph). Painting replays the stored
//! routes; measured cost: under 0.1 ms per paint for a 60-node diagram.
//!
//! API for other views (the tour page): build a [`GraphSpec`], call
//! [`Chorong::graph_pane_set`] with a key of your own, and place [`Chorong::render_graph_pane`] with
//! the same key. [`Chorong::graph_pane_drop`] forgets it.

use super::*;
use crate::graph::{self, GraphOptions, Scope};
use cho_graph::{
    Diagram, Direction, GroupBy, Layout, LineKind, NodeStyle, Orientation, Provenance, RefGraph,
    ViewSpec,
};
use gpui::{AnyElement, Hsla, PathBuilder, canvas};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// The smallest scale Fit uses; below it the diagram scrolls instead (text stays readable).
const MIN_FIT: f32 = 0.85;
const TOOLBAR_H: f32 = 26.;

/// A place in the repository a node opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeLoc {
    /// Repository-relative path.
    pub path: String,
    /// One-based line, for display and when there is no range.
    pub line: usize,
    /// The anchored range, as UTF-8 byte offsets into the file; the caret lands at its start.
    pub range: Option<std::ops::Range<usize>>,
}

/// What a graph pane shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GraphSource {
    /// The neighbourhood of a declaration (its exact SCIP symbol), from the index.
    Neighbourhood {
        symbol: String,
        label: String,
        direction: Direction,
        depth: usize,
    },
    /// A diagram given whole (an investigation's own nodes and edges), with where each node
    /// opens.
    #[allow(
        dead_code,
        reason = "kept for diagram views in tours; only tests build it (owner: leave for now)"
    )]
    Diagram {
        diagram: Diagram,
        at: BTreeMap<String, CodeLoc>,
    },
}

/// A graph pane's content: the input of [`Chorong::graph_pane_set`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GraphSpec {
    pub source: GraphSource,
    /// `None`: whichever of left to right and top down fits the pane better.
    pub orientation: Option<Orientation>,
    pub group_by: GroupBy,
}

impl GraphSpec {
    /// The neighbourhood of `symbol`, oriented to fit the pane, without group boxes.
    pub(crate) fn neighbourhood(
        symbol: String,
        label: String,
        direction: Direction,
        depth: usize,
    ) -> Self {
        Self {
            source: GraphSource::Neighbourhood {
                symbol,
                label,
                direction,
                depth: depth.clamp(1, graph::MAX_DEPTH),
            },
            orientation: None,
            group_by: GroupBy::None,
        }
    }

    /// A ready diagram (its own orientation and groups).
    #[allow(
        dead_code,
        reason = "kept for diagram views in tours; only tests call it (owner: leave for now)"
    )]
    pub(crate) fn diagram(diagram: Diagram, at: BTreeMap<String, CodeLoc>) -> Self {
        Self {
            orientation: Some(diagram.orientation),
            source: GraphSource::Diagram { diagram, at },
            group_by: GroupBy::None,
        }
    }

    /// The pane header's qualifier: `handle · out · depth 1`.
    pub(crate) fn qualifier(&self) -> String {
        match &self.source {
            GraphSource::Neighbourhood {
                label,
                direction,
                depth,
                ..
            } => format!("{label} \u{b7} {} \u{b7} depth {depth}", direction.as_str()),
            GraphSource::Diagram { diagram, .. } => format!("{} nodes", diagram.nodes.len()),
        }
    }

    /// A neighbourhood as the `query` of `PaneContent::Graph`, so a saved layout reopens it.
    pub(crate) fn to_query(&self) -> Option<String> {
        match &self.source {
            GraphSource::Neighbourhood {
                symbol,
                label,
                direction,
                depth,
            } => serde_json::to_string(&Query {
                symbol: symbol.clone(),
                label: label.clone(),
                direction: direction.as_str().into(),
                depth: *depth,
                top_down: self.orientation == Some(Orientation::TopDown),
            })
            .ok(),
            GraphSource::Diagram { .. } => None,
        }
    }

    pub(crate) fn from_query(query: &str) -> Option<Self> {
        let q: Query = serde_json::from_str(query).ok()?;
        let direction = match q.direction.as_str() {
            "in" => Direction::In,
            "out" => Direction::Out,
            _ => Direction::Both,
        };
        let mut s = Self::neighbourhood(q.symbol, q.label, direction, q.depth);
        if q.top_down {
            s.orientation = Some(Orientation::TopDown);
        }
        Some(s)
    }
}

#[derive(Serialize, Deserialize)]
struct Query {
    symbol: String,
    label: String,
    direction: String,
    depth: usize,
    #[serde(default)]
    top_down: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zoom {
    Fit,
    Actual,
}

enum Built {
    Building,
    /// One drawing, or one per orientation when the pane chooses ([`pick`]).
    Ready {
        views: Vec<(Diagram, Layout)>,
        at: BTreeMap<String, CodeLoc>,
    },
    Failed(String),
    /// A neighbourhood with no index loaded: says so, with the Build index button. Built
    /// again once the index is ready.
    NeedsIndex,
}

struct PaneState {
    spec: GraphSpec,
    built: Built,
    zoom: Zoom,
    /// The size of the drawing area at the last paint, for Fit.
    viewport: Rc<Cell<(f32, f32)>>,
    /// The drawing's scroll position, and the zoom and viewport it was last centred for.
    scroll: gpui::ScrollHandle,
    centred: Rc<Cell<Option<(Zoom, i32, i32)>>>,
    generation: Generation,
}

impl PaneState {
    /// The diagram and layout on screen, if built.
    fn shown(&self) -> Option<(&Diagram, &Layout)> {
        match &self.built {
            Built::Ready { views, .. } if !views.is_empty() => {
                let (d, l) = &views[pick(views, self.viewport.get())];
                Some((d, l))
            }
            _ => None,
        }
    }
}

/// Every graph pane's state, by its key.
#[derive(Default)]
pub(crate) struct GraphPanes {
    panes: BTreeMap<String, PaneState>,
    generation: Generation,
}

/// The diagram laid out in `orientation`, or in both when it is `None`.
fn lay_out(d: &Diagram, orientation: Option<Orientation>) -> Vec<(Diagram, Layout)> {
    let os = match orientation {
        Some(o) => vec![o],
        None => vec![Orientation::LeftRight, Orientation::TopDown],
    };
    os.into_iter()
        .map(|o| {
            let mut d = d.clone();
            d.orientation = o;
            let l = cho_graph::layout(&d);
            (d, l)
        })
        .collect()
}

/// The scale Fit would use for a `w` x `h` drawing in the viewport, before the floor.
fn fit_scale(w: f32, h: f32, (vw, vh): (f32, f32)) -> f32 {
    (vw / w).min(vh / h).min(1.0)
}

/// Which view to show in a viewport of `size`: the one Fit can draw largest (the first on a
/// tie, and before the pane has been measured).
fn pick(views: &[(Diagram, Layout)], size: (f32, f32)) -> usize {
    if size.0 <= 0. || size.1 <= 0. {
        return 0;
    }
    let mut best = 0;
    for (i, (_, l)) in views.iter().enumerate() {
        let (b, s) = (&views[best].1, l);
        if fit_scale(s.width as f32, s.height as f32, size)
            > fit_scale(b.width as f32, b.height as f32, size) + 0.01
        {
            best = i;
        }
    }
    best
}

/// What a node's layout and its diagram entry make in the pane.
fn node_colours(
    style: NodeStyle,
    kind: Option<&str>,
    c: &cho_layout_ui::colors::Colors,
    focus_fill: Hsla,
) -> (Hsla, Hsla, Hsla, bool) {
    // (fill, border, text, dashed)
    let name = match kind {
        Some("function" | "method" | "constructor" | "macro") => c.kind_function,
        Some("struct" | "class" | "enum" | "interface" | "trait" | "type" | "type-alias") => {
            c.kind_type
        }
        _ => c.text,
    };
    match style {
        NodeStyle::Focus => (focus_fill, c.text_accent, c.text, false),
        NodeStyle::Normal => (c.editor, c.border, name, false),
        NodeStyle::External | NodeStyle::More => (c.editor, c.border, c.text_muted, true),
    }
}

fn line_colour(l: LineKind, c: &cho_layout_ui::colors::Colors) -> Hsla {
    match l {
        LineKind::Verified => c.icon,
        LineKind::Claimed => c.warning.unwrap_or(c.text_muted),
        LineKind::Broken => c.error.unwrap_or(c.text),
    }
}

/// The diagram for a neighbourhood, from the reference graph. Runs on the background executor.
fn neighbourhood_diagram(
    rg: &RefGraph,
    spec: &GraphSpec,
) -> Result<(Diagram, BTreeMap<String, CodeLoc>), String> {
    let GraphSource::Neighbourhood {
        symbol,
        label,
        direction,
        depth,
    } = &spec.source
    else {
        return Err("not a neighbourhood".into());
    };
    let ix = rg
        .decls
        .iter()
        .position(|d| d.symbol.symbol == *symbol)
        .ok_or_else(|| format!("{label} is not a declaration in the index"))?;
    let mut view = ViewSpec::new(Provenance::default());
    view.direction = *direction;
    view.depth = *depth;
    view.externals = true;
    let g = cho_graph::neighbourhood(rg, &[ix as u32], &view);
    let mut d =
        cho_graph::diagram_from_graph(&g, spec.group_by, spec.orientation.unwrap_or_default());
    let mut at = BTreeMap::new();
    for n in &mut d.nodes {
        // Links in the pane go through `at`; the provenance here is not the user's.
        n.link = None;
        if let Some(gn) = g.node(&n.id)
            && let (Some(path), Some(line)) = (&gn.path, gn.line)
        {
            at.insert(
                n.id.clone(),
                CodeLoc {
                    path: path.clone(),
                    line,
                    range: None,
                },
            );
        }
    }
    Ok((d, at))
}

impl Chorong {
    /// Shows `spec` under `key`: builds it in the background unless that key already holds the
    /// same spec. A neighbourhood needs the index; a diagram lays out at once.
    pub(crate) fn graph_pane_set(&mut self, key: &str, spec: GraphSpec, cx: &mut Context<Self>) {
        if self.graph_panes.panes.get(key).is_some_and(|p| {
            p.spec == spec && !matches!(p.built, Built::Failed(_) | Built::NeedsIndex)
        }) {
            return;
        }
        let generation = self.graph_panes.generation.next();
        let zoom = self
            .graph_panes
            .panes
            .get(key)
            .map(|p| p.zoom)
            .unwrap_or(Zoom::Fit);
        let mut state = PaneState {
            spec: spec.clone(),
            built: Built::Building,
            zoom,
            viewport: Rc::new(Cell::new((0., 0.))),
            scroll: gpui::ScrollHandle::new(),
            centred: Rc::new(Cell::new(None)),
            generation,
        };
        match &spec.source {
            GraphSource::Diagram { diagram, at } => {
                state.built = Built::Ready {
                    views: lay_out(diagram, spec.orientation),
                    at: at.clone(),
                };
                self.graph_panes.panes.insert(key.to_string(), state);
            }
            GraphSource::Neighbourhood { .. } => {
                if !self.features.on(Feature::Graph) {
                    state.built =
                        Built::Failed("Graphs are off (the graph.enabled setting)".into());
                    self.graph_panes.panes.insert(key.to_string(), state);
                    return;
                }
                let IndexState::Ready(sem, _) = &self.index else {
                    state.built = Built::NeedsIndex;
                    self.graph_panes.panes.insert(key.to_string(), state);
                    return;
                };
                let sem = sem.clone();
                let cached = self
                    .ref_graph
                    .as_ref()
                    .filter(|(s, _)| Arc::ptr_eq(s, &sem))
                    .map(|(_, g)| g.clone());
                let cost = self.features.probe(Feature::Graph);
                self.graph_panes.panes.insert(key.to_string(), state);
                let key = key.to_string();
                cx.spawn(async move |this, cx| {
                    let sem2 = sem.clone();
                    let (rg, result) = cx
                        .background_spawn(async move {
                            let _t = cost.background();
                            let rg = cached.unwrap_or_else(|| Arc::new(RefGraph::build(&sem2)));
                            let result = neighbourhood_diagram(&rg, &spec)
                                .map(|(d, at)| (lay_out(&d, spec.orientation), at));
                            (rg, result)
                        })
                        .await;
                    this.update(cx, |this, cx| {
                        this.ref_graph = Some((sem, rg));
                        if let Some(p) = this.graph_panes.panes.get_mut(&key)
                            && p.generation.is(generation)
                        {
                            p.built = match result {
                                Ok((views, at)) => Built::Ready { views, at },
                                Err(e) => Built::Failed(e),
                            };
                            // For screenshot and verify scripts: the pane is drawn now.
                            if crate::devlog::on() {
                                eprintln!("GRAPH ready key={key}");
                            }
                        }
                        this.redraw_graph_panes(cx);
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Draws every graph pane again (after a change of size, for Fit). A view that places
    /// [`Self::render_graph_pane`] in a pane of another kind redraws that pane itself.
    pub(crate) fn redraw_graph_panes(&mut self, cx: &mut Context<Self>) {
        self.shell.update(cx, |shell, cx| {
            let panes: Vec<cho_layout::PaneId> = shell
                .workspace()
                .panes()
                .iter()
                // Tour pages draw graphs in their card panes (placeholders) too.
                .filter(|(_, c)| {
                    matches!(
                        c.kind(),
                        cho_layout::PaneKind::Graph | cho_layout::PaneKind::Placeholder
                    )
                })
                .map(|(id, _)| *id)
                .collect();
            for p in panes {
                shell.refresh_pane(p, cx);
            }
        });
        cx.notify();
    }

    /// Forgets the pane under `key`.
    pub(crate) fn graph_pane_drop(&mut self, key: &str) {
        self.graph_panes.panes.remove(key);
    }

    /// The qualifier for a `PaneKind::Graph` pane's header.
    pub(super) fn graph_pane_qualifier(&self, content: &cho_layout::PaneContent) -> String {
        match content {
            cho_layout::PaneContent::Graph { query, .. } => GraphSpec::from_query(query)
                .map(|s| s.qualifier())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// The body of a `PaneKind::Graph` pane: its query names the spec (and is its key).
    pub(super) fn render_graph_pane_content(
        &mut self,
        content: &cho_layout::PaneContent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cho_layout::PaneContent::Graph { query, .. } = content else {
            return div().into_any_element();
        };
        if !self.graph_panes.panes.contains_key(query.as_str()) {
            match GraphSpec::from_query(query) {
                Some(spec) => self.graph_pane_set(query, spec, cx),
                None => {
                    let c = crate::pane_style::colors(&self.theme, cx);
                    return message(
                        "This graph pane names nothing Chorong can draw",
                        c.text_muted,
                    );
                }
            }
        }
        self.render_graph_pane(query, cx)
    }

    /// Show graph popover, "In a pane": the selected declaration's neighbourhood, with the
    /// popover's direction and depth, as a graph pane (bottom bar).
    pub(super) fn graph_confirm_in_pane(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Graph(p)) = self.overlay.take() else {
            return;
        };
        let Some(Scope::Symbol { symbol, label }) = p.scopes.into_iter().nth(p.selected) else {
            self.notify_text("A graph pane starts from a declaration", cx);
            return;
        };
        let spec = GraphSpec::neighbourhood(
            symbol,
            label,
            self.graph_opts.direction,
            self.graph_opts.depth,
        );
        self.open_graph_pane(spec, cx);
    }

    /// Opens (or shows) a graph pane for a neighbourhood spec.
    pub(crate) fn open_graph_pane(&mut self, spec: GraphSpec, cx: &mut Context<Self>) {
        let Some(query) = spec.to_query() else {
            return;
        };
        self.graph_pane_set(&query, spec, cx);
        let content = cho_layout::PaneContent::Graph {
            query: query.clone(),
            title: "Graph".into(),
        };
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| {
                let existing = ws.panes().iter().find_map(|(id, c)| {
                    matches!(c, cho_layout::PaneContent::Graph { query: q, .. } if *q == query)
                        .then_some(*id)
                });
                let pane = match existing {
                    Some(p) => p,
                    None => ws.open(content, None)?,
                };
                ws.reveal(pane)
            });
        });
        cx.notify();
    }

    fn graph_zoom(&mut self, key: &str, zoom: Zoom, cx: &mut Context<Self>) {
        if let Some(p) = self.graph_panes.panes.get_mut(key) {
            p.zoom = zoom;
        }
        self.redraw_graph_panes(cx);
    }

    /// A click on node `index` of the layout: opens its declaration.
    fn graph_node_clicked(&mut self, key: &str, index: usize, cx: &mut Context<Self>) {
        let Some(p) = self.graph_panes.panes.get(key) else {
            return;
        };
        let (Some((diagram, layout)), Built::Ready { at, .. }) = (p.shown(), &p.built) else {
            return;
        };
        let Some(nb) = layout.nodes.get(index) else {
            return;
        };
        let Some(loc) = at.get(&nb.id).cloned() else {
            let what = match diagram.nodes[nb.index].style {
                NodeStyle::External => "is not in the code",
                _ => "has no place in the code",
            };
            self.notify_text(format!("{} {what}", nb.label), cx);
            return;
        };
        // On a tour page, a node that is a tour stop opens like the stop: tinted, with its note.
        if self.tour_node_clicked(&loc, cx) {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.open(
            root.join(&loc.path),
            Target {
                byte: loc.range.as_ref().map(|r| r.start),
                line: Some(loc.line.saturating_sub(1)),
                ..Default::default()
            },
            true,
            cx,
        );
    }

    /// Open in browser: today's HTML page for a neighbourhood; the SVG for a diagram.
    fn graph_open_in_browser(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(p) = self.graph_panes.panes.get(key) else {
            return;
        };
        let dark = self.theme.appearance == cho_theme::Appearance::Dark;
        match &p.spec.source {
            GraphSource::Neighbourhood {
                symbol,
                label,
                direction,
                depth,
            } => {
                let saved = std::mem::replace(
                    &mut self.graph_opts,
                    GraphOptions {
                        direction: *direction,
                        depth: *depth,
                        kinds: Vec::new(),
                        share: false,
                    },
                );
                self.build_graph(
                    Scope::Symbol {
                        symbol: symbol.clone(),
                        label: label.clone(),
                    },
                    cx,
                );
                self.graph_opts = saved;
            }
            GraphSource::Diagram { .. } => {
                let Some((diagram, layout)) = p.shown() else {
                    return;
                };
                let svg = cho_graph::to_svg(diagram, layout, Some(&self.theme));
                let written = graph::out_dir()
                    .ok_or_else(|| "no cache directory".to_string())
                    .and_then(|dir| {
                        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                        let path = dir.join(format!(
                            "diagram-{:016x}{}.svg",
                            fnv1a(svg.as_bytes()),
                            if dark { "-dark" } else { "" }
                        ));
                        std::fs::write(&path, svg).map_err(|e| e.to_string())?;
                        Ok(path)
                    });
                match written {
                    Ok(path) => cx.open_url(&format!("file://{}", path.display())),
                    Err(e) => self.notify_text(format!("Open in browser: {e}"), cx),
                }
            }
        }
    }

    /// The graph pane under `key`: the drawing and a toolbar (legend, Fit, 1:1, Open in
    /// browser). Call [`Self::graph_pane_set`] first.
    pub(crate) fn render_graph_pane(&mut self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let c = crate::pane_style::colors(&self.theme, cx);
        // An index arrived since: build the neighbourhood now.
        if let Some(p) = self.graph_panes.panes.get(key)
            && matches!(p.built, Built::NeedsIndex)
            && matches!(self.index, IndexState::Ready(..))
        {
            let spec = p.spec.clone();
            self.graph_pane_set(key, spec, cx);
        }
        let Some(p) = self.graph_panes.panes.get(key) else {
            return message("No graph", c.text_muted);
        };
        let body = match &p.built {
            Built::Building => message("Laying out the graph\u{2026}", c.text_muted),
            Built::Failed(e) => message(e, c.text_muted),
            Built::NeedsIndex => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .px_3()
                .text_size(px(12.5))
                .text_color(c.text_muted)
                .child("This graph is built from the index, and there is none yet.")
                .child(self.build_button(cx))
                .into_any_element(),
            Built::Ready { .. } => match p.shown() {
                Some((diagram, layout)) if !layout.nodes.is_empty() => {
                    self.draw_diagram(key, diagram, layout, p, &c, cx)
                }
                _ => message("Nothing to draw", c.text_muted),
            },
        };
        let zoom = p.zoom;
        let notes = match &p.built {
            Built::Ready { .. } => p
                .shown()
                .map(|(d, l)| {
                    let mut notes = d.notes.clone();
                    let (vw, vh) = p.viewport.get();
                    let s = match p.zoom {
                        Zoom::Actual => 1.0,
                        Zoom::Fit => {
                            fit_scale(l.width as f32, l.height as f32, (vw, vh)).max(MIN_FIT)
                        }
                    };
                    if vw > 0. && (l.width as f32 * s > vw + 1. || l.height as f32 * s > vh + 1.) {
                        notes.push("scroll for more".into());
                    }
                    notes.join(" \u{b7} ")
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
        let button = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .px(px(6.))
                .rounded(px(3.))
                .border_1()
                .border_color(c.border)
                .bg(if on { c.ghost_selected } else { c.element_bg })
                .text_color(c.text)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover))
                .child(label)
        };
        let legend = |label: &'static str, l: LineKind| {
            let colour = line_colour(l, &c);
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(line_sample(l, colour))
                .child(label)
        };
        let (k1, k2, k3) = (key.to_string(), key.to_string(), key.to_string());
        let toolbar =
            div()
                .flex_none()
                .h(px(TOOLBAR_H))
                .px_2()
                .flex()
                .items_center()
                .gap(px(10.))
                .border_t_1()
                .border_color(c.border_variant)
                .text_size(px(12.))
                .text_color(c.text_muted)
                .whitespace_nowrap()
                .overflow_hidden()
                .child(legend("verified", LineKind::Verified))
                .child(legend("claimed", LineKind::Claimed))
                .child(legend("broken", LineKind::Broken))
                .when(!notes.is_empty(), |d| d.child(notes))
                .child(div().flex_1())
                .child(button("graph-fit", "Fit", zoom == Zoom::Fit).on_click(
                    cx.listener(move |this, _, _, cx| this.graph_zoom(&k1, Zoom::Fit, cx)),
                ))
                .child(
                    button("graph-actual", "1:1", zoom == Zoom::Actual).on_click(
                        cx.listener(move |this, _, _, cx| this.graph_zoom(&k2, Zoom::Actual, cx)),
                    ),
                )
                .child(button("graph-browser", "Open in browser", false).on_click(
                    cx.listener(move |this, _, _, cx| this.graph_open_in_browser(&k3, cx)),
                ));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.editor)
            .child(div().flex_1().min_h_0().relative().child(body))
            .child(toolbar)
            .into_any_element()
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one paint pass needs the pane key, diagram, layout, pane state and colours"
    )]
    fn draw_diagram(
        &self,
        key: &str,
        diagram: &Diagram,
        layout: &Layout,
        p: &PaneState,
        c: &cho_layout_ui::colors::Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (zoom, viewport) = (p.zoom, p.viewport.clone());
        let (w, h) = (layout.width as f32, layout.height as f32);
        let (vw, vh) = viewport.get();
        let s = match zoom {
            Zoom::Actual => 1.0,
            Zoom::Fit if vw <= 0. || vh <= 0. => 1.0,
            Zoom::Fit => fit_scale(w, h, (vw, vh)).max(MIN_FIT),
        };
        let (dw, dh) = (w * s, h * s);
        let (ox, oy) = (((vw - dw) / 2.).max(0.), ((vh - dh) / 2.).max(0.));
        // Too big for the pane: open at the top left, or centred on the focus when that would
        // leave the focus out of view; once per zoom and pane size, so scrolling afterwards is
        // the user's. The toolbar then says there is more.
        let key_now = (zoom, vw.round() as i32, vh.round() as i32);
        if vw > 0. && p.centred.get() != Some(key_now) {
            p.centred.set(Some(key_now));
            let focus = layout
                .nodes
                .iter()
                .find(|n| n.style == NodeStyle::Focus)
                .or(layout.nodes.first());
            let (fx, fy) = focus
                .map(|n| {
                    (
                        (n.rect.x as f32 + n.rect.w as f32 / 2.) * s,
                        (n.rect.y as f32 + n.rect.h as f32 / 2.) * s,
                    )
                })
                .unwrap_or_default();
            let seen = |f: f32, v: f32| f >= 24. && f <= v - 24.;
            let x = if seen(fx, vw) {
                0.
            } else {
                (fx - vw / 2.).clamp(0., (dw - vw).max(0.))
            };
            let y = if seen(fy, vh) {
                0.
            } else {
                (fy - vh / 2.).clamp(0., (dh - vh).max(0.))
            };
            p.scroll.set_offset(gpui::point(px(-x), px(-y)));
        }
        let mono = cx.global::<cho_fonts::Fonts>().mono.clone();
        // `text.accent` at 18% (design/visual-design.md, "Verification labels").
        let focus_fill = hsla(self.theme.ui.text_accent.with_alpha(46));
        let mut canvas_div = div()
            .relative()
            .flex_none()
            .w(px(dw))
            .h(px(dh))
            .ml(px(ox))
            .mt(px(oy));
        // Group boxes.
        for g in &layout.groups {
            let r = g.rect;
            canvas_div = canvas_div.child(
                div()
                    .absolute()
                    .left(px(r.x as f32 * s))
                    .top(px(r.y as f32 * s))
                    .w(px(r.w as f32 * s))
                    .h(px(r.h as f32 * s))
                    .rounded(px(6. * s))
                    .border_1()
                    .border_color(c.border_variant)
                    .bg(c.element_bg)
                    .child(
                        div()
                            .pl(px(8. * s))
                            .pt(px(3. * s))
                            .text_size(px(12. * s))
                            .text_color(c.text_muted)
                            .whitespace_nowrap()
                            .child(g.label.clone()),
                    ),
            );
        }
        // Edges: painted paths with arrowheads.
        let paths: Vec<Painted> = layout
            .edges
            .iter()
            .map(|e| (e.points.clone(), e.line, line_colour(e.line, c)))
            .collect();
        canvas_div = canvas_div.child(
            canvas(
                |_, _, _| (),
                move |b, _, window, _| {
                    let at = |(x, y): (i32, i32)| {
                        gpui::point(b.origin.x + px(x as f32 * s), b.origin.y + px(y as f32 * s))
                    };
                    for (points, line, colour) in &paths {
                        if points.len() < 2 {
                            continue;
                        }
                        let mut p = PathBuilder::stroke(px(1.25));
                        if let Some([a, g]) = line.dash() {
                            p = p.dash_array(&[
                                px(a as f32 * s.max(0.8)),
                                px(g as f32 * s.max(0.8)),
                            ]);
                        }
                        p.move_to(at(points[0]));
                        for &q in &points[1..] {
                            p.line_to(at(q));
                        }
                        if let Ok(path) = p.build() {
                            window.paint_path(path, *colour);
                        }
                        let [a, bb] = cho_graph::arrow_base(points);
                        let mut head = PathBuilder::fill();
                        head.move_to(at(*points.last().unwrap_or(&points[0])));
                        head.line_to(at(a));
                        head.line_to(at(bb));
                        head.close();
                        if let Ok(path) = head.build() {
                            window.paint_path(path, *colour);
                        }
                    }
                },
            )
            .absolute()
            .left_0()
            .top_0()
            .size_full(),
        );
        // Edge labels.
        for e in &layout.edges {
            if let (Some(t), Some(r)) = (&e.label, e.label_rect) {
                canvas_div = canvas_div.child(
                    div()
                        .absolute()
                        .left(px(r.x as f32 * s))
                        .top(px(r.y as f32 * s))
                        .w(px(r.w as f32 * s))
                        .h(px(r.h as f32 * s))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .bg(c.editor)
                        .text_size(px(cho_graph::FONT_PX as f32 * s))
                        .text_color(c.text_muted)
                        .font_family(mono.clone())
                        .whitespace_nowrap()
                        .child(t.clone()),
                );
            }
        }
        // Nodes.
        for (i, nb) in layout.nodes.iter().enumerate() {
            let n = &diagram.nodes[nb.index];
            let (fill, border, text, dashed) =
                node_colours(nb.style, n.kind.as_deref(), c, focus_fill);
            let r = nb.rect;
            let k = key.to_string();
            let tip_text = n.detail.clone().unwrap_or_else(|| n.label.clone());
            canvas_div = canvas_div.child(
                div()
                    .id(("graph-node", i))
                    .absolute()
                    .left(px(r.x as f32 * s))
                    .top(px(r.y as f32 * s))
                    .w(px(r.w as f32 * s))
                    .h(px(r.h as f32 * s))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5. * s))
                    .border_1()
                    .when(dashed, |d| d.border_dashed())
                    .border_color(border)
                    .bg(fill)
                    .text_color(text)
                    .text_size(px(cho_graph::FONT_PX as f32 * s))
                    .font_family(mono.clone())
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|st| st.border_color(c.text_accent))
                    .tooltip(tip(tip_text))
                    .child(nb.label.clone())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.graph_node_clicked(&k, i, cx)),
                    ),
            );
        }
        // Measure the drawing area for Fit; a change of size draws again.
        let cho = cx.entity().downgrade();
        let measure = canvas(
            move |b, _, cx| {
                let size = (f32::from(b.size.width), f32::from(b.size.height));
                let old = viewport.get();
                if (old.0 - size.0).abs() > 0.5 || (old.1 - size.1).abs() > 0.5 {
                    viewport.set(size);
                    // Pane bodies are cached: ask for the graph panes to be drawn again.
                    let cho = cho.clone();
                    cx.defer(move |cx| {
                        if let Some(o) = cho.upgrade() {
                            o.update(cx, |o, cx| o.redraw_graph_panes(cx));
                        }
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full();
        div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .child(measure)
            .child(
                self.scroll_areas
                    .get(format!("graph-scroll-{key}"))
                    .tracking(p.scroll.clone())
                    .both(
                        SharedString::from(format!("graph-scroll-{key}")),
                        self.features.probe_if_on(Feature::Overscroll),
                        div().child(canvas_div),
                    )
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full(),
            )
            .into_any_element()
    }
}

/// An edge as painted: its route, kind and colour.
type Painted = (Vec<(i32, i32)>, LineKind, Hsla);

/// A short sample of a line kind for the legend.
fn line_sample(l: LineKind, colour: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let y = b.origin.y + b.size.height / 2.;
            let mut p = PathBuilder::stroke(px(1.25));
            if let Some([a, g]) = l.dash() {
                p = p.dash_array(&[px(a as f32), px(g as f32)]);
            }
            p.move_to(gpui::point(b.origin.x, y));
            p.line_to(gpui::point(b.origin.x + b.size.width, y));
            if let Ok(path) = p.build() {
                window.paint_path(path, colour);
            }
        },
    )
    .w(px(18.))
    .h(px(8.))
}

/// A stable name for a diagram file (FNV-1a of its bytes).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn message(text: &str, muted: Hsla) -> AnyElement {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbourhood_specs_round_trip_through_the_pane_query() {
        let s = GraphSpec::neighbourhood(
            "rust-analyzer cargo app 0.1.0 Server#handle().".into(),
            "Server::handle".into(),
            Direction::Out,
            1,
        );
        let q = s.to_query().unwrap();
        assert_eq!(GraphSpec::from_query(&q), Some(s.clone()));
        assert_eq!(s.qualifier(), "Server::handle \u{b7} out \u{b7} depth 1");
        assert!(GraphSpec::from_query("not json").is_none());
        let deep = GraphSpec::neighbourhood("x".into(), "x".into(), Direction::In, 99);
        assert!(
            matches!(deep.source, GraphSource::Neighbourhood { depth, .. } if depth == graph::MAX_DEPTH)
        );
        let d = GraphSpec::diagram(Diagram::default(), BTreeMap::new());
        assert!(d.to_query().is_none());
    }
}

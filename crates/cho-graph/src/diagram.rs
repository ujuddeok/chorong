//! Interim diagrams: a small directed graph laid out deterministically in Rust, for the
//! viewer's graph pane and the CLI's SVG (`docs/public/graph.md`, "Diagrams").
//!
//! The layout is a small Sugiyama-style layering written here (no dependency): cycles broken by
//! a depth-first walk in input order, layers by longest path, dummy points on long edges,
//! crossings reduced by barycentre sweeps, positions pulled toward neighbours, and orthogonal
//! edge routes with one track per bend. Groups are drawn as boxes: each group gets its own
//! column (a row when laid out left to right), so boxes never overlap.
//!
//! The same input always gives the same output: every order comes from the input order, ties
//! are broken by index, nothing is hashed, and nothing is read from the environment. Text is
//! measured with a fixed advance for 12 px monospace, so the result does not depend on fonts.

use crate::model::{EdgeStatus, Graph, NodeKind};
use serde::Serialize;
use std::collections::BTreeMap;

/// Text size of node labels, in px. The SVG and the pane draw at this size.
pub const FONT_PX: i32 = 12;
/// Advance of one character of 12 px monospace, in tenths of a px (0.6 em).
const CHAR_W_10: i32 = 72;
/// Labels longer than this are cut with an ellipsis.
pub const MAX_LABEL_CHARS: usize = 40;
const NODE_H: i32 = 28;
const NODE_PAD_X: i32 = 10;
const NODE_GAP: i32 = 24;
const LAYER_GAP: i32 = 48;
const TRACK_GAP: i32 = 8;
const MARGIN: i32 = 16;
const GROUP_PAD: i32 = 12;
const GROUP_LABEL_H: i32 = 18;
const GROUP_GAP: i32 = 24;
const SWEEPS: usize = 8;
const ALIGN_PASSES: usize = 4;

/// Which way edges run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    TopDown,
    #[default]
    LeftRight,
}

/// How a node is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeStyle {
    #[default]
    Normal,
    /// The node the diagram is about (the root of a neighbourhood).
    Focus,
    /// Not in the code: a third-party package, or a node an investigation names that has no
    /// declaration in the repository.
    External,
    /// "+N more": stands for nodes left out.
    More,
}

/// How sure an edge is (`design/visual-design.md`, "Verification labels").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineKind {
    /// From the index, or checked against it. Solid.
    #[default]
    Verified,
    /// Stated by an author or mapped without the index recording it. Dashed.
    Claimed,
    /// Checked and found wrong. Dotted, in the error colour.
    Broken,
}

impl LineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Claimed => "claimed",
            Self::Broken => "broken",
        }
    }

    /// Dash and gap lengths in px: `None` for a solid line.
    pub fn dash(self) -> Option<[i32; 2]> {
        match self {
            Self::Verified => None,
            Self::Claimed => Some([5, 4]),
            Self::Broken => Some([2, 3]),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiagramNode {
    /// Unique within the diagram; later nodes with the same id are ignored.
    pub id: String,
    pub label: String,
    /// Nodes with the same group are drawn inside one box with this title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub style: NodeStyle,
    /// Outline kind (`function`, `struct`, ...), for the label colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// One line shown on hover: path and line, or what the node stands for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Where a click goes (a `chorong://open` link).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiagramEdge {
    pub from: String,
    pub to: String,
    pub line: LineKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// What to lay out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Diagram {
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
    pub orientation: Orientation,
    /// Short statements about what was left out ("2 tests hidden"); shown beside the drawing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A rectangle in px, origin top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeBox {
    /// Index into [`Diagram::nodes`].
    pub index: usize,
    pub id: String,
    /// The label as drawn (cut to [`MAX_LABEL_CHARS`]).
    pub label: String,
    pub rect: Rect,
    pub style: NodeStyle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EdgeRoute {
    /// Index into [`Diagram::edges`].
    pub index: usize,
    pub line: LineKind,
    /// From the source's side to the target's side; the arrow is at the last point.
    pub points: Vec<(i32, i32)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Where the label is drawn (centred in this box).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_rect: Option<Rect>,
    /// The edge runs against the layering because it closes a cycle.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reversed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupBox {
    pub label: String,
    pub rect: Rect,
}

/// Where everything goes. Coordinates are whole px from the top left of a `width` x
/// `height` canvas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    pub orientation: Orientation,
    pub groups: Vec<GroupBox>,
    pub edges: Vec<EdgeRoute>,
    pub nodes: Vec<NodeBox>,
    /// Edges left out because an end is not a node.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped_edges: Vec<usize>,
}

impl Layout {
    /// The node under a point, if any.
    pub fn node_at(&self, x: i32, y: i32) -> Option<&NodeBox> {
        self.nodes.iter().find(|n| n.rect.contains(x, y))
    }
}

/// Text width of `s` in px at [`FONT_PX`] monospace.
pub fn text_width(s: &str) -> i32 {
    (s.chars().count() as i32 * CHAR_W_10 + 9) / 10
}

/// `s` cut to `max` characters, the last one an ellipsis.
pub fn cut_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

/// One point in the layered graph: a node, or a bend of a long edge.
struct Vertex {
    /// Real node index, or `None` for a dummy.
    node: Option<usize>,
    layer: usize,
    /// Size across the layer (width top-down, height left-right).
    cross: i32,
    /// Size along the edges.
    main: i32,
    cluster: usize,
}

/// An edge between adjacent layers, `upper` in layer `l`, `lower` in `l + 1`.
#[derive(Clone, Copy)]
struct Seg {
    upper: usize,
    lower: usize,
    /// Which input edge it belongs to.
    edge: usize,
}

/// Lays `d` out. Pure: the same diagram always gives the same layout.
pub fn layout(d: &Diagram) -> Layout {
    let td = d.orientation == Orientation::TopDown;
    let inp = collect_inputs(d);
    let n = inp.nodes.len();
    let (reversed, dag) = break_cycles(n, &inp.edges);
    let layer = assign_layers(n, &dag);
    let cl = assign_clusters(d, &inp.nodes);
    let lay = build_vertices(td, &inp.labels, &layer, &dag, &cl);
    let mut ord = order_layers(&lay);
    let cluster_order = order_clusters(&lay.verts, cl.names.len(), &mut ord);
    let (col_start, col_width) =
        cluster_columns(td, &lay.verts, &ord.layers, &cl.names, &cluster_order);
    let lo = place_cross(&lay.verts, &ord, &cluster_order, &col_start, &col_width);
    let tr = route_tracks(&lay, &lo);
    let label_room = label_rooms(d, &inp, &lay);
    let axis = main_axis(
        td,
        cl.has_groups,
        &lay.verts,
        &ord.layers,
        &tr.track_count,
        &label_room,
    );
    let node_boxes = node_boxes(d, &inp, &lay.verts, &lo, &axis);
    let mut routes = route_edges(d, &inp, &reversed, &lay, &axis, &tr, &node_boxes);
    let groups = group_boxes(&cluster_order, &cl, &node_boxes);

    place_labels(d, &mut routes, &node_boxes, &groups);

    let (width, height) = canvas_size(&node_boxes, &groups, &routes);
    Layout {
        width,
        height,
        orientation: d.orientation,
        groups,
        edges: routes,
        nodes: node_boxes,
        dropped_edges: inp.dropped,
    }
}

/// The nodes and edges a layout works on, after dropping duplicates and unknown ends.
struct Inputs {
    /// Input index of the first node with each id.
    nodes: Vec<usize>,
    /// Cut labels, per kept node.
    labels: Vec<String>,
    /// Edges with both ends known: (from, to, edge index).
    edges: Vec<(usize, usize, usize)>,
    /// Self loops: (node, edge index), kept apart.
    loops: Vec<(usize, usize)>,
    /// Edge indices with an unknown end.
    dropped: Vec<usize>,
}

/// Nodes (first of each id) and edges with both ends known; self loops kept apart.
fn collect_inputs(d: &Diagram) -> Inputs {
    // Nodes, first of each id.
    let mut by_id: BTreeMap<&str, usize> = BTreeMap::new();
    let mut nodes: Vec<usize> = vec![];
    for (i, n) in d.nodes.iter().enumerate() {
        if !by_id.contains_key(n.id.as_str()) {
            by_id.insert(n.id.as_str(), nodes.len());
            nodes.push(i);
        }
    }
    let labels: Vec<String> = nodes
        .iter()
        .map(|&i| cut_label(&d.nodes[i].label, MAX_LABEL_CHARS))
        .collect();

    // Edges with both ends known; self loops kept apart.
    let mut dropped = vec![];
    let mut edges: Vec<(usize, usize, usize)> = vec![]; // (from, to, edge index)
    let mut loops: Vec<(usize, usize)> = vec![];
    for (ei, e) in d.edges.iter().enumerate() {
        match (by_id.get(e.from.as_str()), by_id.get(e.to.as_str())) {
            (Some(&a), Some(&b)) if a == b => loops.push((a, ei)),
            (Some(&a), Some(&b)) => edges.push((a, b, ei)),
            _ => dropped.push(ei),
        }
    }
    Inputs {
        nodes,
        labels,
        edges,
        loops,
        dropped,
    }
}

/// Which edges to reverse, and the directed edges of the resulting acyclic graph.
fn break_cycles(n: usize, edges: &[(usize, usize, usize)]) -> (Vec<bool>, Vec<(usize, usize)>) {
    // Break cycles: a depth-first walk in input order; an edge to a node on the stack is
    // reversed.
    let mut out_adj: Vec<Vec<usize>> = vec![vec![]; n]; // edge positions in `edges`
    for (k, &(a, _, _)) in edges.iter().enumerate() {
        out_adj[a].push(k);
    }
    let mut reversed = vec![false; edges.len()];
    let mut state = vec![0u8; n]; // 0 new, 1 on stack, 2 done
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        state[root] = 1;
        while let Some(&mut (v, ref mut next)) = stack.last_mut() {
            if *next < out_adj[v].len() {
                let k = out_adj[v][*next];
                *next += 1;
                let w = edges[k].1;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => reversed[k] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    // Directed edges of the acyclic graph.
    let dag: Vec<(usize, usize)> = edges
        .iter()
        .zip(&reversed)
        .map(|(&(a, b, _), &r)| if r { (b, a) } else { (a, b) })
        .collect();
    (reversed, dag)
}

/// Layers by longest path from the sources (Kahn's order, ties by index).
fn assign_layers(n: usize, dag: &[(usize, usize)]) -> Vec<usize> {
    let mut layer = vec![0usize; n];
    let mut indeg = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![vec![]; n];
    for &(a, b) in dag {
        indeg[b] += 1;
        succ[a].push(b);
    }
    let mut ready: std::collections::BTreeSet<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
    while let Some(v) = ready.pop_first() {
        for &w in &succ[v] {
            layer[w] = layer[w].max(layer[v] + 1);
            indeg[w] -= 1;
            if indeg[w] == 0 {
                ready.insert(w);
            }
        }
    }
    layer
}

/// The groups nodes belong to.
struct Clusters {
    /// Group name per cluster; `None` for the cluster of ungrouped nodes.
    names: Vec<Option<String>>,
    /// Cluster of each node.
    of: Vec<usize>,
    /// The cluster of ungrouped nodes (also used for edges between clusters).
    ungrouped: usize,
    /// Whether any cluster has a name (and so a box).
    has_groups: bool,
}

/// Clusters: one per named group in order of first appearance, and one for nodes without a
/// group (no box).
fn assign_clusters(d: &Diagram, nodes: &[usize]) -> Clusters {
    let mut names: Vec<Option<String>> = vec![];
    let mut of = vec![0usize; nodes.len()];
    for (v, &i) in nodes.iter().enumerate() {
        let g = d.nodes[i].group.clone();
        let c = match names.iter().position(|x| *x == g) {
            Some(c) => c,
            None => {
                names.push(g);
                names.len() - 1
            }
        };
        of[v] = c;
    }
    let ungrouped = match names.iter().position(Option::is_none) {
        Some(c) => c,
        None => {
            names.push(None);
            names.len() - 1
        }
    };
    let has_groups = names.iter().any(Option::is_some);
    Clusters {
        names,
        of,
        ungrouped,
        has_groups,
    }
}

/// The layered graph: vertices, the segments between adjacent layers and each edge's chain.
struct Layered {
    verts: Vec<Vertex>,
    segs: Vec<Seg>,
    /// Per kept edge: its chain of vertices from upper to lower.
    chains: Vec<Vec<usize>>,
    layer_count: usize,
}

/// Vertices: the nodes, then dummies for edges spanning more than one layer.
fn build_vertices(
    td: bool,
    labels: &[String],
    layer: &[usize],
    dag: &[(usize, usize)],
    cl: &Clusters,
) -> Layered {
    let n = labels.len();
    let cluster_of = &cl.of;
    let mut verts: Vec<Vertex> = (0..n)
        .map(|v| {
            let w = text_width(&labels[v]) + 2 * NODE_PAD_X;
            let (cross, main) = if td { (w, NODE_H) } else { (NODE_H, w) };
            Vertex {
                node: Some(v),
                layer: layer[v],
                cross,
                main,
                cluster: cluster_of[v],
            }
        })
        .collect();
    let mut segs: Vec<Seg> = vec![];
    // Per kept edge: its chain of vertices from upper to lower.
    let mut chains: Vec<Vec<usize>> = vec![];
    for (k, &(a, b)) in dag.iter().enumerate() {
        let mut chain = vec![a];
        let cluster = if cluster_of[a] == cluster_of[b] {
            cluster_of[a]
        } else {
            cl.ungrouped
        };
        for l in layer[a] + 1..layer[b] {
            verts.push(Vertex {
                node: None,
                layer: l,
                cross: 0,
                main: 0,
                cluster,
            });
            chain.push(verts.len() - 1);
        }
        chain.push(b);
        for w in chain.windows(2) {
            segs.push(Seg {
                upper: w[0],
                lower: w[1],
                edge: k,
            });
        }
        chains.push(chain);
    }
    let layer_count = verts.iter().map(|v| v.layer + 1).max().unwrap_or(0);
    Layered {
        verts,
        segs,
        chains,
        layer_count,
    }
}

/// The vertices of each layer in order, with their neighbours and positions.
struct Ordering {
    layers: Vec<Vec<usize>>,
    /// Upper neighbours per vertex.
    ups: Vec<Vec<usize>>,
    /// Lower neighbours per vertex.
    downs: Vec<Vec<usize>>,
    /// Index of each vertex within its layer.
    pos: Vec<usize>,
}

fn set_pos(layers: &[Vec<usize>], pos: &mut [usize]) {
    for l in layers {
        for (i, &v) in l.iter().enumerate() {
            pos[v] = i;
        }
    }
}

fn reorder(l: &mut Vec<usize>, nbrs: &[Vec<usize>], pos: &[usize]) {
    let mut keyed: Vec<(f64, usize, usize)> = l
        .iter()
        .map(|&v| {
            let ns = &nbrs[v];
            let key = if ns.is_empty() {
                pos[v] as f64
            } else {
                ns.iter().map(|&u| pos[u] as f64).sum::<f64>() / ns.len() as f64
            };
            (key, pos[v], v)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    *l = keyed.into_iter().map(|k| k.2).collect();
}

/// Order within layers: input order first, then barycentre sweeps.
fn order_layers(lay: &Layered) -> Ordering {
    let (verts, segs, layer_count) = (&lay.verts, &lay.segs, lay.layer_count);
    let mut layers: Vec<Vec<usize>> = vec![vec![]; layer_count];
    for (i, v) in verts.iter().enumerate() {
        layers[v.layer].push(i);
    }
    let mut ups: Vec<Vec<usize>> = vec![vec![]; verts.len()];
    let mut downs: Vec<Vec<usize>> = vec![vec![]; verts.len()];
    for s in segs {
        downs[s.upper].push(s.lower);
        ups[s.lower].push(s.upper);
    }
    let mut pos = vec![0usize; verts.len()];
    set_pos(&layers, &mut pos);
    let mut best = layers.clone();
    let mut best_cross = crossings(&layers, &downs, &pos);
    for _ in 0..SWEEPS {
        for l in 1..layer_count {
            reorder(&mut layers[l], &ups, &pos);
            set_pos(&layers, &mut pos);
        }
        for l in (0..layer_count.saturating_sub(1)).rev() {
            reorder(&mut layers[l], &downs, &pos);
            set_pos(&layers, &mut pos);
        }
        let c = crossings(&layers, &downs, &pos);
        if c < best_cross {
            best_cross = c;
            best = layers.clone();
        }
    }
    layers = best;
    set_pos(&layers, &mut pos);
    Ordering {
        layers,
        ups,
        downs,
        pos,
    }
}

/// Clusters side by side: ordered by their mean relative position, ties by first appearance.
/// Reorders each layer by cluster; returns the clusters in left-to-right order.
fn order_clusters(verts: &[Vertex], cluster_count: usize, ord: &mut Ordering) -> Vec<usize> {
    let (layers, pos) = (&mut ord.layers, &mut ord.pos);
    let mut rank_key: Vec<(f64, usize)> = vec![(0.0, 0); cluster_count];
    for (c, key) in rank_key.iter_mut().enumerate() {
        let (mut sum, mut count) = (0.0, 0usize);
        for (v, vert) in verts.iter().enumerate() {
            if vert.cluster == c {
                let len = layers[vert.layer].len().max(1) as f64;
                sum += (pos[v] as f64 + 0.5) / len;
                count += 1;
            }
        }
        *key = (if count == 0 { 0.0 } else { sum / count as f64 }, c);
    }
    let mut cluster_order: Vec<usize> = (0..cluster_count).collect();
    cluster_order.sort_by(|&a, &b| rank_key[a].0.total_cmp(&rank_key[b].0).then(a.cmp(&b)));
    let mut cluster_rank = vec![0usize; cluster_count];
    for (r, &c) in cluster_order.iter().enumerate() {
        cluster_rank[c] = r;
    }
    for l in layers.iter_mut() {
        l.sort_by_key(|&v| (cluster_rank[verts[v].cluster], pos[v]));
    }
    set_pos(layers, pos);
    cluster_order
}

/// Gap on the cross axis after vertex `v`.
fn gap_after(verts: &[Vertex], v: usize) -> i32 {
    if verts[v].node.is_some() {
        NODE_GAP
    } else {
        TRACK_GAP
    }
}

/// Size of `row` across the layer, gaps included.
fn row_size(verts: &[Vertex], row: &[usize]) -> i32 {
    row.iter()
        .enumerate()
        .map(|(i, &v)| {
            verts[v].cross
                + if i + 1 < row.len() {
                    gap_after(verts, v)
                } else {
                    0
                }
        })
        .sum()
}

/// Cross-axis columns, one per cluster: (start, width) per cluster.
fn cluster_columns(
    td: bool,
    verts: &[Vertex],
    layers: &[Vec<usize>],
    cluster_names: &[Option<String>],
    cluster_order: &[usize],
) -> (Vec<i32>, Vec<i32>) {
    let lead = |c: usize| -> i32 {
        if cluster_names[c].is_some() {
            GROUP_PAD + if td { 0 } else { GROUP_LABEL_H }
        } else {
            0
        }
    };
    let trail = |c: usize| {
        if cluster_names[c].is_some() {
            GROUP_PAD
        } else {
            0
        }
    };
    let mut col_start = vec![0i32; cluster_names.len()];
    let mut col_width = vec![0i32; cluster_names.len()];
    let mut at = MARGIN;
    for &c in cluster_order {
        let w = layers
            .iter()
            .map(|l| {
                let row: Vec<usize> = l
                    .iter()
                    .copied()
                    .filter(|&v| verts[v].cluster == c)
                    .collect();
                row_size(verts, &row)
            })
            .max()
            .unwrap_or(0);
        if w == 0 {
            continue;
        }
        at += lead(c);
        col_start[c] = at;
        col_width[c] = w;
        at += w + trail(c) + GROUP_GAP;
    }
    (col_start, col_width)
}

/// Places one row of vertices (all of one cluster) near their `desired` leading edges, inside
/// the cluster's column, writing the result to `lo`.
fn place_row(
    verts: &[Vertex],
    col_start: &[i32],
    col_width: &[i32],
    row: &[usize],
    desired: &[f64],
    lo: &mut [f64],
) {
    if row.is_empty() {
        return;
    }
    let c = verts[row[0]].cluster;
    let (start, end) = (col_start[c] as f64, (col_start[c] + col_width[c]) as f64);
    // Push right from the desired spots, and push left from them; take the mean, then
    // remove what overlap is left.
    let mut right = vec![0f64; row.len()];
    let mut prev = f64::NEG_INFINITY;
    for (i, &v) in row.iter().enumerate() {
        let x = desired[i].max(start).max(prev);
        right[i] = x;
        prev = x + (verts[v].cross + gap_after(verts, v)) as f64;
    }
    let mut left = vec![0f64; row.len()];
    let mut next = f64::INFINITY;
    for (i, &v) in row.iter().enumerate().rev() {
        let gap = if i + 1 < row.len() {
            gap_after(verts, v)
        } else {
            0
        };
        let x = desired[i]
            .min(end - verts[v].cross as f64)
            .min(next - (verts[v].cross + gap) as f64);
        left[i] = x;
        next = x;
    }
    let mut prev = start;
    for (i, &v) in row.iter().enumerate() {
        let x = ((right[i] + left[i]) / 2.0).round().max(prev);
        lo[v] = x;
        prev = x + (verts[v].cross + gap_after(verts, v)) as f64;
    }
    // Keep the row inside its column.
    let last = *row.last().unwrap_or(&row[0]);
    let over = lo[last] + verts[last].cross as f64 - end;
    if over > 0.0 {
        for &v in row {
            lo[v] -= over;
        }
    }
}

/// Positions across: centred in the column, then pulled toward neighbours. Returns the
/// leading edge of each vertex on the cross axis.
fn place_cross(
    verts: &[Vertex],
    ord: &Ordering,
    cluster_order: &[usize],
    col_start: &[i32],
    col_width: &[i32],
) -> Vec<i32> {
    let (layers, ups, downs) = (&ord.layers, &ord.ups, &ord.downs);
    let layer_count = layers.len();
    let mut lo = vec![0f64; verts.len()]; // leading edge on the cross axis
    let rows: Vec<Vec<Vec<usize>>> = layers
        .iter()
        .map(|l| {
            cluster_order
                .iter()
                .map(|&c| {
                    l.iter()
                        .copied()
                        .filter(|&v| verts[v].cluster == c)
                        .collect::<Vec<_>>()
                })
                .filter(|r| !r.is_empty())
                .collect()
        })
        .collect();
    for layer_rows in &rows {
        for row in layer_rows {
            let c = verts[row[0]].cluster;
            let start = col_start[c] + (col_width[c] - row_size(verts, row)) / 2;
            let mut x = start;
            for &v in row {
                lo[v] = x as f64;
                x += verts[v].cross + gap_after(verts, v);
            }
        }
    }
    let centre = |v: usize, lo: &[f64]| lo[v] + verts[v].cross as f64 / 2.0;
    for pass in 0..ALIGN_PASSES * 2 {
        let downward = pass % 2 == 0;
        let order: Vec<usize> = if downward {
            (0..layer_count).collect()
        } else {
            (0..layer_count).rev().collect()
        };
        for l in order {
            for row in &rows[l] {
                let desired: Vec<f64> = row
                    .iter()
                    .map(|&v| {
                        let nb = if downward { &ups[v] } else { &downs[v] };
                        let nb = if nb.is_empty() {
                            if downward { &downs[v] } else { &ups[v] }
                        } else {
                            nb
                        };
                        if nb.is_empty() {
                            lo[v]
                        } else {
                            let mut cs: Vec<f64> = nb.iter().map(|&u| centre(u, &lo)).collect();
                            cs.sort_by(f64::total_cmp);
                            let m = if cs.len() % 2 == 1 {
                                cs[cs.len() / 2]
                            } else {
                                (cs[cs.len() / 2 - 1] + cs[cs.len() / 2]) / 2.0
                            };
                            m - verts[v].cross as f64 / 2.0
                        }
                    })
                    .collect();
                place_row(verts, col_start, col_width, row, &desired, &mut lo);
            }
        }
    }
    lo.iter().map(|x| x.round() as i32).collect()
}

/// Where each segment starts and ends across, and the bend track it uses.
struct Tracks {
    /// Cross position where each segment leaves its upper vertex.
    port_out: Vec<i32>,
    /// Cross position where each segment arrives at its lower vertex.
    port_in: Vec<i32>,
    /// Bend track of each segment in the gap below its layer, if it bends.
    seg_track: Vec<Option<usize>>,
    /// Tracks used in the gap below each layer.
    track_count: Vec<usize>,
}

/// Ports and bundles: the edges leaving one node for the next layer share one trunk (a single
/// bend track), and so do the edges arriving at one node when their sources have no other.
fn route_tracks(lay: &Layered, lo: &[i32]) -> Tracks {
    let (verts, segs, layer_count) = (&lay.verts, &lay.segs, lay.layer_count);
    // Ports: the middle of a node's side (one line out, one line in); a bend point's own spot.
    let port = |v: usize| {
        if verts[v].node.is_some() {
            lo[v] + verts[v].cross / 2
        } else {
            lo[v]
        }
    };
    let port_out: Vec<i32> = segs.iter().map(|s| port(s.upper)).collect();
    let port_in: Vec<i32> = segs.iter().map(|s| port(s.lower)).collect();
    // Bundles: the edges leaving one node for the next layer share one trunk (a single bend
    // track), and so do the edges arriving at one node when their sources have no other.
    let (mut outdeg, mut indeg) = (vec![0usize; verts.len()], vec![0usize; verts.len()]);
    for sg in segs {
        outdeg[sg.upper] += 1;
        indeg[sg.lower] += 1;
    }
    // (0: by source, 1: by target, 2: alone; the vertex or segment).
    type Bundle = (u8, usize);
    let bundle_of = |i: usize| -> Bundle {
        let sg = segs[i];
        if outdeg[sg.upper] >= 2 {
            (0, sg.upper)
        } else if indeg[sg.lower] >= 2 {
            (1, sg.lower)
        } else {
            (2, i)
        }
    };
    let mut seg_track: Vec<Option<usize>> = vec![None; segs.len()];
    let mut track_count = vec![0usize; layer_count];
    for (l, count) in track_count.iter_mut().enumerate() {
        let mut bundles: BTreeMap<Bundle, (i32, i32, Vec<usize>)> = BTreeMap::new();
        for (i, sg) in segs.iter().enumerate() {
            if verts[sg.upper].layer != l {
                continue;
            }
            let b = bundles
                .entry(bundle_of(i))
                .or_insert((i32::MAX, i32::MIN, vec![]));
            b.0 = b.0.min(port_out[i]).min(port_in[i]);
            b.1 = b.1.max(port_out[i]).max(port_in[i]);
            b.2.push(i);
        }
        let mut list: Vec<(i32, i32, Bundle, Vec<usize>)> = bundles
            .into_iter()
            .filter(|(_, (a, b, _))| a != b)
            .map(|(k, (a, b, m))| (a, b, k, m))
            .collect();
        list.sort();
        let mut ends: Vec<i32> = vec![];
        for (a, b, _, members) in list {
            let t = match ends.iter().position(|&e| e + TRACK_GAP < a) {
                Some(t) => t,
                None => {
                    ends.push(i32::MIN);
                    ends.len() - 1
                }
            };
            ends[t] = b;
            for m in members {
                seg_track[m] = Some(t);
            }
        }
        *count = ends.len();
    }
    Tracks {
        port_out,
        port_in,
        seg_track,
        track_count,
    }
}

/// Room for labels on both sides of the tracks, in the gap where a labelled edge is.
fn label_rooms(d: &Diagram, inp: &Inputs, lay: &Layered) -> Vec<i32> {
    let td = d.orientation == Orientation::TopDown;
    let mut label_room = vec![0i32; lay.layer_count];
    for (k, chain) in lay.chains.iter().enumerate() {
        let Some(t) = d.edges[inp.edges[k].2]
            .label
            .as_deref()
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        let room = if td {
            FONT_PX + 6 + 8
        } else {
            text_width(&cut_label(t, MAX_LABEL_CHARS)) + 8 + 8
        };
        for w in chain.windows(2) {
            let l = lay.verts[w[0]].layer;
            label_room[l] = label_room[l].max(room);
        }
    }
    label_room
}

/// Main-axis positions of the layers.
struct MainAxis {
    /// Size of each layer along the main axis.
    thick: Vec<i32>,
    /// Where each layer starts.
    layer_start: Vec<i32>,
    /// Gap after each layer.
    gap: Vec<i32>,
}

impl MainAxis {
    /// Where vertex `v` starts along the main axis, centred in its layer.
    fn main_lo(&self, verts: &[Vertex], v: usize) -> i32 {
        self.layer_start[verts[v].layer] + (self.thick[verts[v].layer] - verts[v].main) / 2
    }

    /// Where track `t` of the gap below layer `l` runs.
    fn track_main(&self, track_count: &[usize], l: usize, t: usize) -> i32 {
        let n = track_count[l] as i32;
        let gap_start = self.layer_start[l] + self.thick[l];
        gap_start + (self.gap[l] - (n - 1) * TRACK_GAP) / 2 + t as i32 * TRACK_GAP
    }
}

/// Main-axis positions of the layers.
fn main_axis(
    td: bool,
    has_groups: bool,
    verts: &[Vertex],
    layers: &[Vec<usize>],
    track_count: &[usize],
    label_room: &[i32],
) -> MainAxis {
    let layer_count = layers.len();
    let thick: Vec<i32> = (0..layer_count)
        .map(|l| layers[l].iter().map(|&v| verts[v].main).max().unwrap_or(0))
        .collect();
    let mut layer_start = vec![0i32; layer_count];
    let mut gap = vec![0i32; layer_count];
    let mut at_main = MARGIN
        + if has_groups {
            GROUP_PAD + if td { GROUP_LABEL_H } else { 0 }
        } else {
            0
        };
    for l in 0..layer_count {
        layer_start[l] = at_main;
        let tracks = (track_count[l] as i32 - 1).max(0) * TRACK_GAP;
        gap[l] = LAYER_GAP.max(32 + tracks + 2 * label_room[l]);
        at_main += thick[l] + gap[l];
    }
    MainAxis {
        thick,
        layer_start,
        gap,
    }
}

/// A point from its cross and main coordinates.
fn pt(td: bool, cross: i32, main: i32) -> (i32, i32) {
    if td { (cross, main) } else { (main, cross) }
}

/// Node boxes.
fn node_boxes(
    d: &Diagram,
    inp: &Inputs,
    verts: &[Vertex],
    lo: &[i32],
    axis: &MainAxis,
) -> Vec<NodeBox> {
    let td = d.orientation == Orientation::TopDown;
    let (nodes, labels) = (&inp.nodes, &inp.labels);
    let rect_of = |v: usize| {
        let (c, m) = (lo[v], axis.main_lo(verts, v));
        let (x, y) = pt(td, c, m);
        let (w, h) = if td {
            (verts[v].cross, verts[v].main)
        } else {
            (verts[v].main, verts[v].cross)
        };
        Rect { x, y, w, h }
    };
    (0..nodes.len())
        .map(|v| NodeBox {
            index: nodes[v],
            id: d.nodes[nodes[v]].id.clone(),
            label: labels[v].clone(),
            rect: rect_of(v),
            style: d.nodes[nodes[v]].style,
        })
        .collect()
}

/// Edge routes, in input edge order (self loops included).
fn route_edges(
    d: &Diagram,
    inp: &Inputs,
    reversed: &[bool],
    lay: &Layered,
    axis: &MainAxis,
    tr: &Tracks,
    node_boxes: &[NodeBox],
) -> Vec<EdgeRoute> {
    let td = d.orientation == Orientation::TopDown;
    let (verts, segs, chains) = (&lay.verts, &lay.segs, &lay.chains);
    let (layer_start, thick) = (&axis.layer_start, &axis.thick);
    let seg_index: BTreeMap<(usize, usize, usize), usize> = segs
        .iter()
        .enumerate()
        .map(|(i, s)| ((s.edge, s.upper, s.lower), i))
        .collect();
    let mut routes: Vec<EdgeRoute> = vec![];
    for (k, chain) in chains.iter().enumerate() {
        let mut points: Vec<(i32, i32)> = vec![];
        for w in chain.windows(2) {
            let s = seg_index[&(k, w[0], w[1])];
            let l = verts[w[0]].layer;
            let start_main = axis.main_lo(verts, w[0]) + verts[w[0]].main;
            let start_main = if verts[w[0]].node.is_none() {
                layer_start[l] + thick[l]
            } else {
                start_main
            };
            let end_main = if verts[w[1]].node.is_none() {
                layer_start[l + 1]
            } else {
                axis.main_lo(verts, w[1])
            };
            let (a, b) = (tr.port_out[s], tr.port_in[s]);
            points.push(pt(td, a, start_main));
            if let Some(t) = tr.seg_track[s] {
                let tm = axis.track_main(&tr.track_count, l, t);
                points.push(pt(td, a, tm));
                points.push(pt(td, b, tm));
            }
            points.push(pt(td, b, end_main));
        }
        simplify(&mut points);
        let ei = inp.edges[k].2;
        if reversed[k] {
            points.reverse();
        }
        routes.push(EdgeRoute {
            index: ei,
            line: d.edges[ei].line,
            points,
            label: None,
            label_rect: None,
            reversed: reversed[k],
        });
    }
    for &(v, ei) in &inp.loops {
        let r = node_boxes[v].rect;
        let points = if td {
            let (x, y) = (r.x + r.w, r.y + r.h / 2);
            vec![(x, y - 6), (x + 14, y - 6), (x + 14, y + 6), (x, y + 6)]
        } else {
            let (x, y) = (r.x + r.w / 2, r.y + r.h);
            vec![(x - 6, y), (x - 6, y + 14), (x + 6, y + 14), (x + 6, y)]
        };
        routes.push(EdgeRoute {
            index: ei,
            line: d.edges[ei].line,
            points,
            label: None,
            label_rect: None,
            reversed: false,
        });
    }
    routes.sort_by_key(|r| r.index);
    routes
}

/// Group boxes around their members.
fn group_boxes(cluster_order: &[usize], cl: &Clusters, node_boxes: &[NodeBox]) -> Vec<GroupBox> {
    let mut groups = vec![];
    for &c in cluster_order {
        let Some(name) = &cl.names[c] else {
            continue;
        };
        let members: Vec<&NodeBox> = (0..node_boxes.len())
            .filter(|&v| cl.of[v] == c)
            .map(|v| &node_boxes[v])
            .collect();
        if members.is_empty() {
            continue;
        }
        let x0 = members.iter().map(|m| m.rect.x).min().unwrap_or(0) - GROUP_PAD;
        let y0 = members.iter().map(|m| m.rect.y).min().unwrap_or(0) - GROUP_PAD - GROUP_LABEL_H;
        let x1 = members
            .iter()
            .map(|m| m.rect.x + m.rect.w)
            .max()
            .unwrap_or(0)
            + GROUP_PAD;
        let y1 = members
            .iter()
            .map(|m| m.rect.y + m.rect.h)
            .max()
            .unwrap_or(0)
            + GROUP_PAD;
        let label = cut_label(name, MAX_LABEL_CHARS);
        let x1 = x1.max(x0 + text_width(&label) + 2 * GROUP_PAD);
        groups.push(GroupBox {
            label,
            rect: Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            },
        });
    }
    groups
}

/// Canvas size: everything drawn, plus the margin.
fn canvas_size(node_boxes: &[NodeBox], groups: &[GroupBox], routes: &[EdgeRoute]) -> (i32, i32) {
    let mut max_x = 0;
    let mut max_y = 0;
    let mut grow = |r: &Rect| {
        max_x = max_x.max(r.x + r.w);
        max_y = max_y.max(r.y + r.h);
    };
    node_boxes.iter().for_each(|b| grow(&b.rect));
    groups.iter().for_each(|g| grow(&g.rect));
    for r in routes {
        for &(x, y) in &r.points {
            grow(&Rect { x, y, w: 0, h: 0 });
        }
        if let Some(lr) = &r.label_rect {
            grow(lr);
        }
    }
    (max_x + MARGIN, max_y + MARGIN)
}

/// Crossings between adjacent layers, counted pairwise (graphs here are small).
fn crossings(layers: &[Vec<usize>], downs: &[Vec<usize>], pos: &[usize]) -> usize {
    let mut total = 0;
    for l in layers {
        let mut pairs: Vec<(usize, usize)> = vec![];
        for &u in l {
            for &w in &downs[u] {
                pairs.push((pos[u], pos[w]));
            }
        }
        for i in 0..pairs.len() {
            for j in i + 1..pairs.len() {
                let (a, b) = (pairs[i], pairs[j]);
                if (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1) {
                    total += 1;
                }
            }
        }
    }
    total
}

/// Drops repeated points and points in the middle of a straight run.
fn simplify(points: &mut Vec<(i32, i32)>) {
    points.dedup();
    let mut i = 1;
    while i + 1 < points.len() {
        let (a, b, c) = (points[i - 1], points[i], points[i + 1]);
        if (a.0 == b.0 && b.0 == c.0) || (a.1 == b.1 && b.1 == c.1) {
            points.remove(i);
        } else {
            i += 1;
        }
    }
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// An axis-aligned segment as a 1 px wide rectangle.
fn seg_rect(a: (i32, i32), b: (i32, i32)) -> Rect {
    Rect {
        x: a.0.min(b.0),
        y: a.1.min(b.1),
        w: (a.0 - b.0).abs().max(1),
        h: (a.1 - b.1).abs().max(1),
    }
}

/// Places each edge label on its own line, on the ground colour, where it covers no box, no
/// other label and no other edge's line: tried on the last segment, then the first, then the
/// rest, at several points along each. When no spot is clear, the one with the fewest
/// overlaps wins (the first such, so the result is still deterministic).
fn place_labels(d: &Diagram, routes: &mut [EdgeRoute], nodes: &[NodeBox], groups: &[GroupBox]) {
    let mut blocked: Vec<Rect> = nodes
        .iter()
        .map(|n| Rect {
            x: n.rect.x - 2,
            y: n.rect.y - 2,
            w: n.rect.w + 4,
            h: n.rect.h + 4,
        })
        .collect();
    blocked.extend(groups.iter().map(|g| Rect {
        x: g.rect.x,
        y: g.rect.y,
        w: text_width(&g.label) + 16,
        h: GROUP_LABEL_H + 2,
    }));
    let lines: Vec<Vec<Rect>> = routes
        .iter()
        .map(|r| r.points.windows(2).map(|w| seg_rect(w[0], w[1])).collect())
        .collect();
    for (ri, route) in routes.iter_mut().enumerate() {
        let Some(text) = d.edges[route.index]
            .label
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| cut_label(t, MAX_LABEL_CHARS))
        else {
            continue;
        };
        let points = &route.points;
        let (w, h) = (text_width(&text) + 8, FONT_PX + 6);
        let n = points.len().saturating_sub(1);
        let mut order: Vec<usize> = (0..n).rev().take(1).collect();
        if n > 1 {
            order.push(0);
        }
        order.extend((1..n.saturating_sub(1)).rev());
        let mut best: Option<(usize, Rect)> = None;
        'search: for &si in &order {
            let (a, b) = (points[si], points[si + 1]);
            for f in [50, 30, 70, 15, 85] {
                let (cx, cy) = (a.0 + (b.0 - a.0) * f / 100, a.1 + (b.1 - a.1) * f / 100);
                let r = Rect {
                    x: cx - w / 2,
                    y: cy - h / 2,
                    w,
                    h,
                };
                // The label must sit on its own segment, inside its length.
                let along = if a.1 == b.1 {
                    r.x >= a.0.min(b.0) && r.x + r.w <= a.0.max(b.0)
                } else {
                    r.y >= a.1.min(b.1) && r.y + r.h <= a.1.max(b.1)
                };
                let hits = blocked.iter().filter(|o| overlaps(o, &r)).count()
                    + lines
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != ri)
                        .flat_map(|(_, segs)| segs)
                        .filter(|o| overlaps(o, &r))
                        .count()
                    + usize::from(!along);
                if best.is_none_or(|(h0, _)| hits < h0) {
                    best = Some((hits, r));
                    if hits == 0 {
                        break 'search;
                    }
                }
            }
        }
        if let Some((_, r)) = best {
            blocked.push(r);
            route.label = Some(text);
            route.label_rect = Some(r);
        }
    }
}

/// How nodes of a reference graph are boxed in a diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupBy {
    #[default]
    None,
    File,
    Package,
}

/// A reference graph (a neighbourhood from the index) as a diagram, with the HTML page's
/// defaults (`design/graph.md`, "Readable graphs"):
///
/// - only lines that lead to the focus: an edge between two nodes the walk reached at the same
///   distance is left out;
/// - tests are hidden (unless the focus is one), and [`Diagram::notes`] says how many;
/// - edges between the same two nodes become one, labelled with their kinds other than
///   `calls` and `references`; an edge with mapped references is drawn as claimed.
///
/// Nodes are the focus and the ends of the edges kept.
pub fn diagram_from_graph(g: &Graph, group_by: GroupBy, orientation: Orientation) -> Diagram {
    let test = |id: &str| g.node(id).is_some_and(|n| n.is_test && !n.focus);
    let leads = |a: &str, b: &str| match (
        g.node(a).and_then(|n| n.depth),
        g.node(b).and_then(|n| n.depth),
    ) {
        (Some(x), Some(y)) => x != y,
        _ => true,
    };
    let kept: Vec<&crate::model::Edge> = g
        .edges
        .iter()
        .filter(|e| !test(&e.from) && !test(&e.to) && leads(&e.from, &e.to))
        .collect();
    let mut used: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for e in &kept {
        used.insert(&e.from);
        used.insert(&e.to);
    }
    let hidden_tests = g
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Symbol && test(&n.id))
        .count();
    let file_label = |id: Option<&String>| -> Option<String> {
        id.and_then(|p| g.node(p))
            .map(|f| f.path.clone().unwrap_or_else(|| f.label.clone()))
    };
    let nodes: Vec<DiagramNode> = g
        .nodes
        .iter()
        .filter(|n| n.focus || used.contains(n.id.as_str()))
        .map(|n| DiagramNode {
            id: n.id.clone(),
            label: n.label.clone(),
            group: match group_by {
                GroupBy::None => None,
                GroupBy::File if n.kind == NodeKind::Symbol => file_label(n.parent.as_ref()),
                GroupBy::File => None,
                GroupBy::Package => n.package.clone(),
            },
            style: if n.focus {
                NodeStyle::Focus
            } else {
                match n.kind {
                    NodeKind::External => NodeStyle::External,
                    NodeKind::More => NodeStyle::More,
                    _ => NodeStyle::Normal,
                }
            },
            kind: n.scip_kind.clone(),
            detail: match (&n.path, n.line) {
                (Some(p), Some(l)) => Some(format!("{p}:{l}")),
                (Some(p), None) => Some(p.clone()),
                _ => None,
            },
            link: n.link.clone(),
        })
        .collect();
    // Merge parallel edges in order of first appearance.
    let mut order: Vec<(String, String)> = vec![];
    let mut merged: BTreeMap<(String, String), (Vec<&'static str>, u32, bool)> = BTreeMap::new();
    for e in &kept {
        let key = (e.from.clone(), e.to.clone());
        let entry = merged.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            (vec![], 0, false)
        });
        if !entry.0.contains(&e.kind.as_str()) {
            entry.0.push(e.kind.as_str());
        }
        entry.1 += e.count;
        entry.2 |= e.status != EdgeStatus::Verified || e.mapped > 0;
    }
    let edges = order
        .into_iter()
        .map(|key| {
            let (kinds, _, claimed) = &merged[&key];
            DiagramEdge {
                from: key.0,
                to: key.1,
                line: if *claimed {
                    LineKind::Claimed
                } else {
                    LineKind::Verified
                },
                label: {
                    let named: Vec<&str> = kinds
                        .iter()
                        .copied()
                        .filter(|k| !matches!(*k, "calls" | "references"))
                        .collect();
                    (!named.is_empty()).then(|| named.join(", "))
                },
            }
        })
        .collect();
    let mut notes = vec![];
    if hidden_tests > 0 {
        notes.push(format!(
            "{hidden_tests} test{} hidden",
            if hidden_tests == 1 { "" } else { "s" }
        ));
    }
    Diagram {
        nodes,
        edges,
        orientation,
        notes,
    }
}

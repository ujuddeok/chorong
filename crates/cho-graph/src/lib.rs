//! Reference graph engine: who uses a symbol and what it uses, rolled up into files and
//! packages. Pure functions over `cho-semantic` (SCIP) and `cho-syntax` (Tree-sitter)
//! data; no GUI. See `docs/public/graph.md`.
//!
//! Every edge comes from an index occurrence. What the index cannot see is stated, never
//! filled in by name matching.

mod build;
mod diagram;
mod dot;
mod html;
mod model;
mod packages;
mod svg;
mod views;

pub use build::{Decl, FileInfo, NodeRef, RawEdge, RefGraph};
pub use diagram::{
    Diagram, DiagramEdge, DiagramNode, EdgeRoute, FONT_PX, GroupBox, GroupBy, Layout, LineKind,
    MAX_LABEL_CHARS, NodeBox, NodeStyle, Orientation, Rect, cut_label, diagram_from_graph, layout,
    text_width,
};
pub use dot::to_dot;
pub use html::{Assets, PageOptions, assets_for_page, to_html};
#[cfg(test)]
pub(crate) use html::{assets_folder_name, install_shared_assets, shared_assets_for};
pub use model::{
    Edge, EdgeKind, EdgeStatus, Graph, LIMITS, Level, Meta, Node, NodeKind, Provenance, Rollup,
    Rollups, UnresolvedCall, ViewInfo, ViewKind,
};
pub use packages::{PackageInfo, PackageKind};
pub use svg::{arrow_base, diagram_vars, to_svg};
pub use views::{
    Changed, Direction, Hunk, ResolveError, ViewSpec, blast_radius, changed_decls, file_view,
    neighbourhood, outbound_closure, package_view, resolve_symbol,
};

#[cfg(test)]
mod diagram_tests;
#[cfg(test)]
mod tests;

//! Graphviz output: packages and files as nested clusters.

use crate::model::{EdgeKind, Graph, NodeKind};
use std::collections::BTreeMap;
use std::fmt::Write;

fn q(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

/// `s` for a `//` comment: a line break would end the comment and the rest of `s` would be
/// read as DOT, so every control character becomes a space.
fn comment(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// DOT for Graphviz and quick checks. Cyclic edges are red; `type-uses` edges are dashed.
pub fn to_dot(g: &Graph) -> String {
    let mut out = String::new();
    let m = &g.meta.provenance;
    if m.shared {
        let _ = writeln!(
            out,
            "{}",
            comment(&format!(
                "// {} | commit {} | index {} | exported for sharing",
                g.view.title,
                m.commit.as_deref().unwrap_or("-"),
                g.meta.index_status
            ))
        );
    } else {
        let _ = writeln!(
            out,
            "{}",
            comment(&format!(
                "// {} | worktree {} | branch {} | commit {} | dirty {} | index {}",
                g.view.title,
                m.worktree,
                m.branch.as_deref().unwrap_or("-"),
                m.commit.as_deref().unwrap_or("-"),
                m.dirty,
                g.meta.index_status
            ))
        );
    }
    let _ = writeln!(out, "// {}", g.meta.limits);
    out.push_str("digraph cho {\n  rankdir=LR;\n  compound=true;\n  node [shape=box, fontname=\"Helvetica\", fontsize=10];\n  edge [fontname=\"Helvetica\", fontsize=8];\n");
    let mut children: BTreeMap<&str, Vec<&crate::model::Node>> = BTreeMap::new();
    for n in &g.nodes {
        if let Some(p) = &n.parent {
            children.entry(p.as_str()).or_default().push(n);
        }
    }
    fn emit(
        out: &mut String,
        n: &crate::model::Node,
        children: &BTreeMap<&str, Vec<&crate::model::Node>>,
        depth: usize,
    ) {
        let pad = "  ".repeat(depth);
        match children.get(n.id.as_str()) {
            Some(kids) if matches!(n.kind, NodeKind::Package | NodeKind::File) => {
                let _ = writeln!(
                    out,
                    "{pad}subgraph {} {{\n{pad}  label={};",
                    q(&format!("cluster_{}", n.id)),
                    q(&n.label)
                );
                for k in kids {
                    emit(out, k, children, depth + 1);
                }
                let _ = writeln!(out, "{pad}}}");
            }
            _ => {
                let shape = match n.kind {
                    NodeKind::More => "ellipse",
                    NodeKind::External => "component",
                    _ => "box",
                };
                let style = if n.focus {
                    ", style=filled, fillcolor=lightyellow"
                } else {
                    ""
                };
                let tip = n
                    .link
                    .as_deref()
                    .map(|l| format!(", URL={}", q(l)))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "{pad}{} [label={}, shape={shape}{style}{tip}];",
                    q(&n.id),
                    q(&n.label)
                );
            }
        }
    }
    for n in g.nodes.iter().filter(|n| n.parent.is_none()) {
        emit(&mut out, n, &children, 1);
    }
    for e in &g.edges {
        let style = match e.kind {
            EdgeKind::TypeUses => "dashed",
            EdgeKind::Implements => "dotted",
            _ => "solid",
        };
        let color = if e.in_cycle { ", color=red" } else { "" };
        let _ = writeln!(
            out,
            "  {} -> {} [label={}, style={style}{color}];",
            q(&e.from),
            q(&e.to),
            q(&format!("{} x{}", e.kind.as_str(), e.count))
        );
    }
    out.push_str("}\n");
    out
}

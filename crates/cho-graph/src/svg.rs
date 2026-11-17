//! A laid-out diagram as SVG: real `<text>` at 12 px, colours as CSS custom properties from
//! the theme (both bundled themes by `prefers-color-scheme` when none is given). Byte-identical
//! for the same layout and theme.

use crate::diagram::{Diagram, FONT_PX, Layout, LineKind, NodeStyle};
use cho_theme::{Appearance, Rgba, Theme};
use std::fmt::Write;

/// The diagram colours from one theme, as `--name:value;` pairs
/// (`design/visual-design.md`, "Verification labels").
pub fn diagram_vars(t: &Theme) -> String {
    let ui = &t.ui;
    let accent = ui.text_accent;
    let syn = |k: &str| {
        t.syntax_style(k)
            .and_then(|s| s.color)
            .unwrap_or(t.editor_foreground)
    };
    let vars = [
        ("bg", t.editor_background),
        ("fg", t.editor_foreground),
        ("muted", t.text_muted),
        ("border", t.border),
        ("group-bg", ui.element_background),
        ("group-border", ui.border_variant),
        ("node-bg", t.editor_background),
        ("focus-bg", accent.with_alpha(46)),
        ("accent", accent),
        ("verified", ui.icon),
        ("claimed", ui.warning.unwrap_or(t.text_muted)),
        ("broken", ui.error.unwrap_or(t.text)),
        ("label-bg", t.editor_background.with_alpha(230)),
        ("kind-function", syn("function")),
        ("kind-type", syn("type")),
    ];
    let mut s = String::new();
    for (k, v) in vars {
        let _ = write!(s, "--{k}:{};", css(v));
    }
    s
}

fn css(c: Rgba) -> String {
    if c.a == 0xff {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("rgba({},{},{},{:.2})", c.r, c.g, c.b, c.a as f32 / 255.0)
    }
}

/// XML text and attribute escaping.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c if (c as u32) < 0x20 && c != '\t' => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

/// Which name colour a node's outline kind takes.
fn kind_class(kind: Option<&str>) -> &'static str {
    match kind {
        Some("function" | "method" | "constructor" | "macro") => "k-fn",
        Some("struct" | "class" | "enum" | "interface" | "trait" | "type" | "type-alias") => "k-ty",
        _ => "",
    }
}

/// The SVG for `layout` of `diagram`. `theme` fixes the colours; `None` carries both bundled
/// themes and follows the reader's light or dark preference.
pub fn to_svg(diagram: &Diagram, layout: &Layout, theme: Option<&Theme>) -> String {
    let mut s = String::new();
    let (w, h) = (layout.width.max(1), layout.height.max(1));
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" class="cho-diagram" role="img">"#
    );
    s.push_str("<style>\n");
    match theme {
        Some(t) => {
            let _ = writeln!(s, ".cho-diagram{{{}}}", diagram_vars(t));
        }
        None => {
            let _ = writeln!(
                s,
                ".cho-diagram{{{}}}\n@media (prefers-color-scheme: dark){{.cho-diagram{{{}}}}}",
                diagram_vars(&Theme::bundled(Appearance::Light)),
                diagram_vars(&Theme::bundled(Appearance::Dark))
            );
        }
    }
    let _ = writeln!(
        s,
        "\
.cho-diagram{{background:var(--bg);font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:{FONT_PX}px}}
.cho-diagram text{{fill:var(--fg);dominant-baseline:central}}
.group rect{{fill:var(--group-bg);stroke:var(--group-border)}}
.group text{{fill:var(--muted);font-family:system-ui,sans-serif}}
.node rect{{fill:var(--node-bg);stroke:var(--border)}}
.node.focus rect{{fill:var(--focus-bg);stroke:var(--accent)}}
.node.external rect,.node.more rect{{stroke-dasharray:4 3}}
.node.external text,.node.more text{{fill:var(--muted)}}
.node .k-fn{{fill:var(--kind-function)}}
.node .k-ty{{fill:var(--kind-type)}}
.edge{{fill:none;stroke-width:1.25}}
.edge.verified{{stroke:var(--verified)}}
.edge.claimed{{stroke:var(--claimed);stroke-dasharray:{c}}}
.edge.broken{{stroke:var(--broken);stroke-dasharray:{b}}}
.arrow.verified{{fill:var(--verified)}}
.arrow.claimed{{fill:var(--claimed)}}
.arrow.broken{{fill:var(--broken)}}
.elabel rect{{fill:var(--label-bg)}}
.elabel text{{fill:var(--muted)}}",
        c = dash_attr(LineKind::Claimed),
        b = dash_attr(LineKind::Broken)
    );
    s.push_str("</style>\n");
    let _ = writeln!(
        s,
        r#"<rect width="{w}" height="{h}" fill="var(--bg)" style="fill:var(--bg)"/>"#
    );

    for g in &layout.groups {
        let r = g.rect;
        let _ = writeln!(
            s,
            r#"<g class="group"><rect x="{}" y="{}" width="{}" height="{}" rx="6"/><text x="{}" y="{}">{}</text></g>"#,
            r.x,
            r.y,
            r.w,
            r.h,
            r.x + 8,
            r.y + 12,
            esc(&g.label)
        );
    }
    for e in &layout.edges {
        if e.points.len() < 2 {
            continue;
        }
        let class = e.line.as_str();
        let mut d = String::new();
        for (i, (x, y)) in e.points.iter().enumerate() {
            let _ = write!(d, "{}{x} {y}", if i == 0 { "M" } else { " L" });
        }
        let _ = writeln!(s, r#"<path class="edge {class}" d="{d}"/>"#);
        let [a, b] = arrow_base(&e.points);
        let tip = *e.points.last().unwrap_or(&(0, 0));
        let _ = writeln!(
            s,
            r#"<path class="arrow {class}" d="M{} {} L{} {} L{} {} Z"/>"#,
            tip.0, tip.1, a.0, a.1, b.0, b.1
        );
    }
    for e in &layout.edges {
        if let (Some(t), Some(r)) = (&e.label, e.label_rect) {
            let _ = writeln!(
                s,
                r#"<g class="elabel"><rect x="{}" y="{}" width="{}" height="{}" rx="3"/><text x="{}" y="{}" text-anchor="middle">{}</text></g>"#,
                r.x,
                r.y,
                r.w,
                r.h,
                r.x + r.w / 2,
                r.y + r.h / 2,
                esc(t)
            );
        }
    }
    for nb in &layout.nodes {
        let n = &diagram.nodes[nb.index];
        let style = match nb.style {
            NodeStyle::Normal => "",
            NodeStyle::Focus => " focus",
            NodeStyle::External => " external",
            NodeStyle::More => " more",
        };
        let r = nb.rect;
        let title = n
            .detail
            .as_deref()
            .map(|d| format!("<title>{}</title>", esc(d)));
        let body = format!(
            r#"<g class="node{style}" data-id="{}">{}<rect x="{}" y="{}" width="{}" height="{}" rx="5"/><text class="{}" x="{}" y="{}" text-anchor="middle">{}</text></g>"#,
            esc(&n.id),
            title.unwrap_or_default(),
            r.x,
            r.y,
            r.w,
            r.h,
            kind_class(n.kind.as_deref()),
            r.x + r.w / 2,
            r.y + r.h / 2,
            esc(&nb.label)
        );
        match &n.link {
            Some(link) => {
                let _ = writeln!(s, r#"<a href="{}">{body}</a>"#, esc(link));
            }
            None => {
                let _ = writeln!(s, "{body}");
            }
        }
    }
    s.push_str("</svg>\n");
    s
}

/// The two back corners of an arrowhead 8 px long at the end of `points`.
pub fn arrow_base(points: &[(i32, i32)]) -> [(i32, i32); 2] {
    let n = points.len();
    if n < 2 {
        return [(0, 0), (0, 0)];
    }
    let (tip, prev) = (points[n - 1], points[n - 2]);
    let (dx, dy) = ((tip.0 - prev.0).signum(), (tip.1 - prev.1).signum());
    let (bx, by) = (tip.0 - dx * 8, tip.1 - dy * 8);
    // Routes are orthogonal, so the direction is along one axis.
    [(bx - dy * 4, by - dx * 4), (bx + dy * 4, by + dx * 4)]
}

fn dash_attr(l: LineKind) -> String {
    l.dash()
        .map(|[a, b]| format!("{a} {b}"))
        .unwrap_or_default()
}

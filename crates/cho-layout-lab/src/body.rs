//! Placeholder pane contents and each kind's header icon and qualifier. Shapes and colours
//! follow design/visual-design.md ("Colour by element"); the real panes come from the viewer at
//! integration.

use cho_layout::PaneContent;
use cho_layout_ui::colors::{Colors, kind_glyph};
use cho_layout_ui::widgets::{Verification, chip, dot, small_icon};
use gpui::{
    AnyElement, FontWeight, Hsla, IntoElement, ParentElement, SharedString, Styled, div,
    prelude::FluentBuilder, px,
};

/// The short muted text after a pane's title (visual-design.md: counts and status).
pub fn qualifier(content: &PaneContent) -> String {
    match content {
        PaneContent::FileTree => "cho".into(),
        PaneContent::Tour { .. } => "3/6".into(),
        PaneContent::Context => "hovering".into(),
        PaneContent::PinnedContext(p) => format!("{} · {}", p.symbol_kind, p.commit),
        PaneContent::Outline => "33".into(),
        PaneContent::CallStack { .. } => "5 frames".into(),
        PaneContent::Comparison { .. } => "2/5".into(),
        PaneContent::Graph { query, .. } => query.clone(),
        PaneContent::Editor { path, .. } => path
            .rsplit_once('/')
            .map(|(dir, _)| dir.to_string())
            .unwrap_or_default(),
        PaneContent::Search => String::new(),
        PaneContent::Changes => "3 files".into(),
        PaneContent::Highlights => "2".into(),
        PaneContent::Placeholder { .. } => String::new(),
    }
}

fn text(s: &str) -> SharedString {
    SharedString::from(s.to_string())
}

/// A list row: indent, glyph in its colour, label, then muted meta at the right.
fn row(
    c: &Colors,
    indent: f32,
    glyph: &str,
    glyph_color: Hsla,
    label: &str,
    meta: &str,
) -> gpui::Div {
    div()
        .h(px(22.))
        .flex()
        .items_center()
        .gap(px(6.))
        .pl(px(8. + indent))
        .pr(px(8.))
        .child(small_icon(glyph, glyph_color))
        .child(div().text_color(c.text).child(text(label)))
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(11.))
                .text_color(c.text_muted)
                .child(text(meta)),
        )
}

/// A declaration row: the kind's glyph in the kind's syntax colour.
fn kind_row(c: &Colors, indent: f32, kind: &str, name: &str, meta: &str) -> gpui::Div {
    row(c, indent, kind_glyph(kind), c.kind(kind), name, meta)
}

fn pill(c: &Colors, n: &str) -> AnyElement {
    div()
        .px(px(6.))
        .h(px(16.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .bg(c.element_bg)
        .text_size(px(11.))
        .text_color(c.text_muted)
        .child(text(n))
        .into_any_element()
}

fn section(c: &Colors, title: &str, qualifier: &str) -> AnyElement {
    div()
        .h(px(22.))
        .mt(px(8.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(c.text_muted)
        .child(small_icon("chevron-down", c.icon_muted))
        .child(text(title))
        .child(div().font_weight(FontWeight::NORMAL).child(text(qualifier)))
        .into_any_element()
}

/// A tour stop's state, shown by its number badge.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Done,
    Current,
    ToDo,
    Broken,
}

fn stop_badge(c: &Colors, n: usize, s: Stop) -> AnyElement {
    let d = div()
        .size(px(18.))
        .flex_none()
        .flex()
        .justify_center()
        .items_center()
        .rounded(px(9.))
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text(&n.to_string()));
    match s {
        Stop::Done => {
            let col = c.success.unwrap_or(c.text_muted);
            d.border_1().border_color(col).text_color(col)
        }
        Stop::Current => d.bg(c.text_accent).text_color(c.editor),
        Stop::ToDo => d.border_1().border_color(c.border).text_color(c.text_muted),
        Stop::Broken => {
            let col = c.error.unwrap_or(c.text_muted);
            d.border_1().border_color(col).text_color(col)
        }
    }
    .into_any_element()
}

fn tour_row(c: &Colors, n: usize, s: Stop, kind: &str, name: &str, at: &str) -> AnyElement {
    let current = s == Stop::Current;
    let broken = s == Stop::Broken;
    div()
        .relative()
        .h(px(24.))
        .mx(px(4.))
        .rounded(px(4.))
        .flex()
        .items_center()
        .gap(px(6.))
        .pl(px(6.))
        .pr(px(6.))
        .when(current, |d| {
            d.bg(c.selected).child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(4.))
                    .bottom(px(4.))
                    .w(px(2.))
                    .bg(c.text_accent),
            )
        })
        .child(stop_badge(c, n, s))
        .child(small_icon(
            kind_glyph(kind),
            if broken { c.text_muted } else { c.kind(kind) },
        ))
        .child(
            div()
                .text_color(if s == Stop::Done {
                    c.text_muted
                } else {
                    c.text
                })
                .when(current, |d| d.font_weight(FontWeight::MEDIUM))
                .child(text(name)),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(11.))
                .text_color(if broken {
                    c.error.unwrap_or(c.text_muted)
                } else {
                    c.text_muted
                })
                .when(broken, |d| d.line_through())
                .child(text(at)),
        )
        .into_any_element()
}

fn frame_row(
    c: &Colors,
    i: usize,
    v: Verification,
    kind: &str,
    name: &str,
    at: &str,
) -> AnyElement {
    let broken = v == Verification::Broken;
    div()
        .h(px(24.))
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .child(
            div()
                .w(px(14.))
                .text_size(px(11.))
                .text_color(c.text_muted)
                .child(text(&i.to_string())),
        )
        .child(dot(c, v))
        .child(small_icon(
            kind_glyph(kind),
            if broken { c.text_muted } else { c.kind(kind) },
        ))
        .child(
            div()
                .text_color(if broken { c.text_muted } else { c.text })
                .when(broken, |d| d.line_through())
                .when(v == Verification::Claimed, |d| {
                    // GPUI 0.2.2 has no dashed text underline; wavy keeps the shape distinct.
                    d.text_decoration_1()
                        .text_decoration_wavy()
                        .text_decoration_color(c.warning.unwrap_or(c.text_muted))
                })
                .child(text(name)),
        )
        .child(div().flex_1())
        .child(
            div()
                .text_size(px(11.))
                .text_color(if broken {
                    c.error.unwrap_or(c.text_muted)
                } else {
                    c.text_muted
                })
                .child(text(at)),
        )
        .child(chip(c, v))
        .into_any_element()
}

fn code_line(c: &Colors, n: usize, parts: &[(&str, Hsla)]) -> AnyElement {
    div()
        .h(px(19.))
        .flex()
        .items_center()
        .font_family(c.mono_font.clone())
        .whitespace_nowrap()
        .text_size(px(12.))
        .child(
            div()
                .w(px(40.))
                .pr(px(10.))
                .flex()
                .justify_end()
                .text_color(c.line_number)
                .child(text(&n.to_string())),
        )
        .children(
            parts
                .iter()
                .map(|(t, col)| div().flex_none().text_color(*col).child(text(t))),
        )
        .into_any_element()
}

/// The context pane's declaration card: editor background, `border.variant` edge.
fn card(c: &Colors, kind: &str, prefix: &str, name: &str, meta: &str) -> AnyElement {
    div()
        .mx(px(8.))
        .p(px(6.))
        .rounded(px(4.))
        .bg(c.editor)
        .border_1()
        .border_color(c.border_variant)
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(small_icon(kind_glyph(kind), c.kind(kind)))
                .child(div().text_color(c.text_muted).child(text(prefix)))
                .child(
                    div()
                        .text_color(c.text)
                        .font_weight(FontWeight::MEDIUM)
                        .child(text(name)),
                ),
        )
        .child(
            div()
                .pl(px(20.))
                .text_size(px(11.))
                .text_color(c.text_muted)
                .child(text(meta)),
        )
        .into_any_element()
}

/// Placeholder contents for a pane.
pub fn render(content: &PaneContent, c: &Colors) -> AnyElement {
    let col = div().flex().flex_col().py(px(4.)).text_size(px(12.5));
    let file = |indent: f32, name: &str| row(c, indent, "file-code", c.icon, name, "");
    let folder = |indent: f32, name: &str| row(c, indent, "folder", c.icon, name, "");
    match content {
        PaneContent::FileTree => col
            .child(folder(0., "app"))
            .child(folder(0., "crates"))
            .child(folder(12., "cho-core"))
            .child(file(24., "buffer.rs"))
            .child(file(24., "line.rs"))
            .child(folder(12., "cho-layout"))
            .child(row(c, 0., "file-text", c.icon, "README.md", ""))
            .into_any_element(),
        PaneContent::Tour { .. } => col
            .child(tour_row(
                c,
                1,
                Stop::Done,
                "module",
                "Open a file",
                "main.rs:41",
            ))
            .child(tour_row(
                c,
                2,
                Stop::Done,
                "method",
                "Point at a symbol",
                "pointer.rs:12",
            ))
            .child(tour_row(
                c,
                3,
                Stop::Current,
                "method",
                "utf16_to_byte",
                "buffer.rs:168",
            ))
            .child(tour_row(c, 4, Stop::ToDo, "struct", "LineCol", "line.rs:9"))
            .child(tour_row(
                c,
                5,
                Stop::Broken,
                "function",
                "old_offsets",
                "buffer.rs:301",
            ))
            .into_any_element(),
        PaneContent::Context => col
            .child(section(c, "Declaration", "· you are in"))
            .child(card(
                c,
                "method",
                "SourceBuffer::",
                "utf16_to_byte",
                "method · lines 168–179",
            ))
            .child(section(c, "Symbol", "· under the pointer"))
            .child(kind_row(c, 0., "struct", "LineCol", "struct"))
            .child(section(c, "References", "14 in 3 files"))
            .child(row(c, 12., "file-code", c.icon, "buffer.rs", "").child(pill(c, "6")))
            .child(row(c, 12., "file-code", c.icon, "line.rs", "").child(pill(c, "5")))
            .into_any_element(),
        PaneContent::PinnedContext(p) => col
            .child(section(c, "Declaration", ""))
            .child(card(c, &p.symbol_kind, "", &p.symbol, &p.symbol_kind))
            .child(section(c, "Pinned at", ""))
            .child(row(
                c,
                0.,
                "file-code",
                c.icon,
                &format!("{} @ byte {}", p.path, p.byte_offset),
                &p.commit,
            ))
            .into_any_element(),
        PaneContent::Outline => col
            .child(kind_row(c, 0., "struct", "SourceBuffer", "120"))
            .child(kind_row(c, 14., "function", "new", "12"))
            .child(kind_row(c, 14., "method", "utf16_to_byte", "11"))
            .child(kind_row(c, 14., "method", "line_col", "9"))
            .child(kind_row(c, 0., "enum", "Encoding", "8"))
            .child(kind_row(c, 0., "impl", "Display for LineCol", "6"))
            .child(kind_row(c, 0., "constant", "TAB_WIDTH", "1"))
            .child(kind_row(c, 0., "macro", "offset!", "4"))
            .into_any_element(),
        PaneContent::Comparison { .. } => col
            .child(row(c, 0., "list", c.icon, "Receive the request", "differs"))
            .child(row(c, 0., "list", c.icon, "Validate the order", "same"))
            .into_any_element(),
        PaneContent::CallStack { .. } => col
            .child(frame_row(
                c,
                0,
                Verification::Verified,
                "method",
                "view::Chorong::on_mouse_move",
                "view.rs:412",
            ))
            .child(frame_row(
                c,
                1,
                Verification::Verified,
                "function",
                "pointer::byte_at",
                "pointer.rs:12",
            ))
            .child(frame_row(
                c,
                2,
                Verification::Claimed,
                "method",
                "SourceBuffer::utf16_to_byte",
                "buffer.rs:168",
            ))
            .child(frame_row(
                c,
                3,
                Verification::Broken,
                "function",
                "LineCol::from_byte",
                "anchor not found at 04e63f8",
            ))
            .into_any_element(),
        PaneContent::Graph { query, .. } => col
            .child(kind_row(c, 0., "struct", query, "12 nodes · 18 edges"))
            .child(
                kind_row(c, 12., "method", "SourceBuffer::line_col", "inbound")
                    .child(chip(c, Verification::Verified)),
            )
            .child(
                kind_row(c, 12., "method", "Outline::entries", "inbound")
                    .child(chip(c, Verification::Claimed)),
            )
            .into_any_element(),
        PaneContent::Editor { .. } => col
            .pt(px(6.))
            .child(code_line(
                c,
                166,
                &[
                    ("impl ", c.syntax_keyword),
                    ("SourceBuffer", c.syntax_type),
                    (" {", c.text),
                ],
            ))
            .child(code_line(
                c,
                167,
                &[("    /// UTF-16 column to byte offset.", c.syntax_comment)],
            ))
            .child(code_line(
                c,
                168,
                &[
                    ("    pub fn ", c.syntax_keyword),
                    ("utf16_to_byte", c.syntax_function),
                    ("(&self, at: LineCol) -> usize {", c.text),
                ],
            ))
            .child(code_line(
                c,
                169,
                &[("        let line = self.line(at.line);", c.text)],
            ))
            .child(code_line(
                c,
                170,
                &[("        line.byte_at_utf16(at.col)", c.text)],
            ))
            .child(code_line(c, 171, &[("    }", c.text)]))
            .child(code_line(c, 172, &[("}", c.text)]))
            .into_any_element(),
        PaneContent::Search => col
            .child(row(c, 0., "search", c.icon, "utf16", "").child(pill(c, "3")))
            .into_any_element(),
        PaneContent::Changes => col
            .child(row(c, 0., "file-code", c.icon, "src/buffer.rs", "").child(pill(c, "2")))
            .child(row(
                c,
                1.,
                "square-function",
                c.icon,
                "~ SourceBuffer::line",
                "",
            ))
            .child(row(
                c,
                1.,
                "square-function",
                c.icon,
                "+ SourceBuffer::utf16_to_byte",
                "",
            ))
            .into_any_element(),
        PaneContent::Highlights => col
            .child(row(c, 0., "highlighter", c.icon, "byte_at_utf16", "").child(pill(c, "4")))
            .child(row(c, 0., "highlighter", c.icon, "line", "").child(pill(c, "7")))
            .into_any_element(),
        PaneContent::Placeholder { name } => col
            .child(row(c, 0., "square-dashed", c.icon, name, ""))
            .into_any_element(),
    }
}

//! The outline pane: the current file's declarations, each with its kind glyph, name and line
//! count, indented by scope (docs/public/visual-design.md, "Kind marks", "Row states").

use super::*;
use cho_layout_ui::colors::kind_glyph;
use cho_layout_ui::widgets::small_icon;
use cho_syntax::SymbolKind;

const ROW_H: f32 = 22.;
const INSET: f32 = 4.;
const PAD: f32 = 6.;
const INDENT: f32 = 14.;
const GLYPH: f32 = 14.;

/// Containers are drawn Medium (visual-design.md, "Kind marks").
pub(crate) fn is_container(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Struct | SymbolKind::Class | SymbolKind::Impl | SymbolKind::Interface
    )
}

/// The outline entry holding `caret`: the innermost declaration whose range contains it.
/// Outline entries and `symbols` share an index.
pub(crate) fn current_entry(symbols: &[cho_syntax::Symbol], caret: usize) -> Option<usize> {
    symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.range.contains(&caret))
        .max_by_key(|(_, s)| (s.scope.len(), std::cmp::Reverse(s.range.len())))
        .map(|(i, _)| i)
}

/// The guide level and rows of `current`'s parent scope: the entries after the parent down to
/// the next entry at the parent's depth or shallower. None at the top level.
fn active_scope(
    outline: &[crate::document::OutlineEntry],
    current: usize,
) -> Option<(usize, std::ops::Range<usize>)> {
    let depth = outline.get(current)?.depth;
    let level = depth.checked_sub(1)?;
    let parent = outline[..current].iter().rposition(|e| e.depth == level)?;
    let end = outline[parent + 1..]
        .iter()
        .position(|e| e.depth <= level)
        .map_or(outline.len(), |i| parent + 1 + i);
    Some((level, parent + 1..end))
}

impl Chorong {
    pub(super) fn render_outline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let count = self.ed().doc.as_ref().map_or(0, |d| d.outline.len());
        let current = self
            .ed()
            .doc
            .as_ref()
            .zip(self.ed().caret)
            .and_then(|(d, caret)| current_entry(&d.symbols, caret));
        let c = crate::pane_style::colors(&self.theme, cx);
        let guide_col = hsla(t.ui.panel_indent_guide);
        let guide_active = hsla(t.ui.panel_indent_guide_active);
        // The current entry's parent scope: its guide is drawn active down that scope's rows,
        // as the tree does for the selected file's folder.
        let active_guide = self
            .ed()
            .doc
            .as_ref()
            .zip(current)
            .and_then(|(d, cur)| active_scope(&d.outline, cur));
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .pt_1()
            .text_size(px(12.5))
            .when(count == 0, |d| {
                d.child(
                    div()
                        .p_3()
                        .text_color(c.text_muted)
                        .child("No declarations"),
                )
            })
            .child(
                crate::scroll::uniform_list(
                    "outline",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        let Some(doc) = &this.ed().doc else {
                            return Vec::new();
                        };
                        range
                            .map(|ix| {
                                let e = &doc.outline[ix];
                                let byte = e.byte;
                                let word = e.kind.as_str();
                                let is_current = current == Some(ix);
                                let medium = is_current || is_container(e.kind);
                                let guides = (0..e.depth).map(|level| {
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left(px(PAD + INDENT * level as f32 + GLYPH / 2.))
                                        .w(px(1.))
                                        .bg(match &active_guide {
                                            Some((l, rows))
                                                if *l == level && rows.contains(&ix) =>
                                            {
                                                guide_active
                                            }
                                            _ => guide_col,
                                        })
                                });
                                let row = div()
                                    .id(("outline", ix))
                                    .relative()
                                    .w_full()
                                    .h(px(ROW_H))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .pl(px(PAD + INDENT * e.depth as f32))
                                    .pr_2()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    // Current: the selected fill and an accent bar.
                                    .when(is_current, |d| d.bg(c.selected))
                                    .when(!is_current, |d| d.hover(|s| s.bg(c.hover)))
                                    .active(|s| s.bg(c.active))
                                    .children(guides)
                                    // A 2 px accent bar at the current row's left edge.
                                    .when(is_current, |d| {
                                        d.child(
                                            div()
                                                .absolute()
                                                .left_0()
                                                .top(px(3.))
                                                .bottom(px(3.))
                                                .w(px(2.))
                                                .rounded(px(1.))
                                                .bg(c.text_accent),
                                        )
                                    })
                                    .child(
                                        div()
                                            .mr(px(6.))
                                            .flex()
                                            .child(small_icon(kind_glyph(word), c.kind(word))),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .when(medium, |d| d.font_weight(FontWeight::MEDIUM))
                                            .child(e.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .pl_2()
                                            .text_size(px(11.))
                                            .text_color(c.text_muted)
                                            .child(e.line_count.to_string()),
                                    )
                                    .tooltip(tip(format!(
                                        "{word} \u{b7} {} line{}",
                                        e.line_count,
                                        if e.line_count == 1 { "" } else { "s" }
                                    )))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.jump_to_byte(byte, cx);
                                        cx.notify();
                                    }));
                                div().w_full().px(px(INSET)).child(row)
                            })
                            .collect()
                    }),
                )
                .wrap(
                    &self.outline_over,
                    &self.outline_scroll,
                    self.features.probe_if_on(Feature::Overscroll),
                )
                .w_full()
                .flex_1(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_syntax::Symbol;

    fn sym(name: &str, scope: &[&str], range: std::ops::Range<usize>) -> Symbol {
        Symbol {
            name: name.into(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            kind: SymbolKind::Function,
            name_range: range.start..range.start + 1,
            range,
        }
    }

    #[test]
    fn current_entry_is_the_innermost_declaration_holding_the_caret() {
        let symbols = [
            sym("Buf", &[], 0..100),
            sym("new", &["Buf"], 10..30),
            sym("len", &["Buf"], 40..60),
            sym("free", &[], 120..140),
        ];
        assert_eq!(current_entry(&symbols, 15), Some(1));
        assert_eq!(current_entry(&symbols, 35), Some(0));
        assert_eq!(current_entry(&symbols, 130), Some(3));
        assert_eq!(current_entry(&symbols, 110), None);
    }

    #[test]
    fn active_scope_is_the_current_entrys_parent() {
        let e = |depth| crate::document::OutlineEntry {
            name: String::new(),
            kind: SymbolKind::Function,
            depth,
            byte: 0,
            line_count: 1,
        };
        // a, A{ b, c{ d }, e }, f
        let o = [e(0), e(0), e(1), e(1), e(2), e(1), e(0)];
        assert_eq!(active_scope(&o, 0), None);
        assert_eq!(active_scope(&o, 3), Some((0, 2..6)));
        assert_eq!(active_scope(&o, 4), Some((1, 4..5)));
        assert_eq!(active_scope(&o, 5), Some((0, 2..6)));
    }
}

//! Every colour the lab draws, resolved from a `cho-theme` theme (Zed theme keys, with the
//! fallbacks of docs/public/visual-design.md). Nothing here is a literal colour.

use cho_theme::{Rgba, Theme};
use gpui::Hsla;

pub fn hsla(c: Rgba) -> Hsla {
    gpui::Rgba {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: f32::from(c.a) / 255.0,
    }
    .into()
}

#[derive(Clone)]
pub struct Colors {
    pub name: String,
    /// Installed UI and code font families (see `fonts.rs`). Not colours, but travel with them
    /// to every view that draws text.
    pub ui_font: gpui::SharedString,
    pub mono_font: gpui::SharedString,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_accent: Hsla,
    pub border: Hsla,
    pub border_variant: Hsla,
    pub panel: Hsla,
    pub editor: Hsla,
    pub header: Hsla,
    pub status_bar: Hsla,
    pub elevated: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub selected: Hsla,
    pub icon: Hsla,
    pub icon_muted: Hsla,
    pub line_number: Hsla,
    /// `text.accent` at 16%: chips and drop zones.
    pub accent_wash: Hsla,
    /// `text.accent` at 7%: a pinned pane's header.
    pub accent_faint: Hsla,
    /// `text.accent` at 10%: the focus bar.
    pub accent_bar: Hsla,
    pub success: Option<Hsla>,
    pub warning: Option<Hsla>,
    pub error: Option<Hsla>,
    pub success_bg: Option<Hsla>,
    pub warning_bg: Option<Hsla>,
    pub error_bg: Option<Hsla>,
    // design/visual-design.md, "Colour by element".
    pub icon_accent: Hsla,
    pub icon_disabled: Hsla,
    pub element_bg: Hsla,
    pub ghost_active: Hsla,
    pub ghost_selected: Hsla,
    pub pane_focus: Hsla,
    pub tab_bar: Hsla,
    pub tab_active: Hsla,
    pub tab_inactive: Hsla,
    pub drop_target: Hsla,
    /// Kind-mark colours (visual-design.md, "Kind marks"): the kind's syntax key, then its
    /// parent, then `text.muted`.
    pub kind_function: Hsla,
    pub kind_type: Hsla,
    pub kind_enum: Hsla,
    pub kind_keyword: Hsla,
    pub kind_namespace: Hsla,
    pub kind_constant: Hsla,
    pub kind_field: Hsla,
    pub kind_macro: Hsla,
    pub syntax_keyword: Hsla,
    pub syntax_function: Hsla,
    pub syntax_type: Hsla,
    pub syntax_comment: Hsla,
}

impl Colors {
    pub fn new(t: &Theme, fonts: crate::fonts::Fonts) -> Self {
        let ui = &t.ui;
        let syntax = |k: &str| {
            t.syntax_style(k)
                .and_then(|s| s.color)
                .map_or(hsla(t.editor_foreground), hsla)
        };
        // The first key that resolves (syntax_style already falls back to parents), else
        // `text.muted`.
        let kind = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| t.syntax_style(k).and_then(|s| s.color))
                .map_or(hsla(t.text_muted), hsla)
        };
        Self {
            name: t.name.clone(),
            ui_font: fonts.ui,
            mono_font: fonts.mono,
            text: hsla(t.text),
            text_muted: hsla(t.text_muted),
            text_accent: hsla(ui.text_accent),
            border: hsla(t.border),
            border_variant: hsla(ui.border_variant),
            panel: hsla(t.panel_background),
            editor: hsla(t.editor_background),
            header: hsla(t.title_bar_background),
            status_bar: hsla(t.status_bar_background),
            elevated: hsla(ui.elevated_surface_background),
            hover: hsla(ui.ghost_element_hover),
            active: hsla(ui.element_active),
            selected: hsla(t.element_selected),
            icon: hsla(ui.icon),
            icon_muted: hsla(ui.icon_muted),
            line_number: hsla(t.editor_line_number),
            accent_wash: hsla(ui.text_accent.with_alpha(0x29)),
            accent_faint: hsla(ui.text_accent.with_alpha(0x12)),
            accent_bar: hsla(ui.text_accent.with_alpha(0x1a)),
            success: ui.success.map(hsla),
            warning: ui.warning.map(hsla),
            error: ui.error.map(hsla),
            success_bg: ui.success_background.map(hsla),
            warning_bg: ui.warning_background.map(hsla),
            error_bg: ui.error_background.map(hsla),
            icon_accent: hsla(ui.icon_accent),
            icon_disabled: hsla(ui.icon_disabled),
            element_bg: hsla(ui.element_background),
            ghost_active: hsla(ui.ghost_element_active),
            ghost_selected: hsla(ui.ghost_element_selected),
            pane_focus: hsla(ui.pane_focused_border),
            tab_bar: hsla(ui.tab_bar_background),
            tab_active: hsla(ui.tab_active_background),
            tab_inactive: hsla(ui.tab_inactive_background),
            drop_target: hsla(ui.drop_target_background),
            kind_function: kind(&["function"]),
            kind_type: kind(&["type"]),
            kind_enum: kind(&["enum", "type"]),
            kind_keyword: kind(&["keyword"]),
            kind_namespace: kind(&["namespace", "type"]),
            kind_constant: kind(&["constant"]),
            kind_field: kind(&["property", "variable"]),
            kind_macro: kind(&["function.macro"]),
            syntax_keyword: syntax("keyword"),
            syntax_function: syntax("function"),
            syntax_type: syntax("type"),
            syntax_comment: syntax("comment"),
        }
    }
}

impl Colors {
    /// A declaration kind's glyph colour, from its kind word.
    pub fn kind(&self, word: &str) -> Hsla {
        match word {
            "function" | "method" => self.kind_function,
            "struct" | "class" | "interface" | "trait" | "protocol" | "type" => self.kind_type,
            "enum" => self.kind_enum,
            "impl" => self.kind_keyword,
            "module" => self.kind_namespace,
            "constant" => self.kind_constant,
            "field" => self.kind_field,
            "macro" => self.kind_macro,
            _ => self.text_muted,
        }
    }
}

/// A declaration kind's Lucide glyph (visual-design.md, "Kind marks").
pub fn kind_glyph(word: &str) -> &'static str {
    match word {
        "function" | "method" => "square-function",
        "struct" | "class" => "box",
        "type" => "type",
        "interface" | "trait" | "protocol" => "component",
        "enum" => "list",
        "impl" => "layers",
        "module" => "package",
        "constant" => "hash",
        "field" => "variable",
        "macro" => "zap",
        _ => "square-dashed",
    }
}

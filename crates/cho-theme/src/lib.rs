//! Zed theme loading.
//!
//! Chorong reads Zed's theme file format directly (schema v0.2.0), so any Zed
//! theme can be used. Only the keys Chorong draws with are kept; unknown keys are
//! ignored, and missing keys fall back to sensible derived values.

use serde::Deserialize;
use std::collections::HashMap;
use thiserror::Error;

/// The bundled default family: original Chorong Dark and Chorong Light themes.
pub const BUNDLED_FAMILY: &str = include_str!("../themes/chorong.json");

#[derive(Debug, Error)]
pub enum ThemeError {
    #[error("theme file is not valid JSON for the Zed theme schema: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid colour `{value}` for `{key}`")]
    Colour { key: String, value: String },
    #[error("theme family `{family}` has no theme named `{name}` (available: {available})")]
    NotFound {
        family: String,
        name: String,
        available: String,
    },
    #[error("theme family `{0}` contains no themes")]
    Empty(String),
}

/// An sRGB colour with straight alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(hex: u32) -> Self {
        Self {
            r: (hex >> 16) as u8,
            g: (hex >> 8) as u8,
            b: hex as u8,
            a: 0xff,
        }
    }

    /// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let nibble = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok().map(|v| v * 17);
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(match hex.len() {
            3 => Self {
                r: nibble(0)?,
                g: nibble(1)?,
                b: nibble(2)?,
                a: 0xff,
            },
            4 => Self {
                r: nibble(0)?,
                g: nibble(1)?,
                b: nibble(2)?,
                a: nibble(3)?,
            },
            6 => Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: 0xff,
            },
            8 => Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            },
            _ => return None,
        })
    }

    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// The colour as `0xRRGGBBAA`.
    pub fn to_u32(self) -> u32 {
        u32::from_be_bytes([self.r, self.g, self.b, self.a])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// How to draw one syntax capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyntaxStyle {
    pub color: Option<Rgba>,
    pub font_style: Option<FontStyle>,
    /// CSS-style weight, e.g. 400 regular, 700 bold.
    pub font_weight: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// A colour set for one collaborator in Zed. Chorong uses these as a palette for
/// distinct highlight groups (pinned occurrences, caller pills).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Player {
    pub cursor: Rgba,
    pub background: Rgba,
    pub selection: Rgba,
}

/// The colours Chorong uses, resolved from one Zed theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub appearance: Appearance,
    pub background: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub panel_background: Rgba,
    pub status_bar_background: Rgba,
    pub title_bar_background: Rgba,
    pub element_selected: Rgba,
    pub editor_background: Rgba,
    pub editor_foreground: Rgba,
    pub editor_gutter_background: Rgba,
    pub editor_line_number: Rgba,
    pub editor_active_line_number: Rgba,
    pub editor_active_line_background: Rgba,
    pub players: Vec<Player>,
    /// Chrome colours from docs/public/visual-design.md ("Theme keys"), each with its fallback.
    pub ui: UiColors,
    syntax: HashMap<String, SyntaxStyle>,
}

/// The pane, row and overlay colours of docs/public/visual-design.md. Every field is resolved: a theme
/// that leaves a key out gets the documented fallback. `success`, `warning` and `error` have no
/// fallback (the text still says what it means), so they stay optional.
#[derive(Debug, Clone, PartialEq)]
pub struct UiColors {
    /// `border.variant`: rules and card edges inside a pane. Fallback: `border` at half alpha.
    pub border_variant: Rgba,
    /// `border.focused`. Fallback: `players[0].cursor`.
    pub border_focused: Rgba,
    /// `element.hover`. Fallback: `text` at 6%.
    pub element_hover: Rgba,
    /// `ghost_element.hover`: hovered rows and ghost buttons. Fallback: `element.hover`.
    pub ghost_element_hover: Rgba,
    /// `element.active`: pressed rows. Fallback: `element.selected`.
    pub element_active: Rgba,
    /// `panel.focused_border`. Fallback: `border.focused`.
    pub panel_focused_border: Rgba,
    /// `panel.indent_guide`. Fallback: `border.variant`.
    pub panel_indent_guide: Rgba,
    /// `panel.indent_guide_active`. Fallback: `border`.
    pub panel_indent_guide_active: Rgba,
    /// `icon`. Fallback: `text.muted`.
    pub icon: Rgba,
    /// `icon.muted`. Fallback: `text.muted`.
    pub icon_muted: Rgba,
    /// `text.accent`: current marker, pinned and focus accents. Fallback: `players[0].cursor`.
    pub text_accent: Rgba,
    /// `elevated_surface.background`: overlays. Fallback: `panel.background`.
    pub elevated_surface_background: Rgba,
    /// `toolbar.background`: the editor header. Fallback: `editor.background`.
    pub toolbar_background: Rgba,
    /// `search.match_background`. Fallback: `players[0].cursor` at 28%.
    pub search_match_background: Rgba,
    /// `success`, else `created`. No colour fallback: the text still says what it means.
    pub success: Option<Rgba>,
    /// `warning`, else `modified`.
    pub warning: Option<Rgba>,
    /// `error`, else `deleted`.
    pub error: Option<Rgba>,
    // design/visual-design.md, "Colour by element", "Additional keys".
    /// `icon.accent`: focused pane, pins, focus mode, active tool tab. Fallback: `text.accent`.
    pub icon_accent: Rgba,
    /// `icon.disabled`. Fallback: `text.muted` at 50%.
    pub icon_disabled: Rgba,
    /// `text.disabled`. Fallback: `text.muted` at 50%.
    pub text_disabled: Rgba,
    /// `element.background`: bordered buttons, key chips, count pills. Fallback: `text` at 8%.
    pub element_background: Rgba,
    /// `ghost_element.active`: pressed ghost buttons. Fallback: `element.active`.
    pub ghost_element_active: Rgba,
    /// `ghost_element.selected`: toggled-on ghost buttons. Fallback: `element.selected`.
    pub ghost_element_selected: Rgba,
    /// `pane.focused_border`: the focused pane's outline. Fallback: `panel.focused_border`.
    pub pane_focused_border: Rgba,
    /// `tab_bar.background`. Fallback: `title_bar.background`.
    pub tab_bar_background: Rgba,
    /// `tab.active_background`. Fallback: `editor.background`.
    pub tab_active_background: Rgba,
    /// `tab.inactive_background`. Fallback: `tab_bar.background`.
    pub tab_inactive_background: Rgba,
    /// `drop_target.background`. Fallback: `text.accent` at 12%.
    pub drop_target_background: Rgba,
    /// `editor.highlighted_line.background`: lines a tour stop or frame points at. Fallback:
    /// `text.accent` at 12%.
    pub editor_highlighted_line_background: Rgba,
    /// `created`, else `success`: lines a change added (the comparison's marks in the code).
    pub created: Option<Rgba>,
    /// `modified`, else `warning`: uncommitted work in a comparison.
    pub modified: Option<Rgba>,
    /// `deleted`, else `error`: lines a change removed.
    pub deleted: Option<Rgba>,
    /// `success.background` etc.: status chips and notices. Fallback: the colour at 14%.
    pub success_background: Option<Rgba>,
    pub warning_background: Option<Rgba>,
    pub error_background: Option<Rgba>,
    /// `success.border` etc.: notice borders. Fallback: the colour at 50%.
    pub success_border: Option<Rgba>,
    pub warning_border: Option<Rgba>,
    pub error_border: Option<Rgba>,
}

impl Theme {
    /// The style for a capture name such as `function.method`, falling back
    /// to its parents (`function`) when the theme has no exact entry.
    pub fn syntax_style(&self, capture: &str) -> Option<SyntaxStyle> {
        let mut name = capture;
        loop {
            if let Some(style) = self.syntax.get(name) {
                return Some(*style);
            }
            name = &name[..name.rfind('.')?];
        }
    }

    /// The bundled theme matching `appearance`.
    pub fn bundled(appearance: Appearance) -> Self {
        let family = ThemeFamily::parse(BUNDLED_FAMILY).expect("bundled theme is valid");
        family
            .themes
            .into_iter()
            .find(|t| t.appearance == appearance)
            .expect("bundled family has light and dark themes")
    }
}

/// A theme file: a named family of one or more themes.
#[derive(Debug, Clone)]
pub struct ThemeFamily {
    pub name: String,
    pub author: String,
    pub themes: Vec<Theme>,
}

impl ThemeFamily {
    pub fn parse(json: &str) -> Result<Self, ThemeError> {
        let raw: RawFamily = serde_json::from_str(json)?;
        if raw.themes.is_empty() {
            return Err(ThemeError::Empty(raw.name));
        }
        let themes = raw
            .themes
            .into_iter()
            .map(resolve)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            name: raw.name,
            author: raw.author,
            themes,
        })
    }

    /// Picks a theme by exact name, or the first theme when `name` is `None`.
    pub fn theme(&self, name: Option<&str>) -> Result<&Theme, ThemeError> {
        match name {
            None => self
                .themes
                .first()
                .ok_or_else(|| ThemeError::Empty(self.name.clone())),
            Some(name) => {
                self.themes
                    .iter()
                    .find(|t| t.name == name)
                    .ok_or_else(|| ThemeError::NotFound {
                        family: self.name.clone(),
                        name: name.into(),
                        available: self
                            .themes
                            .iter()
                            .map(|t| t.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    })
            }
        }
    }
}

#[derive(Deserialize)]
struct RawFamily {
    #[serde(default)]
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    themes: Vec<RawTheme>,
}

#[derive(Deserialize)]
struct RawTheme {
    name: String,
    appearance: RawAppearance,
    #[serde(default)]
    style: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawAppearance {
    Light,
    Dark,
}

#[derive(Deserialize, Default)]
struct RawSyntax {
    color: Option<String>,
    font_style: Option<String>,
    font_weight: Option<f64>,
}

#[derive(Deserialize)]
struct RawPlayer {
    cursor: Option<String>,
    background: Option<String>,
    selection: Option<String>,
}

fn colour(key: &str, value: &str) -> Result<Rgba, ThemeError> {
    Rgba::parse(value).ok_or_else(|| ThemeError::Colour {
        key: key.into(),
        value: value.into(),
    })
}

fn resolve(raw: RawTheme) -> Result<Theme, ThemeError> {
    let appearance = match raw.appearance {
        RawAppearance::Light => Appearance::Light,
        RawAppearance::Dark => Appearance::Dark,
    };
    let style = &raw.style;
    // A string colour for `key`, or None when absent or null.
    let get = |key: &str| -> Result<Option<Rgba>, ThemeError> {
        match style.get(key).and_then(|v| v.as_str()) {
            Some(s) => colour(key, s).map(Some),
            None => Ok(None),
        }
    };
    let (default_bg, default_fg) = match appearance {
        Appearance::Dark => (Rgba::rgb(0x1e1e1e), Rgba::rgb(0xd4d4d4)),
        Appearance::Light => (Rgba::rgb(0xffffff), Rgba::rgb(0x1f1f1f)),
    };
    let background = get("background")?.unwrap_or(default_bg);
    let text = get("text")?.unwrap_or(default_fg);
    let text_muted = get("text.muted")?.unwrap_or(text.with_alpha(0xa0));
    let editor_background = get("editor.background")?.unwrap_or(background);
    let editor_foreground = get("editor.foreground")?.unwrap_or(text);
    let editor_line_number = get("editor.line_number")?.unwrap_or(text_muted);

    let mut syntax = HashMap::new();
    if let Some(map) = style.get("syntax").and_then(|v| v.as_object()) {
        for (name, value) in map {
            let raw: RawSyntax = serde_json::from_value(value.clone()).unwrap_or_default();
            let color = raw.color.as_deref().map(|c| colour(name, c)).transpose()?;
            let font_style = match raw.font_style.as_deref() {
                Some("italic") => Some(FontStyle::Italic),
                Some("oblique") => Some(FontStyle::Oblique),
                Some("normal") => Some(FontStyle::Normal),
                _ => None,
            };
            let font_weight = raw.font_weight.map(|w| w.clamp(100.0, 900.0) as u16);
            syntax.insert(
                name.clone(),
                SyntaxStyle {
                    color,
                    font_style,
                    font_weight,
                },
            );
        }
    }

    let mut players = Vec::new();
    if let Some(list) = style.get("players") {
        let raw: Vec<RawPlayer> = serde_json::from_value(list.clone()).unwrap_or_default();
        for p in raw {
            let Some(cursor) = p
                .cursor
                .as_deref()
                .map(|c| colour("players.cursor", c))
                .transpose()?
            else {
                continue;
            };
            let pick = |v: Option<&str>, key, alpha| -> Result<Rgba, ThemeError> {
                Ok(v.map(|c| colour(key, c))
                    .transpose()?
                    .unwrap_or(cursor.with_alpha(alpha)))
            };
            players.push(Player {
                cursor,
                background: pick(p.background.as_deref(), "players.background", 0xff)?,
                selection: pick(p.selection.as_deref(), "players.selection", 0x3d)?,
            });
        }
    }

    let border = get("border")?.unwrap_or(text.with_alpha(0x26));
    let panel_background = get("panel.background")?.unwrap_or(background);
    let element_selected = get("element.selected")?.unwrap_or(text.with_alpha(0x20));
    let cursor = players.first().map_or(text, |p| p.cursor);
    let border_variant = get("border.variant")?.unwrap_or(border.with_alpha(border.a / 2));
    let border_focused = get("border.focused")?.unwrap_or(cursor);
    let element_hover = get("element.hover")?.unwrap_or(text.with_alpha(0x0f));
    let element_active = get("element.active")?.unwrap_or(element_selected);
    let panel_focused_border = get("panel.focused_border")?.unwrap_or(border_focused);
    let text_accent = get("text.accent")?.unwrap_or(cursor);
    let half_muted = text_muted.with_alpha(text_muted.a / 2);
    let title_bar_background = get("title_bar.background")?.unwrap_or(background);
    let tab_bar_background = get("tab_bar.background")?.unwrap_or(title_bar_background);
    let success = match get("success")? {
        Some(c) => Some(c),
        None => get("created")?,
    };
    let warning = match get("warning")? {
        Some(c) => Some(c),
        None => get("modified")?,
    };
    let error = match get("error")? {
        Some(c) => Some(c),
        None => get("deleted")?,
    };
    // A key's own value, else `base` at alpha `a`, else nothing.
    let or_tint =
        |own: Option<Rgba>, base: Option<Rgba>, a: u8| own.or(base.map(|c| c.with_alpha(a)));
    let ui = UiColors {
        border_variant,
        border_focused,
        element_hover,
        ghost_element_hover: get("ghost_element.hover")?.unwrap_or(element_hover),
        element_active,
        panel_focused_border,
        panel_indent_guide: get("panel.indent_guide")?.unwrap_or(border_variant),
        panel_indent_guide_active: get("panel.indent_guide_active")?.unwrap_or(border),
        icon: get("icon")?.unwrap_or(text_muted),
        icon_muted: get("icon.muted")?.unwrap_or(text_muted),
        text_accent,
        elevated_surface_background: get("elevated_surface.background")?
            .unwrap_or(panel_background),
        toolbar_background: get("toolbar.background")?.unwrap_or(editor_background),
        search_match_background: get("search.match_background")?.unwrap_or(cursor.with_alpha(0x47)),
        success,
        warning,
        error,
        icon_accent: get("icon.accent")?.unwrap_or(text_accent),
        icon_disabled: get("icon.disabled")?.unwrap_or(half_muted),
        text_disabled: get("text.disabled")?.unwrap_or(half_muted),
        element_background: get("element.background")?.unwrap_or(text.with_alpha(0x14)),
        ghost_element_active: get("ghost_element.active")?.unwrap_or(element_active),
        ghost_element_selected: get("ghost_element.selected")?.unwrap_or(element_selected),
        pane_focused_border: get("pane.focused_border")?.unwrap_or(panel_focused_border),
        tab_bar_background,
        tab_active_background: get("tab.active_background")?.unwrap_or(editor_background),
        tab_inactive_background: get("tab.inactive_background")?.unwrap_or(tab_bar_background),
        drop_target_background: get("drop_target.background")?
            .unwrap_or(text_accent.with_alpha(0x1f)),
        editor_highlighted_line_background: get("editor.highlighted_line.background")?
            .unwrap_or(text_accent.with_alpha(0x1f)),
        success_background: or_tint(get("success.background")?, success, 0x24),
        created: get("created")?.or(success),
        modified: get("modified")?.or(warning),
        deleted: get("deleted")?.or(error),
        warning_background: or_tint(get("warning.background")?, warning, 0x24),
        error_background: or_tint(get("error.background")?, error, 0x24),
        success_border: or_tint(get("success.border")?, success, 0x80),
        warning_border: or_tint(get("warning.border")?, warning, 0x80),
        error_border: or_tint(get("error.border")?, error, 0x80),
    };

    Ok(Theme {
        name: raw.name,
        appearance,
        border,
        panel_background,
        status_bar_background: get("status_bar.background")?.unwrap_or(background),
        title_bar_background,
        element_selected,
        editor_gutter_background: get("editor.gutter.background")?.unwrap_or(editor_background),
        editor_active_line_number: get("editor.active_line_number")?.unwrap_or(editor_foreground),
        editor_active_line_background: get("editor.active_line.background")?
            .unwrap_or(text.with_alpha(0x0d)),
        background,
        text,
        text_muted,
        editor_background,
        editor_foreground,
        editor_line_number,
        players,
        ui,
        syntax,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colours() {
        assert_eq!(
            Rgba::parse("#fff"),
            Some(Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255
            })
        );
        assert_eq!(
            Rgba::parse("#1a2b3c80"),
            Some(Rgba {
                r: 0x1a,
                g: 0x2b,
                b: 0x3c,
                a: 0x80
            })
        );
        assert_eq!(Rgba::parse("#12"), None);
        assert_eq!(Rgba::parse("123456"), None);
        assert_eq!(Rgba::parse("#gggggg"), None);
        assert_eq!(Rgba::rgb(0x112233).to_u32(), 0x112233ff);
    }

    #[test]
    fn bundled_family_has_both_appearances() {
        let family = ThemeFamily::parse(BUNDLED_FAMILY).unwrap();
        assert_eq!(family.themes.len(), 2);
        assert_eq!(Theme::bundled(Appearance::Dark).name, "Chorong Dark");
        assert_eq!(Theme::bundled(Appearance::Light).name, "Chorong Light");
        assert!(Theme::bundled(Appearance::Dark).players.len() >= 4);
    }

    #[test]
    fn reads_a_zed_style_theme_with_nulls_and_unknown_keys() {
        let json = r##"{
          "$schema": "https://zed.dev/schema/themes/v0.2.0.json",
          "name": "Sample", "author": "someone",
          "themes": [{
            "name": "Sample Dark", "appearance": "dark",
            "style": {
              "editor.background": "#101010ff",
              "editor.line_number": null,
              "terminal.ansi.red": "#ff0000",
              "players": [{ "cursor": "#3366ff", "background": null, "selection": "#3366ff33" }],
              "syntax": {
                "function": { "color": "#aabbcc", "font_style": null, "font_weight": null },
                "keyword": { "color": "#ddeeff", "font_style": "italic", "font_weight": 700 }
              }
            }
          }]
        }"##;
        let family = ThemeFamily::parse(json).unwrap();
        let theme = family.theme(Some("Sample Dark")).unwrap();
        assert_eq!(theme.editor_background, Rgba::rgb(0x101010));
        assert_eq!(theme.players[0].background, Rgba::rgb(0x3366ff));
        let method = theme.syntax_style("function.method").unwrap();
        assert_eq!(method.color, Some(Rgba::rgb(0xaabbcc)));
        let kw = theme.syntax_style("keyword").unwrap();
        assert_eq!(
            (kw.font_style, kw.font_weight),
            (Some(FontStyle::Italic), Some(700))
        );
        assert!(theme.syntax_style("string").is_none());
        assert!(matches!(
            family.theme(Some("Nope")),
            Err(ThemeError::NotFound { .. })
        ));
    }

    #[test]
    fn ui_keys_fall_back_as_documented() {
        let json = r##"{"name":"x","themes":[{"name":"x","appearance":"dark","style":{
            "text":"#ffffff","border":"#40404080","text.muted":"#aaaaaa",
            "players":[{"cursor":"#3366ff"}],
            "element.hover":"#11111111","text.accent":"#ff00ff","warning":"#ffaa00"
        }}]}"##;
        let t = ThemeFamily::parse(json).unwrap().themes.remove(0);
        let ui = &t.ui;
        assert_eq!(ui.border_variant, Rgba::rgb(0x404040).with_alpha(0x40));
        assert_eq!(ui.border_focused, Rgba::rgb(0x3366ff));
        assert_eq!(ui.panel_focused_border, Rgba::rgb(0x3366ff));
        assert_eq!(ui.ghost_element_hover, Rgba::rgb(0x111111).with_alpha(0x11));
        assert_eq!(ui.text_accent, Rgba::rgb(0xff00ff));
        assert_eq!(ui.icon, Rgba::rgb(0xaaaaaa));
        assert_eq!(
            ui.search_match_background,
            Rgba::rgb(0x3366ff).with_alpha(0x47)
        );
        assert_eq!(ui.warning, Some(Rgba::rgb(0xffaa00)));
        assert_eq!(ui.success, None);
        // A theme with none of the keys still resolves every colour.
        let bare =
            ThemeFamily::parse(r#"{"name":"b","themes":[{"name":"b","appearance":"light"}]}"#)
                .unwrap();
        assert_eq!(
            bare.themes[0].ui.elevated_surface_background,
            bare.themes[0].panel_background
        );
    }

    #[test]
    fn colour_by_element_keys_fall_back_as_documented() {
        let json = r##"{"name":"x","themes":[{"name":"x","appearance":"dark","style":{
            "text":"#ffffff","text.muted":"#aaaaaaff","text.accent":"#ff00ff",
            "title_bar.background":"#202020","editor.background":"#101010",
            "element.selected":"#333333","created":"#00ff00","warning":"#ffaa00",
            "warning.background":"#332200","tab_bar.background":null
        }}]}"##;
        let t = ThemeFamily::parse(json).unwrap().themes.remove(0);
        let ui = &t.ui;
        assert_eq!(ui.icon_accent, Rgba::rgb(0xff00ff));
        assert_eq!(ui.icon_disabled, Rgba::rgb(0xaaaaaa).with_alpha(0x7f));
        assert_eq!(ui.element_background, Rgba::rgb(0xffffff).with_alpha(0x14));
        assert_eq!(ui.ghost_element_selected, Rgba::rgb(0x333333));
        assert_eq!(ui.tab_bar_background, Rgba::rgb(0x202020));
        assert_eq!(ui.tab_inactive_background, Rgba::rgb(0x202020));
        assert_eq!(ui.tab_active_background, Rgba::rgb(0x101010));
        assert_eq!(
            ui.drop_target_background,
            Rgba::rgb(0xff00ff).with_alpha(0x1f)
        );
        // success falls back to `created`; its chip and border derive from it.
        assert_eq!(ui.success, Some(Rgba::rgb(0x00ff00)));
        assert_eq!(
            ui.success_background,
            Some(Rgba::rgb(0x00ff00).with_alpha(0x24))
        );
        assert_eq!(
            ui.success_border,
            Some(Rgba::rgb(0x00ff00).with_alpha(0x80))
        );
        // An explicit key wins over the tint.
        assert_eq!(ui.warning_background, Some(Rgba::rgb(0x332200)));
        // No error colour anywhere: no chip colour either (text still says it).
        assert_eq!((ui.error, ui.error_background), (None, None));
        assert_eq!(ui.pane_focused_border, ui.panel_focused_border);
    }

    #[test]
    fn bundled_themes_colour_verification_states() {
        for a in [Appearance::Dark, Appearance::Light] {
            let ui = Theme::bundled(a).ui;
            assert!(ui.success.is_some() && ui.warning.is_some() && ui.error.is_some());
        }
    }

    #[test]
    fn rejects_bad_colours() {
        let json =
            r#"{"name":"x","themes":[{"name":"x","appearance":"light","style":{"text":"red"}}]}"#;
        assert!(matches!(
            ThemeFamily::parse(json),
            Err(ThemeError::Colour { .. })
        ));
    }
}

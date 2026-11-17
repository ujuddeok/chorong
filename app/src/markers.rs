//! How highlight groups are drawn: a rounded box behind the text, outlined in the group's
//! colour. This module holds the pure parts: which colours, in which order, and the box
//! geometry. Painting lives in `view.rs`.

use cho_theme::{Appearance, Rgba, Theme};

/// Pixels of padding on each side of a marker. Markers are positioned absolutely behind the
/// text, so the padding never moves a character.
pub const PAD: f32 = 2.;
pub const RADIUS: f32 = 3.;

/// Hues (degrees) the first three groups aim for: blue, orange, purple.
const FIRST_HUES: [f32; 3] = [215., 30., 280.];
/// Candidates closer than this (RGB distance) to the selection or current-line colour, as
/// they appear on screen, are skipped.
const TOO_CLOSE: f32 = 40.;

/// Hue in degrees and saturation (HSV) of a colour.
pub fn hue_sat(c: Rgba) -> (f32, f32) {
    let (r, g, b) = (c.r as f32 / 255., c.g as f32 / 255., c.b as f32 / 255.);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d == 0. {
        return (0., 0.);
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.)
    } else if max == g {
        (b - r) / d + 2.
    } else {
        (r - g) / d + 4.
    };
    (h * 60., d / max)
}

fn hue_distance(a: f32, b: f32) -> f32 {
    let d = (a - b).abs() % 360.;
    d.min(360. - d)
}

fn distance(a: Rgba, b: Rgba) -> f32 {
    let d = |x: u8, y: u8| (x as f32 - y as f32).powi(2);
    (d(a.r, b.r) + d(a.g, b.g) + d(a.b, b.b)).sqrt()
}

/// `fg` at `alpha` (0 to 1) over the opaque `bg`, as one solid colour.
pub fn blend(fg: Rgba, bg: Rgba, alpha: f32) -> Rgba {
    let mix = |f: u8, b: u8| (f32::from(f) * alpha + f32::from(b) * (1. - alpha)).round() as u8;
    Rgba {
        r: mix(fg.r, bg.r),
        g: mix(fg.g, bg.g),
        b: mix(fg.b, bg.b),
        a: 255,
    }
}

/// The group colours for `theme`, in the order groups take them: blue, orange, purple first,
/// then whichever remaining colour is furthest in hue from those already used. Drawn from the
/// theme's player colours; any that look like the selection or the current line are skipped
/// (unless that would leave fewer than three).
pub fn palette(theme: &Theme) -> Vec<Rgba> {
    let selection = theme.players.first().map(|p| {
        blend(
            p.selection.with_alpha(255),
            theme.editor_background,
            f32::from(p.selection.a) / 255.,
        )
    });
    let line = blend(
        theme.editor_active_line_background.with_alpha(255),
        theme.editor_background,
        f32::from(theme.editor_active_line_background.a) / 255.,
    );
    let all: Vec<Rgba> = theme
        .players
        .iter()
        .map(|p| p.cursor.with_alpha(255))
        .collect();
    let far: Vec<Rgba> = all
        .iter()
        .copied()
        .filter(|c| {
            distance(*c, line) >= TOO_CLOSE
                && selection.is_none_or(|s| distance(*c, s) >= TOO_CLOSE)
        })
        .collect();
    let mut pool = if far.len() >= 3 { far } else { all };
    let mut out = Vec::with_capacity(pool.len());
    for target in FIRST_HUES {
        let best = pool
            .iter()
            .enumerate()
            .filter(|(_, c)| hue_sat(**c).1 > 0.15)
            .min_by(|(_, a), (_, b)| {
                hue_distance(hue_sat(**a).0, target)
                    .total_cmp(&hue_distance(hue_sat(**b).0, target))
            })
            .map(|(i, _)| i);
        if let Some(i) = best {
            out.push(pool.remove(i));
        }
    }
    while !pool.is_empty() {
        let i = (0..pool.len())
            .max_by(|&a, &b| {
                let spread = |i: usize| {
                    out.iter()
                        .map(|c| hue_distance(hue_sat(*c).0, hue_sat(pool[i]).0))
                        .fold(360., f32::min)
                };
                spread(a).total_cmp(&spread(b))
            })
            .unwrap_or(0);
        out.push(pool.remove(i));
    }
    out
}

/// A marker's colours and outline width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub fill: Rgba,
    pub outline: Rgba,
    pub outline_width: f32,
}

/// Fill 15-25% alpha, outline 1px in the group colour; the declaration gets a solid 2px
/// outline. Dark themes: stronger outline, weaker fill.
pub fn style(colour: Rgba, appearance: Appearance, declaration: bool) -> Style {
    let (fill_a, outline_a) = match appearance {
        Appearance::Dark => (0x28, 0xe6),
        Appearance::Light => (0x3c, 0xb3),
    };
    Style {
        fill: colour.with_alpha(fill_a),
        outline: colour.with_alpha(if declaration { 0xff } else { outline_a }),
        outline_width: if declaration { 2. } else { 1. },
    }
}

/// The display column of the display-text byte offset `at`: characters, not bytes.
pub fn column(text: &str, at: usize) -> usize {
    text.get(..at).map_or(0, |s| s.chars().count())
}

/// Left edge and width (px) of a marker over display bytes `range` of `text`, with the
/// padding included. Monospace: x = column * advance.
pub fn extent(text: &str, range: &std::ops::Range<usize>, advance: f32) -> (f32, f32) {
    let start = column(text, range.start);
    let end = column(text, range.end);
    (
        start as f32 * advance - PAD,
        (end - start) as f32 * advance + 2. * PAD,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hue_of(c: Rgba) -> f32 {
        hue_sat(c).0
    }

    #[test]
    fn bundled_palettes_start_blue_orange_purple() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let theme = Theme::bundled(appearance);
            let p = palette(&theme);
            assert!(p.len() >= 6, "{appearance:?}: {}", p.len());
            assert!(hue_distance(hue_of(p[0]), 215.) < 30., "blue first");
            assert!(hue_distance(hue_of(p[1]), 30.) < 30., "orange second");
            assert!(hue_distance(hue_of(p[2]), 280.) < 30., "purple third");
            // No two of the first four hues are close.
            for i in 0..4 {
                for j in i + 1..4 {
                    assert!(hue_distance(hue_of(p[i]), hue_of(p[j])) > 40.);
                }
            }
        }
    }

    #[test]
    fn colours_close_to_the_selection_are_skipped() {
        let mut theme = Theme::bundled(Appearance::Dark);
        // Make the selection look exactly like the first player's cursor colour.
        let first = theme.players[0].cursor;
        theme.players[0].selection = first.with_alpha(255);
        let p = palette(&theme);
        assert!(!p.contains(&first.with_alpha(255)));
        assert_eq!(p.len(), theme.players.len() - 1);
    }

    #[test]
    fn style_follows_the_brief() {
        let c = Rgba::rgb(0x6fa8ff);
        let dark = style(c, Appearance::Dark, false);
        let light = style(c, Appearance::Light, false);
        assert!(dark.outline.a > light.outline.a, "dark: stronger outline");
        assert!(dark.fill.a < light.fill.a, "dark: weaker fill");
        for s in [dark, light] {
            let pct = s.fill.a as f32 / 255.;
            assert!((0.15..=0.25).contains(&pct), "{pct}");
            assert_eq!(s.outline_width, 1.);
        }
        let decl = style(c, Appearance::Dark, true);
        assert_eq!(decl.outline.a, 255);
        assert!(decl.outline_width > 1.);
    }

    #[test]
    fn extent_uses_columns_not_bytes_and_pads_without_shifting() {
        // "é" is two bytes but one column.
        let text = "é abc";
        let range = 3..6; // "abc"
        let (x, w) = extent(text, &range, 8.);
        assert_eq!(x, 2. * 8. - PAD);
        assert_eq!(w, 3. * 8. + 2. * PAD);
    }
}

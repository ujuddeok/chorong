//! Critical Lens: everything fades except call-name tokens, and calls are told apart by kind.
//!
//! Rendering only. The text, layout and positions never change; the lens adds a
//! `fade_out` to every run outside a call name. Which tokens are call names comes from
//! Tree-sitter (`cho_syntax::analyze`); their kind comes from the SCIP index
//! (`callkind`), never from guessing. Repo calls stay fully bright, external calls are
//! slightly dimmed, unknown calls keep their colour and get a thin underline. Without an
//! index (or before its answer arrives) calls carry no kind: the plain lens, every call
//! bright and none marked, since marking every call "unknown" would be noise.

use crate::callkind::CallKind;
use gpui::{HighlightStyle, Hsla, UnderlineStyle, px};
use std::ops::Range;

/// Opacity left on faded text (about 35%).
pub const FADED_OPACITY: f32 = 0.35;
/// Opacity left on external calls (slightly dimmed).
pub const EXTERNAL_OPACITY: f32 = 0.7;
/// Opacity of a highlighted name that would otherwise be faded: some colour back, not all.
pub const HIGHLIGHT_OPACITY: f32 = 0.65;

/// A call token (display offsets in its line) and what the index says it is; `None` when
/// there is no index answer (the plain lens).
pub type KindedCall = (Range<usize>, Option<CallKind>);

/// The lens style for a call of `kind`. `underline` is a theme colour.
pub fn call_style(
    mut style: HighlightStyle,
    kind: Option<CallKind>,
    underline: Hsla,
) -> HighlightStyle {
    match kind {
        None | Some(CallKind::Repo) => {}
        Some(CallKind::External) => style.fade_out = Some(1. - EXTERNAL_OPACITY),
        Some(CallKind::Unknown) => {
            style.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(underline),
                wavy: false,
            });
        }
    }
    style
}

/// Highlights covering the whole of a `len`-byte line: `base` (sorted, disjoint) with
/// gaps filled, every part outside `calls` faded, call parts styled by their kind.
pub fn lens_highlights(
    len: usize,
    base: &[(Range<usize>, HighlightStyle)],
    calls: &[KindedCall],
    underline: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut segments: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut at = 0;
    for (range, style) in base {
        let (start, end) = (range.start.max(at), range.end.min(len));
        if start >= end {
            continue;
        }
        if at < start {
            segments.push((at..start, HighlightStyle::default()));
        }
        segments.push((start..end, *style));
        at = end;
    }
    if at < len {
        segments.push((at..len, HighlightStyle::default()));
    }

    let fade = |mut style: HighlightStyle| {
        style.fade_out = Some(1. - FADED_OPACITY);
        style
    };
    let mut out = Vec::with_capacity(segments.len() + calls.len());
    let mut call = 0;
    for (range, style) in segments {
        let mut pos = range.start;
        while call < calls.len() && calls[call].0.end <= pos {
            call += 1;
        }
        let mut c = call;
        while pos < range.end {
            match calls.get(c).filter(|r| r.0.start < range.end) {
                Some((r, kind)) if r.start <= pos => {
                    let end = r.end.min(range.end);
                    out.push((pos..end, call_style(style, *kind, underline)));
                    pos = end;
                    if r.end <= range.end {
                        c += 1;
                    }
                }
                Some((r, _)) => {
                    out.push((pos..r.start, fade(style)));
                    pos = r.start;
                }
                None => {
                    out.push((pos..range.end, fade(style)));
                    pos = range.end;
                }
            }
        }
    }
    out
}

/// Lightens the fade on every part of `segments` inside `keep` (sorted display ranges) to
/// [`HIGHLIGHT_OPACITY`]: the occurrences of highlight groups get some colour back under the
/// lens, without competing with the calls. Parts that are not faded or already lighter
/// (external calls), and other styling, are untouched.
pub fn unfade(
    segments: Vec<(Range<usize>, HighlightStyle)>,
    keep: &[Range<usize>],
) -> Vec<(Range<usize>, HighlightStyle)> {
    if keep.is_empty() {
        return segments;
    }
    let mut out = Vec::with_capacity(segments.len() + keep.len() * 2);
    let lighter = 1. - HIGHLIGHT_OPACITY;
    for (range, style) in segments {
        // Not faded, or already lighter than a highlight (an external call): leave it.
        if style.fade_out.is_none_or(|f| f <= lighter) {
            out.push((range, style));
            continue;
        }
        let bright = HighlightStyle {
            fade_out: Some(lighter),
            ..style
        };
        let mut pos = range.start;
        for k in keep
            .iter()
            .filter(|k| k.start < range.end && k.end > range.start)
        {
            let (start, end) = (k.start.max(pos), k.end.min(range.end));
            if start >= end {
                continue;
            }
            if pos < start {
                out.push((pos..start, style));
            }
            out.push((start..end, bright));
            pos = end;
        }
        if pos < range.end {
            out.push((pos..range.end, style));
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn highlighted_occurrences_are_faded_less() {
        let faded = HighlightStyle {
            fade_out: Some(1. - FADED_OPACITY),
            ..Default::default()
        };
        let call = HighlightStyle::default();
        let segs = vec![(0..10, faded), (10..14, call), (14..20, faded)];
        let out = unfade(segs.clone(), &[2..5, 12..16, 18..30]);
        let (f, h) = (Some(1. - FADED_OPACITY), Some(1. - HIGHLIGHT_OPACITY));
        let spans: Vec<_> = out.iter().map(|(r, s)| (r.clone(), s.fade_out)).collect();
        assert_eq!(
            spans,
            [
                (0..2, f),
                (2..5, h),
                (5..10, f),
                // A call is not faded, so a highlight on it stays at full colour.
                (10..14, None),
                (14..16, h),
                (16..18, f),
                (18..20, h),
            ]
        );
        assert_eq!(unfade(segs.clone(), &[]), segs);
    }

    /// Found by `unfade_only_lightens_faded_bytes_inside_highlights`: a highlighted external
    /// call (70%) was darkened to the highlight level (65%).
    #[test]
    fn a_highlighted_external_call_is_not_darkened() {
        let external = call_style(HighlightStyle::default(), Some(CallKind::External), RULE);
        let segs = vec![(0..5, external)];
        assert_eq!(unfade(segs.clone(), &[1..3]), segs);
    }

    /// xorshift64*, for generated lines.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n.max(1) as u64) as usize
        }

        /// Sorted ranges inside `0..limit`, possibly empty, overlapping or repeated.
        fn ranges(&mut self, limit: usize, count: usize) -> Vec<Range<usize>> {
            let mut v: Vec<_> = (0..count)
                .map(|_| {
                    let start = self.below(limit + 1);
                    start..start + self.below(limit + 1 - start)
                })
                .collect();
            v.sort_by_key(|r| r.start);
            v
        }
    }

    /// The style `segments` give byte `b`.
    fn style_at(segments: &[(Range<usize>, HighlightStyle)], b: usize) -> HighlightStyle {
        segments.iter().find(|(r, _)| r.contains(&b)).unwrap().1
    }

    /// Property test over generated lines: the lens's own output (base styles, calls of every
    /// kind) and arbitrary contiguous segments, with highlight ranges that overlap, repeat, are
    /// empty or run past the line. `unfade` keeps the segments contiguous over the same bytes,
    /// with no empty pieces, and changes nothing but `fade_out`, and that only to lighten faded
    /// bytes inside a highlight to `HIGHLIGHT_OPACITY`.
    #[test]
    fn unfade_only_lightens_faded_bytes_inside_highlights() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let styles = [
            HighlightStyle::default(),
            coloured(),
            HighlightStyle {
                fade_out: Some(1. - FADED_OPACITY),
                ..coloured()
            },
            HighlightStyle {
                fade_out: Some(1. - EXTERNAL_OPACITY),
                ..Default::default()
            },
        ];
        let kinds = [
            None,
            Some(CallKind::Repo),
            Some(CallKind::External),
            Some(CallKind::Unknown),
        ];
        for case in 0..20_000 {
            let len = rng.below(40);
            let segments = if case % 2 == 0 {
                let mut base = Vec::new();
                let mut at = 0;
                while at < len {
                    let start = at + rng.below(3);
                    let end = (start + 1 + rng.below(6)).min(len);
                    if start < end {
                        base.push((start..end, styles[rng.below(styles.len())]));
                    }
                    at = end.max(start);
                }
                let mut calls: Vec<KindedCall> = Vec::new();
                let mut at = 0;
                while at < len {
                    let start = at + rng.below(6);
                    let end = start + 1 + rng.below(5);
                    if end <= len {
                        calls.push((start..end, kinds[rng.below(kinds.len())]));
                    }
                    at = end;
                }
                lens_highlights(len, &base, &calls, RULE)
            } else {
                let mut segs = Vec::new();
                let mut at = 0;
                while at < len {
                    let end = (at + 1 + rng.below(8)).min(len);
                    segs.push((at..end, styles[rng.below(styles.len())]));
                    at = end;
                }
                segs
            };
            let count = rng.below(6);
            let keep = rng.ranges(len + 4, count);
            let out = unfade(segments.clone(), &keep);
            let mut at = 0;
            for (r, _) in &out {
                assert!(r.start == at && r.start < r.end, "{out:?}");
                at = r.end;
            }
            assert_eq!(at, len, "covers the line");
            for b in 0..len {
                let (before, after) = (style_at(&segments, b), style_at(&out, b));
                let lit = before.fade_out.is_some() && keep.iter().any(|k| k.contains(&b));
                let want = HighlightStyle {
                    // Lighter, never darker: an external call is already brighter.
                    fade_out: if lit {
                        before.fade_out.map(|f| f.min(1. - HIGHLIGHT_OPACITY))
                    } else {
                        before.fade_out
                    },
                    ..before
                };
                assert_eq!(after, want, "byte {b}: {segments:?} keep {keep:?}");
            }
        }
    }

    const RULE: Hsla = Hsla {
        h: 0.,
        s: 0.,
        l: 0.5,
        a: 1.,
    };

    fn repo(r: Range<usize>) -> KindedCall {
        (r, Some(CallKind::Repo))
    }

    fn coloured() -> HighlightStyle {
        HighlightStyle {
            color: Some(gpui::hsla(0.5, 0.5, 0.5, 1.)),
            ..Default::default()
        }
    }

    fn faded(r: &(Range<usize>, HighlightStyle)) -> bool {
        r.1.fade_out.is_some()
    }

    #[test]
    fn fills_gaps_and_fades_everything_but_calls() {
        // 0..4 styled, 4..6 gap, 6..9 styled; call at 6..9.
        let base = vec![(0..4, coloured()), (6..9, coloured())];
        let out = lens_highlights(12, &base, &[repo(6..9)], RULE);
        let ranges: Vec<_> = out.iter().map(|r| r.0.clone()).collect();
        assert_eq!(ranges, [0..4, 4..6, 6..9, 9..12]);
        assert_eq!(
            out.iter().map(faded).collect::<Vec<_>>(),
            [true, true, false, true]
        );
        assert_eq!(out[2].1, coloured(), "call keeps its syntax style");
        assert_eq!(
            out[0].1.color,
            coloured().color,
            "faded text keeps its colour"
        );
    }

    #[test]
    fn a_call_inside_a_gap_or_spanning_styles_is_left_alone() {
        let base = vec![(0..3, coloured()), (3..6, HighlightStyle::default())];
        let out = lens_highlights(8, &base, &[repo(1..5)], RULE);
        let calls: Vec<_> = out
            .iter()
            .filter(|r| !faded(r))
            .map(|r| r.0.clone())
            .collect();
        assert_eq!(calls, [1..3, 3..5]);
        // Coverage is exact and contiguous.
        let mut at = 0;
        for (r, _) in &out {
            assert_eq!(r.start, at);
            at = r.end;
        }
        assert_eq!(at, 8);
    }

    #[test]
    fn no_calls_fades_the_whole_line_and_empty_lines_are_empty() {
        let out = lens_highlights(5, &[], &[], RULE);
        assert_eq!(out.len(), 1);
        assert!(faded(&out[0]));
        assert!(lens_highlights(0, &[], &[], RULE).is_empty());
    }

    #[test]
    fn fade_leaves_about_35_percent() {
        let out = lens_highlights(1, &[], &[], RULE);
        let mut c = gpui::hsla(0., 0., 1., 1.);
        c.fade_out(out[0].1.fade_out.unwrap());
        assert!((c.a - 0.35).abs() < 1e-6);
    }

    #[test]
    fn kinds_are_told_apart_and_none_is_hidden() {
        let calls = vec![
            (0..2, Some(CallKind::Repo)),
            (3..5, Some(CallKind::External)),
            (6..8, Some(CallKind::Unknown)),
        ];
        let out = lens_highlights(9, &[], &calls, RULE);
        let at = |n: usize| out.iter().find(|r| r.0.start == n).unwrap().1;
        let (repo, ext, unk) = (at(0), at(3), at(6));
        assert!(
            repo.fade_out.is_none() && repo.underline.is_none(),
            "bright"
        );
        let mut c = gpui::hsla(0., 0., 1., 1.);
        c.fade_out(ext.fade_out.unwrap());
        assert!(c.a > 0.6 && c.a < 0.8, "slightly dimmed, not faded");
        assert!(unk.fade_out.is_none(), "unknown stays fully visible");
        assert_eq!(unk.underline.unwrap().color, Some(RULE));
        // Everything outside the calls is faded to the lens level.
        assert!(at(2).fade_out.is_some());
    }

    #[test]
    fn without_an_index_calls_are_bright_and_unmarked() {
        let out = lens_highlights(5, &[], &[(1..4, None)], RULE);
        let call = out.iter().find(|r| r.0 == (1..4)).unwrap().1;
        assert!(call.fade_out.is_none(), "calls stand out");
        assert!(
            call.underline.is_none(),
            "no \"unknown\" mark without an index"
        );
        assert!(out.iter().filter(|r| r.0 != (1..4)).all(faded));
    }
}

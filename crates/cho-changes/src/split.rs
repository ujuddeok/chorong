//! Telling committed changes from uncommitted ones.
//!
//! Three line diffs describe one file: base to HEAD (committed), HEAD to the working folder
//! (uncommitted) and base to the working folder (what is shown). A hunk of the last one is
//! uncommitted where HEAD to working touches it. [`split_hunks`] cuts each shown hunk at the
//! edges of the uncommitted regions, so every piece is purely committed or purely uncommitted.
//! All ranges are zero-based, half-open lines.

use cho_git::LineHunk;
use std::ops::Range;

/// A piece of a base-to-working hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// Base lines replaced.
    pub base: Range<usize>,
    /// Working-folder lines that replace them.
    pub head: Range<usize>,
    /// HEAD to working touches this piece.
    pub uncommitted: bool,
}

/// Where HEAD position `pos` sits in the base, through the base-to-HEAD hunks. A position
/// inside a changed region clamps to the same offset in the base range.
fn head_to_base(bh: &[LineHunk], pos: usize) -> usize {
    let mut shift: isize = 0;
    for h in bh {
        if pos <= h.head.start {
            break;
        }
        if pos < h.head.end {
            return h.base.start + (pos - h.head.start).min(h.base.len());
        }
        shift += h.base.len() as isize - h.head.len() as isize;
    }
    (pos as isize + shift) as usize
}

/// Splits each hunk of `bw` (base to working) at the edges of the regions `hw` (HEAD to
/// working) touches. `bh` (base to HEAD) places HEAD lines in the base. Pieces cover each
/// hunk exactly, in order; none is empty on both sides.
pub fn split_hunks(bw: &[LineHunk], bh: &[LineHunk], hw: &[LineHunk]) -> Vec<Piece> {
    // Each uncommitted region in base and working coordinates.
    let regions: Vec<(Range<usize>, Range<usize>)> = hw
        .iter()
        .map(|e| {
            (
                head_to_base(bh, e.base.start)..head_to_base(bh, e.base.end),
                e.head.clone(),
            )
        })
        .collect();
    let mut out = Vec::new();
    for h in bw {
        let (mut b, mut w) = (h.base.start, h.head.start);
        let mut pieces: Vec<Piece> = Vec::new();
        for (rb, rw) in &regions {
            // Clip the region to the hunk.
            let w0 = rw.start.clamp(h.head.start, h.head.end);
            let w1 = rw.end.clamp(h.head.start, h.head.end);
            let b0 = rb.start.clamp(h.base.start, h.base.end);
            let b1 = rb.end.clamp(h.base.start, h.base.end);
            let qualifies = w0 < w1
                || (rw.is_empty() && h.head.start <= rw.start && rw.start <= h.head.end && b0 < b1);
            if !qualifies {
                continue;
            }
            let (b0, w0) = (b0.max(b), w0.max(w));
            let (b1, w1) = (b1.max(b0), w1.max(w0));
            if b0 > b || w0 > w {
                pieces.push(Piece {
                    base: b..b0,
                    head: w..w0,
                    uncommitted: false,
                });
            }
            if b1 > b0 || w1 > w0 {
                match pieces.last_mut() {
                    Some(p) if p.uncommitted && p.base.end == b0 && p.head.end == w0 => {
                        p.base.end = b1;
                        p.head.end = w1;
                    }
                    _ => pieces.push(Piece {
                        base: b0..b1,
                        head: w0..w1,
                        uncommitted: true,
                    }),
                }
            }
            b = b1;
            w = w1;
        }
        if h.base.end > b || h.head.end > w {
            pieces.push(Piece {
                base: b..h.base.end,
                head: w..h.head.end,
                uncommitted: false,
            });
        }
        out.extend(pieces);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_git::linediff::diff_lines;

    fn hunk(base: Range<usize>, head: Range<usize>) -> LineHunk {
        LineHunk { base, head }
    }

    fn pieces(base: &str, head: &str, work: &str) -> Vec<Piece> {
        let d = |a: &str, b: &str| diff_lines(a.as_bytes(), b.as_bytes(), 2000).hunks;
        split_hunks(&d(base, work), &d(base, head), &d(head, work))
    }

    fn p(base: Range<usize>, head: Range<usize>, uncommitted: bool) -> Piece {
        Piece {
            base,
            head,
            uncommitted,
        }
    }

    #[test]
    fn positions_map_from_head_to_the_base() {
        // HEAD gained two lines at 2 and changed line 6 (base 4..5 became head 6..8).
        let bh = [hunk(2..2, 2..4), hunk(4..5, 6..8)];
        assert_eq!(head_to_base(&bh, 1), 1);
        assert_eq!(head_to_base(&bh, 2), 2);
        assert_eq!(head_to_base(&bh, 4), 2);
        assert_eq!(head_to_base(&bh, 5), 3);
        assert_eq!(head_to_base(&bh, 7), 5);
        assert_eq!(head_to_base(&bh, 8), 5);
        assert_eq!(head_to_base(&bh, 10), 7);
    }

    #[test]
    fn a_hunk_touched_only_by_committed_work_is_committed() {
        assert_eq!(
            pieces("a\nb\nc\n", "a\nB\nc\n", "a\nB\nc\n"),
            [p(1..2, 1..2, false)]
        );
    }

    #[test]
    fn a_hunk_touched_only_by_uncommitted_work_is_uncommitted() {
        assert_eq!(
            pieces("a\nb\nc\n", "a\nb\nc\n", "a\nX\nc\n"),
            [p(1..2, 1..2, true)]
        );
        // An untracked file: nothing at base or HEAD.
        assert_eq!(pieces("", "", "a\nb\n"), [p(0..0, 0..2, true)]);
    }

    #[test]
    fn a_line_changed_twice_counts_as_uncommitted() {
        assert_eq!(
            pieces("a\nb\nc\n", "a\nB\nc\n", "a\nB2\nc\n"),
            [p(1..2, 1..2, true)]
        );
    }

    #[test]
    fn a_mixed_hunk_is_cut_where_the_uncommitted_edit_starts() {
        // Base 1..3 became two committed lines, and the working folder adds one more.
        assert_eq!(
            pieces("a\nb\nc\nd\n", "a\nX\nY\nd\n", "a\nX\nY\nZ\nd\n"),
            [p(1..3, 1..3, false), p(3..3, 3..4, true)]
        );
        // Uncommitted edit in the middle of a committed block.
        assert_eq!(
            pieces(
                "a\nb\nc\nd\ne\nf\n",
                "a\nB\nC\nD\nE\nf\n",
                "a\nB\nC2\nD\nE\nf\n"
            ),
            [
                p(1..2, 1..2, false),
                p(2..3, 2..3, true),
                p(3..5, 3..5, false)
            ]
        );
    }

    #[test]
    fn deletions_are_split_by_who_deleted_them() {
        // HEAD deleted line 1; the working folder also deletes line 3 of the base.
        assert_eq!(
            pieces("a\nb\nc\nd\n", "a\nc\nd\n", "a\nc\n"),
            [p(1..2, 1..1, false), p(3..4, 2..2, true)]
        );
        // The working folder deletes what HEAD changed.
        assert_eq!(
            pieces("a\nb\nc\n", "a\nB\nc\n", "a\nc\n"),
            [p(1..2, 1..1, true)]
        );
    }

    #[test]
    fn work_that_matches_the_base_again_leaves_no_hunk() {
        assert_eq!(pieces("a\nb\n", "a\nB\n", "a\nb\n"), Vec::<Piece>::new());
    }

    /// Small deterministic generator for the property test.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) % n
        }
    }

    fn text(rng: &mut Rng, from: &[String]) -> Vec<String> {
        let mut out = from.to_vec();
        for _ in 0..rng.next(4) {
            let at = rng.next(out.len() as u64 + 1) as usize;
            match rng.next(3) {
                0 => out.insert(at, format!("n{}", rng.next(50))),
                1 if at < out.len() => {
                    out.remove(at);
                }
                _ if at < out.len() => out[at] = format!("m{}", rng.next(50)),
                _ => {}
            }
        }
        out
    }

    fn join(lines: &[String]) -> String {
        lines.iter().map(|l| format!("{l}\n")).collect()
    }

    /// Pieces cover each shown hunk exactly and in order, none is empty, and a working line is
    /// in an uncommitted piece exactly when HEAD to working changed it.
    #[test]
    fn random_histories_split_cleanly() {
        let mut rng = Rng(7);
        for _ in 0..400 {
            let base: Vec<String> = (0..rng.next(12)).map(|i| format!("l{i}")).collect();
            let head = text(&mut rng, &base);
            let work = text(&mut rng, &head);
            let (b, h, w) = (join(&base), join(&head), join(&work));
            let d = |a: &str, c: &str| diff_lines(a.as_bytes(), c.as_bytes(), 2000).hunks;
            let (bw, bh, hw) = (d(&b, &w), d(&b, &h), d(&h, &w));
            let got = split_hunks(&bw, &bh, &hw);
            let touched = |line: usize| hw.iter().any(|e| e.head.contains(&line));
            let ctx = format!("{b:?} {h:?} {w:?}: {got:?}");
            let mut it = got.iter();
            for hunk in &bw {
                let (mut cb, mut cw) = (hunk.base.start, hunk.head.start);
                while (cb, cw) != (hunk.base.end, hunk.head.end) {
                    let piece = it.next().unwrap_or_else(|| panic!("missing piece: {ctx}"));
                    assert_eq!((piece.base.start, piece.head.start), (cb, cw), "{ctx}");
                    assert!(!piece.base.is_empty() || !piece.head.is_empty(), "{ctx}");
                    for line in piece.head.clone() {
                        assert_eq!(piece.uncommitted, touched(line), "{ctx}");
                    }
                    (cb, cw) = (piece.base.end, piece.head.end);
                    assert!(cb <= hunk.base.end && cw <= hunk.head.end, "{ctx}");
                }
            }
            assert!(it.next().is_none(), "{ctx}");
        }
    }
}

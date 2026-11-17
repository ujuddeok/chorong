//! A deterministic line diff (Myers, with git-style sliding of pure insertions and deletions),
//! so changed lines can be computed from two blobs without a checkout or a `git` process.
//!
//! Lines keep their terminating `\n`; a last line without one is still a line. Positions are
//! zero-based line indices; [`Lines`] converts them to UTF-8 byte offsets.

use std::collections::HashMap;
use std::ops::Range;

/// One changed region: `base` lines were replaced by `head` lines. One side may be empty (a
/// pure insertion or deletion); its range is then the position between two lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineHunk {
    pub base: Range<usize>,
    pub head: Range<usize>,
}

/// The result of [`diff_lines`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDiff {
    pub hunks: Vec<LineHunk>,
    /// The edit distance exceeded the cost limit, so the differing middle (after the common
    /// prefix and suffix) is reported as one hunk instead of a minimal diff.
    pub coarse: bool,
}

/// A text split into lines, with the byte offset where each starts.
#[derive(Debug, Clone)]
pub struct Lines<'a> {
    text: &'a [u8],
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub fn new(text: &'a [u8]) -> Self {
        let mut starts = vec![];
        let mut at = 0;
        while at < text.len() {
            starts.push(at);
            at = match text[at..].iter().position(|&b| b == b'\n') {
                Some(i) => at + i + 1,
                None => text.len(),
            };
        }
        Self { text, starts }
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// The bytes of line `i`, including its `\n`.
    pub fn line(&self, i: usize) -> &'a [u8] {
        let end = self.starts.get(i + 1).copied().unwrap_or(self.text.len());
        &self.text[self.starts[i]..end]
    }

    /// The byte offset where line `i` starts; `len()` gives the end of the text.
    pub fn offset(&self, i: usize) -> usize {
        self.starts.get(i).copied().unwrap_or(self.text.len())
    }

    /// The byte range lines `r` cover (empty at the boundary for an empty `r`).
    pub fn bytes(&self, r: &Range<usize>) -> Range<usize> {
        self.offset(r.start)..self.offset(r.end)
    }
}

/// Edit distance beyond which [`diff_lines`] stops searching for a minimal diff.
pub const DEFAULT_MAX_COST: usize = 2000;

/// The changed regions between `base` and `head`, in order.
pub fn diff_lines(base: &[u8], head: &[u8], max_cost: usize) -> LineDiff {
    let (a, b) = (Lines::new(base), Lines::new(head));
    diff(&a, &b, max_cost)
}

/// [`diff_lines`] on texts already split.
pub fn diff<'a>(a: &Lines<'a>, b: &Lines<'a>, max_cost: usize) -> LineDiff {
    // Interning: equal lines get equal ids, assigned in order of first appearance.
    let mut ids: HashMap<&[u8], u32> = HashMap::new();
    let mut intern = |l: &Lines<'a>| -> Vec<u32> {
        (0..l.len())
            .map(|i| {
                let next = ids.len() as u32;
                *ids.entry(l.line(i)).or_insert(next)
            })
            .collect()
    };
    let (xa, xb) = (intern(a), intern(b));
    diff_ids(&xa, &xb, max_cost)
}

/// The diff of two sequences of interned items (equal items have equal ids): lines for
/// [`diff`], tokens for [`crate::worddiff`]. Hunk ranges index the sequences.
pub fn diff_ids(xa: &[u32], xb: &[u32], max_cost: usize) -> LineDiff {
    let prefix = xa.iter().zip(xb).take_while(|(p, q)| p == q).count();
    let suffix = xa[prefix..]
        .iter()
        .rev()
        .zip(xb[prefix..].iter().rev())
        .take_while(|(p, q)| p == q)
        .count();
    let (ma, mb) = (
        &xa[prefix..xa.len() - suffix],
        &xb[prefix..xb.len() - suffix],
    );
    if ma.is_empty() && mb.is_empty() {
        return LineDiff {
            hunks: vec![],
            coarse: false,
        };
    }
    let (matches, coarse) = match myers(ma, mb, max_cost) {
        Some(m) => (m, false),
        None => (vec![], true),
    };

    // Gaps between matched lines are the hunks.
    let mut hunks = vec![];
    let (mut x, mut y) = (0, 0);
    for &(mx, my) in matches.iter().chain([(ma.len(), mb.len())].iter()) {
        if mx > x || my > y {
            hunks.push(LineHunk {
                base: prefix + x..prefix + mx,
                head: prefix + y..prefix + my,
            });
        }
        x = mx + 1;
        y = my + 1;
    }
    slide(&mut hunks, xa, xb);
    LineDiff { hunks, coarse }
}

/// Moves each pure insertion or deletion as far down as the same lines allow, the way git
/// compacts groups, so `}\nfn b() {` becomes `fn b() {\n}`.
fn slide(hunks: &mut [LineHunk], xa: &[u32], xb: &[u32]) {
    for i in 0..hunks.len() {
        let limit_a = hunks.get(i + 1).map_or(xa.len(), |n| n.base.start);
        let limit_b = hunks.get(i + 1).map_or(xb.len(), |n| n.head.start);
        let h = &mut hunks[i];
        if h.base.is_empty() && !h.head.is_empty() {
            while h.head.end < limit_b && h.base.end < limit_a && xb[h.head.start] == xb[h.head.end]
            {
                h.head = h.head.start + 1..h.head.end + 1;
                h.base = h.base.start + 1..h.base.end + 1;
            }
        } else if h.head.is_empty() && !h.base.is_empty() {
            while h.base.end < limit_a && h.head.end < limit_b && xa[h.base.start] == xa[h.base.end]
            {
                h.base = h.base.start + 1..h.base.end + 1;
                h.head = h.head.start + 1..h.head.end + 1;
            }
        }
    }
}

/// Matched line pairs of a minimal diff (Myers' O(ND) algorithm), ascending, or `None` when
/// the edit distance exceeds `max_cost`.
fn myers(a: &[u32], b: &[u32], max_cost: usize) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max_d = (max_cost as isize).min(n + m);
    let o = max_d + 1;
    let mut v = vec![0isize; (2 * max_d + 3) as usize];
    let mut trace: Vec<Vec<isize>> = vec![];
    let mut found = None;
    'outer: for d in 0..=max_d {
        trace.push(v[(o - d - 1) as usize..=(o + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let ki = (o + k) as usize;
            let mut x = if k == -d || (k != d && v[ki - 1] < v[ki + 1]) {
                v[ki + 1]
            } else {
                v[ki - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[ki] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    let total = found?;
    let mut matches = vec![];
    let (mut x, mut y) = (n, m);
    for d in (0..=total).rev() {
        // `trace[d]` holds k in -d-1..=d+1 at index k + d + 1.
        let t = &trace[d as usize];
        let at = |k: isize| t[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = if d == 0 { 0 } else { at(prev_k) };
        let prev_y = if d == 0 { 0 } else { prev_x - prev_k };
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            matches.push((x as usize, y as usize));
        }
        x = prev_x;
        y = prev_y;
    }
    matches.reverse();
    Some(matches)
}

/// How much of `base` survives in `head`, as git scores renames: bytes of matched lines over
/// the larger size, 0–100. A coarse diff counts only the common prefix and suffix.
pub fn similarity(base: &[u8], head: &[u8], max_cost: usize) -> u8 {
    let larger = base.len().max(head.len());
    if larger == 0 {
        return 100;
    }
    let (a, b) = (Lines::new(base), Lines::new(head));
    let d = diff(&a, &b, max_cost);
    let changed: usize = d.hunks.iter().map(|h| a.bytes(&h.base).len()).sum();
    let kept = base.len() - changed;
    ((kept * 100) / larger) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(a: &str, b: &str) -> Vec<(Range<usize>, Range<usize>)> {
        diff_lines(a.as_bytes(), b.as_bytes(), DEFAULT_MAX_COST)
            .hunks
            .into_iter()
            .map(|h| (h.base, h.head))
            .collect()
    }

    /// Applying the hunks to `a` must give `b`, and unchanged lines must be equal.
    fn check(a: &str, b: &str, max_cost: usize) {
        let (la, lb) = (Lines::new(a.as_bytes()), Lines::new(b.as_bytes()));
        let r = diff(&la, &lb, max_cost);
        let (mut x, mut y) = (0, 0);
        for h in &r.hunks {
            assert!(h.base.start >= x && h.head.start >= y);
            assert_eq!(h.base.start - x, h.head.start - y);
            for k in 0..h.base.start - x {
                assert_eq!(la.line(x + k), lb.line(y + k));
            }
            assert!(!h.base.is_empty() || !h.head.is_empty());
            x = h.base.end;
            y = h.head.end;
        }
        assert_eq!(la.len() - x, lb.len() - y);
        for k in 0..la.len() - x {
            assert_eq!(la.line(x + k), lb.line(y + k));
        }
    }

    #[test]
    fn equal_texts_have_no_hunks() {
        assert!(d("a\nb\n", "a\nb\n").is_empty());
        assert!(d("", "").is_empty());
    }

    #[test]
    fn insertions_deletions_and_replacements() {
        assert_eq!(d("a\nc\n", "a\nb\nc\n"), vec![(1..1, 1..2)]);
        assert_eq!(d("a\nb\nc\n", "a\nc\n"), vec![(1..2, 1..1)]);
        assert_eq!(d("a\nb\nc\n", "a\nx\nc\n"), vec![(1..2, 1..2)]);
        assert_eq!(d("", "a\n"), vec![(0..0, 0..1)]);
        assert_eq!(d("a", "a\n"), vec![(0..1, 0..1)]);
    }

    #[test]
    fn insertions_slide_below_a_repeated_line() {
        let a = "fn a() {\n}\nfn c() {\n}\n";
        let b = "fn a() {\n}\nfn b() {\n}\nfn c() {\n}\n";
        assert_eq!(d(a, b), vec![(2..2, 2..4)]);
    }

    #[test]
    fn hunks_reconstruct_the_head() {
        let cases = [
            ("a\nb\nc\nd\n", "b\nc\nx\nd\ny\n"),
            ("x\nx\nx\n", "x\ny\nx\n"),
            ("1\n2\n3\n4\n5\n6\n", "6\n5\n4\n3\n2\n1\n"),
            ("a\nb", "a\nc"),
        ];
        for (a, b) in cases {
            check(a, b, DEFAULT_MAX_COST);
            check(b, a, DEFAULT_MAX_COST);
            check(a, b, 1);
        }
    }

    #[test]
    fn a_cost_limit_gives_one_coarse_hunk() {
        let r = diff_lines(b"p\n1\n2\n3\ns\n", b"p\n4\n5\n6\ns\n", 2);
        assert!(r.coarse);
        assert_eq!(r.hunks.len(), 1);
        assert_eq!(
            (r.hunks[0].base.clone(), r.hunks[0].head.clone()),
            (1..4, 1..4)
        );
    }

    /// Seeded random texts: hunks reconstruct the head, and without the cost limit the diff is
    /// minimal (edited lines = n + m - 2 * LCS). The `line_diff` fuzz target checks the first
    /// part on arbitrary bytes.
    #[test]
    fn random_diffs_are_valid_and_minimal() {
        let mut seed: u64 = 0x5eed;
        let mut next = |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        for _ in 0..500 {
            let mut text = |len: u64| -> String {
                (0..next(len))
                    .map(|_| ["a\n", "b\n", "c\n", "}\n", "\n"][next(5) as usize])
                    .collect()
            };
            let (a, b) = (text(14), text(14));
            check(&a, &b, DEFAULT_MAX_COST);
            check(&a, &b, next(4) as usize);
            let (la, lb) = (Lines::new(a.as_bytes()), Lines::new(b.as_bytes()));
            let r = diff(&la, &lb, DEFAULT_MAX_COST);
            let edited: usize = r.hunks.iter().map(|h| h.base.len() + h.head.len()).sum();
            let mut t = vec![vec![0usize; lb.len() + 1]; la.len() + 1];
            for i in (0..la.len()).rev() {
                for j in (0..lb.len()).rev() {
                    t[i][j] = if la.line(i) == lb.line(j) {
                        t[i + 1][j + 1] + 1
                    } else {
                        t[i + 1][j].max(t[i][j + 1])
                    };
                }
            }
            assert_eq!(edited, la.len() + lb.len() - 2 * t[0][0], "{a:?} -> {b:?}");
        }
    }

    #[test]
    fn similarity_scores_shared_lines() {
        assert_eq!(similarity(b"a\nb\n", b"a\nb\n", 100), 100);
        assert_eq!(similarity(b"a\nb\n", b"a\nc\n", 100), 50);
        assert_eq!(similarity(b"a\n", b"b\n", 100), 0);
    }
}

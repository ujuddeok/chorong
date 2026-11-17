//! Changed words within a changed region: which old and new lines are versions of each other,
//! and which tokens differ between them. Deterministic, from [`crate::linediff`] run over
//! tokens instead of lines.
//!
//! The rules, so a reader sees word highlights only where they help:
//!
//! - Tokens are identifiers and numbers (runs of letters, digits and `_`), single punctuation
//!   characters, and whitespace runs. Words inside a string literal are tokens of their own.
//! - Whitespace is ignored when comparing: a line that differs only in whitespace is paired
//!   and marked [`PairedLine::whitespace_only`], with no spans.
//! - An old and a new line are paired only when they are at least [`MIN_SIMILARITY`] percent
//!   alike by words (twice the common words over both counts; punctuation is matched but not
//!   counted, so `let x = foo(a, b);` and `let total = compute(b, a, scale);` are not alike).
//!   A line without words counts its punctuation instead. The pairing is in order and
//!   maximises the total likeness.
//! - A paired line gets no spans when more than half of either side's words differ.
//! - Changed tokens separated by one or two unchanged tokens become one span.
//!
//! Positions are UTF-8 byte offsets within a line (lines are passed without their `\n`).

use crate::linediff::diff_ids;
use std::collections::HashMap;
use std::ops::Range;

/// Lines at least this alike (percent) are paired.
const MIN_SIMILARITY: usize = 50;

/// A paired line with more than this share (percent) of its tokens changed gets no spans.
const MAX_CHANGED: usize = 50;

/// Unchanged tokens between two changed ones, up to this many, are drawn as part of one span.
const MERGE_GAP: usize = 2;

/// A region with more line pairs to weigh than this is not paired (no word highlights).
const MAX_PAIR_CELLS: usize = 10_000;

/// Edit distance beyond which two lines' tokens are not diffed (a coarse diff is no use here).
const MAX_TOKEN_COST: usize = 200;

/// An old line and the new line that is a version of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedLine {
    /// Index of the line among the region's old (base) lines.
    pub base: usize,
    /// Index of the line among the region's new (head) lines.
    pub head: usize,
    /// Byte ranges of the changed words in the old line.
    pub base_spans: Vec<Range<usize>>,
    /// Byte ranges of the changed words in the new line.
    pub head_spans: Vec<Range<usize>>,
    /// The lines differ only in whitespace.
    pub whitespace_only: bool,
}

/// The tokens of a line that count (no whitespace), as byte ranges.
pub fn tokens(line: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut word: Option<usize> = None;
    for (i, c) in line.char_indices() {
        let is_word = c.is_alphanumeric() || c == '_';
        match (word, is_word) {
            (None, true) => word = Some(i),
            (Some(start), false) => {
                out.push(start..i);
                word = None;
            }
            _ => {}
        }
        if !is_word && !c.is_whitespace() {
            out.push(i..i + c.len_utf8());
        }
    }
    if let Some(start) = word {
        out.push(start..line.len());
    }
    out
}

/// One line, tokenised and interned.
struct Tokenised {
    ranges: Vec<Range<usize>>,
    ids: Vec<u32>,
    /// Which tokens count for likeness: the words, or every token in a line without words.
    counts: Vec<bool>,
}

fn tokenise<'a>(line: &'a str, ids: &mut HashMap<&'a str, u32>) -> Tokenised {
    let ranges = tokens(line);
    let ids = ranges
        .iter()
        .map(|r| {
            let next = ids.len() as u32;
            *ids.entry(&line[r.clone()]).or_insert(next)
        })
        .collect();
    let word: Vec<bool> = ranges
        .iter()
        .map(|r| {
            line[r.clone()]
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_')
        })
        .collect();
    let counts = if word.contains(&true) {
        word
    } else {
        vec![true; ranges.len()]
    };
    Tokenised {
        ranges,
        ids,
        counts,
    }
}

/// How many of `t`'s counted tokens `flags` marks.
fn counted(t: &Tokenised, flags: &[bool]) -> usize {
    flags
        .iter()
        .zip(&t.counts)
        .filter(|(f, c)| **f && **c)
        .count()
}

/// The unchanged token pairs of two lines (indices into each), or `None` when the diff is too
/// costly to be useful.
fn common(a: &[u32], b: &[u32]) -> Option<Vec<bool>> {
    let d = diff_ids(a, b, MAX_TOKEN_COST);
    if d.coarse {
        return None;
    }
    let mut kept = vec![true; a.len()];
    for h in &d.hunks {
        for k in h.base.clone() {
            kept[k] = false;
        }
    }
    Some(kept)
}

/// How alike two tokenised lines are, 0–100: twice the common counted tokens over both
/// counts. Two lines with nothing to count are alike.
fn likeness(a: &Tokenised, b: &Tokenised) -> usize {
    let total = counted(a, &vec![true; a.ids.len()]) + counted(b, &vec![true; b.ids.len()]);
    if total == 0 {
        return 100;
    }
    let same = common(&a.ids, &b.ids).map_or(0, |k| counted(a, &k));
    200 * same / total
}

/// The changed tokens of one side as spans: a run of changed tokens, joined across gaps of up
/// to [`MERGE_GAP`] unchanged ones.
fn spans(ranges: &[Range<usize>], changed: &[bool]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut last_changed: Option<usize> = None;
    for (i, c) in changed.iter().enumerate() {
        if !c {
            continue;
        }
        match (last_changed, out.last_mut()) {
            (Some(l), Some(span)) if i - l - 1 <= MERGE_GAP => span.end = ranges[i].end,
            _ => out.push(ranges[i].clone()),
        }
        last_changed = Some(i);
    }
    out
}

/// The changed words of a paired line.
fn pair(base: &Tokenised, head: &Tokenised, b: usize, h: usize) -> PairedLine {
    let mut p = PairedLine {
        base: b,
        head: h,
        base_spans: Vec::new(),
        head_spans: Vec::new(),
        whitespace_only: base.ids == head.ids,
    };
    if p.whitespace_only {
        return p;
    }
    let d = diff_ids(&base.ids, &head.ids, MAX_TOKEN_COST);
    if d.coarse {
        return p;
    }
    let mut changed_base = vec![false; base.ids.len()];
    let mut changed_head = vec![false; head.ids.len()];
    for hunk in &d.hunks {
        hunk.base.clone().for_each(|k| changed_base[k] = true);
        hunk.head.clone().for_each(|k| changed_head[k] = true);
    }
    let too_much = |t: &Tokenised, c: &[bool]| {
        100 * counted(t, c) > MAX_CHANGED * counted(t, &vec![true; c.len()])
    };
    if too_much(base, &changed_base) || too_much(head, &changed_head) {
        return p;
    }
    p.base_spans = spans(&base.ranges, &changed_base);
    p.head_spans = spans(&head.ranges, &changed_head);
    p
}

/// Pairs the old and new lines of one changed region and finds their changed words. Lines
/// are passed without their line ending. A region too large to weigh ([`MAX_PAIR_CELLS`]) has
/// no pairs.
pub fn paired_lines(base: &[&str], head: &[&str]) -> Vec<PairedLine> {
    if base.is_empty() || head.is_empty() || base.len() * head.len() > MAX_PAIR_CELLS {
        return Vec::new();
    }
    let mut ids = HashMap::new();
    let tb: Vec<Tokenised> = base.iter().map(|l| tokenise(l, &mut ids)).collect();
    let th: Vec<Tokenised> = head.iter().map(|l| tokenise(l, &mut ids)).collect();
    let (n, m) = (tb.len(), th.len());
    let alike: Vec<Vec<usize>> = tb
        .iter()
        .map(|a| th.iter().map(|b| likeness(a, b)).collect())
        .collect();
    // best[i][j]: the most total likeness pairing base[i..] with head[j..], in order.
    let mut best = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let skip = best[i + 1][j].max(best[i][j + 1]);
            let take = if alike[i][j] >= MIN_SIMILARITY {
                alike[i][j] + best[i + 1][j + 1]
            } else {
                0
            };
            best[i][j] = skip.max(take);
        }
    }
    // Walk it back; on a tie, pairing comes first, then skipping an old line.
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        let a = alike[i][j];
        if a >= MIN_SIMILARITY && best[i][j] == a + best[i + 1][j + 1] {
            out.push(pair(&tb[i], &th[j], i, j));
            i += 1;
            j += 1;
        } else if best[i][j] == best[i + 1][j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests;

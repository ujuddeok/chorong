//! Fuzzy ranking (ordered-subsequence matching via nucleo), for the viewer's file picker and
//! the shell's pane pickers, so a query matches the same way in each.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Index into the candidate list.
    pub index: usize,
    pub score: u32,
    /// Byte ranges in the candidate that matched the query, for highlighting.
    pub ranges: Vec<Range<usize>>,
}

pub struct Ranker {
    matcher: Matcher,
    /// Equal scores put the shorter candidate first (paths); otherwise list order decides.
    shorter_first: bool,
}

impl Ranker {
    /// For file paths: path separators score as boundaries, and ties prefer shorter paths.
    pub fn paths() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            shorter_first: true,
        }
    }

    /// For labels in a list with its own order (recency, say): ties keep that order.
    pub fn labels() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            shorter_first: false,
        }
    }

    /// The best `limit` candidates for `query`, best first. Ties prefer shorter candidates
    /// ([`Ranker::paths`]), then earlier ones. An empty query returns the first `limit`
    /// candidates.
    pub fn rank<S: AsRef<str>>(
        &mut self,
        candidates: &[S],
        query: &str,
        limit: usize,
    ) -> Vec<Match> {
        let query = query.trim();
        if query.is_empty() {
            return (0..candidates.len().min(limit))
                .map(|index| Match {
                    index,
                    score: 0,
                    ranges: Vec::new(),
                })
                .collect();
        }
        // Fuzzy atoms only: the query is a plain subsequence, not nucleo's `^`/`$` syntax.
        let pattern = Pattern::new(
            query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, usize)> = candidates
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let hay = Utf32Str::new(c.as_ref(), &mut buf);
                pattern.score(hay, &mut self.matcher).map(|s| (s, i))
            })
            .collect();
        let len = |i: usize| {
            if self.shorter_first {
                candidates[i].as_ref().len()
            } else {
                0
            }
        };
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(len(a.1).cmp(&len(b.1)))
                .then(a.1.cmp(&b.1))
        });
        scored.truncate(limit);
        scored
            .into_iter()
            .map(|(score, index)| {
                let text = candidates[index].as_ref();
                let hay = Utf32Str::new(text, &mut buf);
                let mut idx = Vec::new();
                pattern.indices(hay, &mut self.matcher, &mut idx);
                idx.sort_unstable();
                idx.dedup();
                Match {
                    index,
                    score,
                    ranges: char_ranges(text, &idx),
                }
            })
            .collect()
    }
}

/// Converts matched char indices into merged byte ranges.
pub fn char_ranges(text: &str, chars: &[u32]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut want = chars.iter().peekable();
    for (n, (byte, ch)) in text.char_indices().enumerate() {
        match want.peek() {
            Some(&&c) if c as usize == n => {
                want.next();
                let end = byte + ch.len_utf8();
                match out.last_mut() {
                    Some(last) if last.end == byte => last.end = end,
                    _ => out.push(byte..end),
                }
            }
            Some(_) => {}
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_RESULTS: usize = 50;

    const FILES: &[&str] = &[
        "docs/PLAN.md",
        "crates/cho-syntax/src/outline.rs",
        "crates/cho-core/src/engine-foo.rs",
        "crates/cho-syntax/src/highlight.rs",
        "app/src/view.rs",
    ];

    #[test]
    fn ordered_subsequence_matches() {
        let hits = Ranker::paths().rank(FILES, "engrs", 50);
        // `highlight.rs` also contains e..n..g..r..s in order; the filename match ranks first.
        assert_eq!(FILES[hits[0].index], "crates/cho-core/src/engine-foo.rs");
    }

    #[test]
    fn order_matters() {
        assert_eq!(Ranker::paths().rank(&["abc.rs"], "abc", 50).len(), 1);
        assert!(Ranker::paths().rank(&["abc.rs"], "cba", 50).is_empty());
    }

    #[test]
    fn filename_matches_rank_first() {
        let hits = Ranker::paths().rank(FILES, "outline", 50);
        assert_eq!(FILES[hits[0].index], "crates/cho-syntax/src/outline.rs");
    }

    #[test]
    fn results_are_limited_and_empty_query_lists_files() {
        let many: Vec<String> = (0..200).map(|i| format!("src/file{i}.rs")).collect();
        assert_eq!(Ranker::paths().rank(&many, "file", MAX_RESULTS).len(), 50);
        assert_eq!(Ranker::paths().rank(&many, "", MAX_RESULTS).len(), 50);
    }

    #[test]
    fn match_ranges_are_byte_ranges_in_the_candidate() {
        let hits = Ranker::paths().rank(&["src/engine.rs"], "eng", 5);
        let text = "src/engine.rs";
        let got: String = hits[0].ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(got, "eng");
    }

    #[test]
    fn labels_keep_the_list_order_on_ties() {
        let hits = Ranker::labels().rank(&["outline long", "outline"], "outline", 5);
        assert_eq!(hits.iter().map(|h| h.index).collect::<Vec<_>>(), [0, 1]);
        let hits = Ranker::paths().rank(&["outline long", "outline"], "outline", 5);
        assert_eq!(hits[0].index, 1);
    }

    #[test]
    fn multibyte_paths_do_not_panic() {
        let hits = Ranker::paths().rank(&["src/héllo/wörld.rs"], "wör", 5);
        assert_eq!(hits.len(), 1);
        for r in &hits[0].ranges {
            assert!("src/héllo/wörld.rs".is_char_boundary(r.start));
        }
    }
}

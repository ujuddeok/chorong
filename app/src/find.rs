//! Find in File (Cmd-F): the find bar's state and its matching, without GPUI.
//!
//! The open file is one buffer in memory, so matches are found again on every edit of the
//! query, on the UI thread. A literal query is matched as typed (case-insensitive unless
//! **Aa** is on); with **.\*** on it is a regular expression (`regex` crate: linear time, so a
//! pattern cannot hang the viewer). Matches are UTF-8 byte ranges, sorted and disjoint, and
//! become the pane's Find highlight group (`marks::Mode::Find`), which `n` / `N`, the gutter,
//! the overview lane and the minimap already show.

use std::ops::Range;

/// The most matches kept: enough for any reading use, and it bounds the work of the
/// highlight group's gutter, overview and minimap marks on a huge file.
pub const MAX_MATCHES: usize = 10_000;

/// Every match of `query` in `text`, at most `max`, and whether there were more. Empty
/// matches (a regex such as `a*`) are skipped: they mark nothing. An invalid regex is an
/// error with the regex crate's message (one line).
pub fn find_all(
    text: &str,
    query: &str,
    case_sensitive: bool,
    regex: bool,
    max: usize,
) -> Result<(Vec<Range<usize>>, bool), String> {
    if query.is_empty() {
        return Ok((Vec::new(), false));
    }
    let pattern = if regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let re = regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        // `^` and `$` are line anchors, as in project search; `$` stops before a CR.
        .multi_line(true)
        .crlf(true)
        .build()
        .map_err(|e| one_line(&e.to_string()))?;
    let mut out = Vec::new();
    for m in re.find_iter(text).filter(|m| !m.is_empty()) {
        if out.len() == max {
            return Ok((out, true));
        }
        out.push(m.range());
    }
    Ok((out, false))
}

/// The regex crate's error, which draws a caret under the pattern over several lines, as the
/// last line (the reason), prefixed for the bar.
fn one_line(error: &str) -> String {
    let reason = error
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("invalid pattern");
    let reason = reason.strip_prefix("error: ").unwrap_or(reason);
    format!("Invalid regex: {reason}")
}

/// One code pane's find bar.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FindBar {
    pub query: String,
    /// The query is selected (Cmd-A, or Cmd-F again): typing replaces it.
    pub query_all: bool,
    /// **Aa**: match case. Off, a query matches in any case.
    pub case_sensitive: bool,
    /// **.\***: the query is a regular expression.
    pub regex: bool,
    /// Where the caret was when the query last changed from empty: typing jumps to the first
    /// match at or after it, so narrowing a query does not wander down the file.
    pub origin: usize,
    /// The matches, sorted and disjoint.
    pub matches: Vec<Range<usize>>,
    /// More than [`MAX_MATCHES`] matched.
    pub truncated: bool,
    /// The query is not a valid regex.
    pub error: Option<String>,
}

impl FindBar {
    /// A new bar, its query `query` selected, typing from `caret`.
    pub fn new(query: String, caret: usize) -> Self {
        FindBar {
            query_all: !query.is_empty(),
            query,
            origin: caret,
            ..FindBar::default()
        }
    }

    /// Finds the query again in `text` (after an edit, a toggle or a new document).
    pub fn refresh(&mut self, text: &str) {
        match find_all(
            text,
            &self.query,
            self.case_sensitive,
            self.regex,
            MAX_MATCHES,
        ) {
            Ok((matches, truncated)) => {
                self.matches = matches;
                self.truncated = truncated;
                self.error = None;
            }
            Err(e) => {
                self.matches.clear();
                self.truncated = false;
                self.error = Some(e);
            }
        }
    }

    /// The match typing lands on: the first starting at or after `origin`, wrapping to the
    /// first in the file.
    pub fn first_from_origin(&self) -> Option<usize> {
        (!self.matches.is_empty()).then(|| {
            let i = self.matches.partition_point(|r| r.start < self.origin);
            if i == self.matches.len() { 0 } else { i }
        })
    }

    /// The bar's count: `3 of 12` when the caret is on a match, `12 matches` otherwise, or
    /// why there are none.
    pub fn summary(&self, caret: Option<usize>) -> String {
        if let Some(e) = &self.error {
            return e.clone();
        }
        if self.query.is_empty() {
            return String::new();
        }
        let n = self.matches.len();
        let total = if self.truncated {
            format!("{n}+")
        } else {
            n.to_string()
        };
        if n == 0 {
            return "No matches".into();
        }
        match caret.and_then(|c| crate::marks::occurrence_at(&self.matches, c)) {
            Some(i) => format!("{} of {total}", i + 1),
            None if n == 1 && !self.truncated => "1 match".into(),
            None => format!("{total} matches"),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::single_range_in_vec_init,
    reason = "expected match lists are written as vec![a..b], one range per match"
)]
mod tests {
    use super::*;
    use crate::marks::step_occurrence;

    fn find(text: &str, q: &str, case: bool, regex: bool) -> Vec<Range<usize>> {
        find_all(text, q, case, regex, MAX_MATCHES).unwrap().0
    }

    #[test]
    fn a_literal_ignores_case_unless_aa_is_on() {
        let text = "Foo foo FOO";
        assert_eq!(find(text, "foo", false, false), [0..3, 4..7, 8..11]);
        assert_eq!(find(text, "foo", true, false), [4..7]);
        assert_eq!(find(text, "", false, false), Vec::<Range<usize>>::new());
    }

    #[test]
    fn a_literal_is_not_a_regex() {
        let text = "a.b axb (x)";
        assert_eq!(find(text, "a.b", false, false), [0..3]);
        assert_eq!(find(text, "a.b", false, true), [0..3, 4..7]);
        assert_eq!(find(text, "(x)", false, false), [8..11]);
    }

    #[test]
    fn regex_anchors_are_per_line_and_empty_matches_are_skipped() {
        let text = "fn a\r\nfn b\nlet c";
        assert_eq!(find(text, "^fn", true, true), [0..2, 6..8]);
        assert_eq!(find(text, "[ab]$", true, true), [3..4, 9..10]);
        assert_eq!(find(text, "x*", true, true), Vec::<Range<usize>>::new());
    }

    #[test]
    fn an_invalid_regex_is_reported_on_one_line() {
        let err = find_all("abc", "a(b", false, true, MAX_MATCHES).unwrap_err();
        assert!(err.starts_with("Invalid regex: "), "{err}");
        assert!(!err.contains('\n'), "{err}");
        let mut bar = FindBar::new("a(b".into(), 0);
        bar.regex = true;
        bar.refresh("a(b");
        assert!(bar.matches.is_empty());
        assert_eq!(bar.summary(None), bar.error.clone().unwrap());
        // As a literal the same text is fine.
        bar.regex = false;
        bar.refresh("a(b");
        assert_eq!((bar.matches.clone(), bar.error.clone()), (vec![0..3], None));
    }

    #[test]
    fn matches_are_byte_offsets_in_multi_byte_text() {
        let text = "naïve — ÉCOLE école 🦀crab";
        let e = find(text, "école", false, false);
        let words: Vec<&str> = e.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(words, ["ÉCOLE", "école"], "case folding covers É");
        let crab = find(text, "crab", true, false);
        assert_eq!(crab, [text.find("crab").unwrap()..text.len()]);
        assert_eq!(find(text, "ï", true, false), [2..4]);
    }

    #[test]
    fn matches_stop_at_the_cap() {
        let (m, more) = find_all("aaaa", "a", true, false, 3).unwrap();
        assert_eq!((m.len(), more), (3, true));
        let (m, more) = find_all("aaa", "a", true, false, 3).unwrap();
        assert_eq!((m.len(), more), (3, false));
    }

    #[test]
    fn typing_lands_on_the_first_match_from_where_it_started_wrapping() {
        let mut bar = FindBar::new(String::new(), 4);
        bar.query = "ab".into();
        bar.refresh("ab ab ab ab");
        assert_eq!(bar.matches, [0..2, 3..5, 6..8, 9..11]);
        assert_eq!(bar.first_from_origin(), Some(2), "3..5 starts before 4");
        bar.origin = 9;
        assert_eq!(
            bar.first_from_origin(),
            Some(3),
            "a match at the origin counts"
        );
        bar.origin = 10;
        assert_eq!(bar.first_from_origin(), Some(0), "wraps");
        bar.query = "zz".into();
        bar.refresh("ab");
        assert_eq!(bar.first_from_origin(), None);
    }

    #[test]
    fn the_count_says_which_match_the_caret_is_on() {
        let mut bar = FindBar::new("x".into(), 0);
        assert!(bar.query_all, "a carried-over query starts selected");
        bar.refresh("x y x");
        assert_eq!(bar.summary(Some(4)), "2 of 2");
        assert_eq!(bar.summary(Some(2)), "2 matches");
        bar.refresh("x");
        assert_eq!(bar.summary(None), "1 match");
        bar.refresh("y");
        assert_eq!(bar.summary(None), "No matches");
        bar.query.clear();
        assert_eq!(bar.summary(None), "");
        let (m, truncated) = find_all("xxx", "x", true, false, 2).unwrap();
        bar.query = "x".into();
        (bar.matches, bar.truncated) = (m, truncated);
        assert_eq!(bar.summary(Some(1)), "2 of 2+");
    }

    /// Enter and Shift-Enter step with the highlight group's rule: from the match the caret
    /// is on to the next or previous, wrapping at either end.
    #[test]
    fn enter_and_shift_enter_wrap() {
        let m = find("one two one two one", "one", true, false);
        assert_eq!(m, [0..3, 8..11, 16..19]);
        assert_eq!(step_occurrence(&m, 0, true), Some(1));
        assert_eq!(step_occurrence(&m, 16, true), Some(0), "wraps forward");
        assert_eq!(step_occurrence(&m, 0, false), Some(2), "wraps back");
        assert_eq!(step_occurrence(&m, 5, false), Some(0));
        assert_eq!(step_occurrence(&m, 5, true), Some(1));
    }
}

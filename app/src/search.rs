//! The search panel's data: query options, results grouped by file, and the flat row list
//! the sidebar renders. The search itself runs in `cho-search`, off the UI thread.

use crate::generation::Generation;
use cho_search::{CancelToken, Hit};
use std::ops::Range;

/// Longest result line shown, in bytes (cut at a character boundary).
const MAX_LINE: usize = 220;

/// A result line ready to draw: leading space trimmed, long lines cut, match ranges moved
/// to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultLine {
    pub path: String,
    /// One-based.
    pub line: usize,
    /// Byte offset in the file of the first match; where a click lands.
    pub byte: usize,
    pub text: String,
    /// Byte ranges in `text`.
    pub ranges: Vec<Range<usize>>,
}

pub fn result_line(hit: &Hit) -> ResultLine {
    let trimmed = hit.text.trim_start();
    let shift = hit.text.len() - trimmed.len();
    let mut end = trimmed.len().min(MAX_LINE);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    let ranges = hit
        .ranges
        .iter()
        .filter_map(|r| {
            let (s, e) = (r.start.checked_sub(shift)?, r.end.checked_sub(shift)?);
            (s < end).then_some(s..e.min(end))
        })
        .collect();
    ResultLine {
        path: hit.path.clone(),
        line: hit.line,
        byte: hit.first_match_byte(),
        text: trimmed[..end].to_string(),
        ranges,
    }
}

/// One row of the results list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    File {
        path: String,
        count: usize,
    },
    /// Index into `SearchState::lines`.
    Line(usize),
}

pub fn rows_for(lines: &[ResultLine]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut file_row: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        match file_row {
            Some(f) if matches!(&rows[f], Row::File { path, .. } if *path == l.path) => {
                if let Row::File { count, .. } = &mut rows[f] {
                    *count += 1;
                }
            }
            _ => {
                file_row = Some(rows.len());
                rows.push(Row::File {
                    path: l.path.clone(),
                    count: 1,
                });
            }
        }
        rows.push(Row::Line(i));
    }
    rows
}

/// Where the current query's search is. Old results stay on screen while a new one runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SearchStatus {
    #[default]
    Idle,
    Running,
    Failed(String),
}

pub struct SearchState {
    pub query: String,
    /// The whole query is selected (Cmd-A).
    pub query_all: bool,
    pub regex: bool,
    pub case_sensitive: bool,
    /// Only the files the comparison changed (the Changes list's files), while comparing.
    pub changed_only: bool,
    pub lines: Vec<ResultLine>,
    pub rows: Vec<Row>,
    /// Index into `rows` of the keyboard selection (always a `Row::Line`).
    pub selected: Option<usize>,
    pub files: usize,
    pub truncated: bool,
    pub status: SearchStatus,
    /// Bumped on every query change; a result for an older one is dropped.
    pub generation: Generation,
    pub cancel: CancelToken,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            query_all: false,
            regex: false,
            case_sensitive: false,
            changed_only: false,
            lines: Vec::new(),
            rows: Vec::new(),
            selected: None,
            files: 0,
            truncated: false,
            status: SearchStatus::Idle,
            generation: Generation::default(),
            cancel: CancelToken::new(),
        }
    }
}

impl SearchState {
    /// A search finished with `hits`.
    pub fn set_results(&mut self, hits: &[Hit], truncated: bool) {
        self.lines = hits.iter().map(result_line).collect();
        self.rows = rows_for(&self.lines);
        self.files = self
            .rows
            .iter()
            .filter(|r| matches!(r, Row::File { .. }))
            .count();
        self.truncated = truncated;
        self.selected = self.rows.iter().position(|r| matches!(r, Row::Line(_)));
        self.status = SearchStatus::Idle;
    }

    pub fn clear_results(&mut self) {
        self.set_results(&[], false);
    }

    /// A search for the current query starts; the last results stay until it ends.
    pub fn start(&mut self) {
        self.status = SearchStatus::Running;
    }

    /// A search failed: no results, and the error instead.
    pub fn fail(&mut self, error: String) {
        self.clear_results();
        self.status = SearchStatus::Failed(error);
    }

    /// Moves the selection to the neighbouring result line, skipping file headers.
    pub fn step(&mut self, delta: isize) {
        let Some(cur) = self.selected else { return };
        let mut i = cur as isize + delta;
        while i >= 0 && (i as usize) < self.rows.len() {
            if matches!(self.rows[i as usize], Row::Line(_)) {
                self.selected = Some(i as usize);
                return;
            }
            i += delta;
        }
    }

    pub fn selected_line(&self) -> Option<&ResultLine> {
        match self.rows.get(self.selected?)? {
            Row::Line(i) => self.lines.get(*i),
            Row::File { .. } => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn hit(path: &str, line: usize, text: &str, ranges: Vec<Range<usize>>) -> Hit {
        Hit {
            path: path.into(),
            line,
            line_start: 100,
            text: text.into(),
            ranges,
        }
    }

    #[test]
    fn leading_whitespace_is_trimmed_and_ranges_follow() {
        let l = result_line(&hit("a.rs", 3, "\t  foo(bar)", vec![3..6, 7..10]));
        assert_eq!(l.text, "foo(bar)");
        assert_eq!(l.ranges, vec![0..3, 4..7]);
        assert_eq!(&l.text[l.ranges[0].clone()], "foo");
        assert_eq!(l.byte, 103, "click lands on the real first match byte");
    }

    #[test]
    fn long_lines_are_cut_on_a_char_boundary() {
        let text = format!("{}é{}", "a".repeat(MAX_LINE - 1), "tail");
        let l = result_line(&hit(
            "a.rs",
            1,
            &text,
            vec![0..1, MAX_LINE + 2..MAX_LINE + 4],
        ));
        assert!(l.text.len() <= MAX_LINE);
        assert_eq!(l.ranges, vec![0..1], "a match past the cut is dropped");
    }

    #[test]
    fn rows_group_lines_by_file_with_counts() {
        let lines: Vec<_> = [("a", 1), ("a", 5), ("b", 2)]
            .iter()
            .map(|(p, n)| result_line(&hit(p, *n, "x", vec![0..1])))
            .collect();
        assert_eq!(
            rows_for(&lines),
            vec![
                Row::File {
                    path: "a".into(),
                    count: 2
                },
                Row::Line(0),
                Row::Line(1),
                Row::File {
                    path: "b".into(),
                    count: 1
                },
                Row::Line(2),
            ]
        );
    }

    #[test]
    fn selection_skips_file_headers() {
        let mut s = SearchState::default();
        s.set_results(
            &[hit("a", 1, "x", vec![0..1]), hit("b", 2, "x", vec![0..1])],
            false,
        );
        assert_eq!(s.selected_line().unwrap().path, "a");
        s.step(1);
        assert_eq!(s.selected_line().unwrap().path, "b");
        s.step(1);
        assert_eq!(s.selected_line().unwrap().path, "b", "stays on the last");
        s.step(-1);
        assert_eq!(s.selected_line().unwrap().path, "a");
        assert_eq!(s.files, 2);
    }

    /// A failed search's error no longer shows once the next search starts.
    #[test]
    fn a_new_search_replaces_the_last_error() {
        let mut s = SearchState::default();
        s.start();
        s.fail("bad regex".into());
        assert_eq!(s.status, SearchStatus::Failed("bad regex".into()));
        assert!(s.lines.is_empty());
        s.start();
        assert_eq!(s.status, SearchStatus::Running);
        s.set_results(&[hit("a", 1, "x", vec![0..1])], false);
        assert_eq!(s.status, SearchStatus::Idle);
    }
}

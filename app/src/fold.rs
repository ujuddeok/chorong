//! Folding as a visible-row mapping.
//!
//! The source text is never copied or changed. A fold hides the lines between a
//! signature line and a closing line; `Folds::rows` lists what is shown, in order,
//! and the view renders those rows. Everything here is pure so it can be tested
//! without a window.

use std::collections::HashSet;

/// A foldable region. `start` is the signature line, `end` the closing line.
/// Lines `start + 1 .. end` are hidden while the fold is collapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    pub start: usize,
    pub end: usize,
}

impl Fold {
    pub fn hidden_lines(&self) -> usize {
        self.end - self.start - 1
    }
}

/// One visible row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// A buffer line (zero-based).
    Line(usize),
    /// The `⋯ N lines` placeholder for the fold at this index in `Folds::folds`.
    Placeholder(usize),
    /// A highlight's note strip, just above its first line: the index into
    /// [`Folds::note_lines`]. Shown only while that line is.
    Note(usize),
}

/// Only regions spanning more than this many lines are folded by default.
pub const MIN_FOLD_LINES: usize = 3;

/// Builds folds from `(signature line, closing line)` pairs. Keeps pairs spanning
/// more than `MIN_FOLD_LINES` lines, sorted by start; when two share a start line the
/// outermost wins, and a fold inside another that closes on the same line (a function's
/// body block under a signature spread over several lines) adds nothing and is dropped.
pub fn folds_from_spans(spans: impl IntoIterator<Item = (usize, usize)>) -> Vec<Fold> {
    let mut folds: Vec<Fold> = spans
        .into_iter()
        .filter(|(s, e)| e > s && e - s + 1 > MIN_FOLD_LINES)
        .map(|(start, end)| Fold { start, end })
        .collect();
    folds.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    folds.dedup_by_key(|f| f.start);
    let mut kept: Vec<Fold> = Vec::with_capacity(folds.len());
    for f in folds {
        let redundant = kept
            .iter()
            .rev()
            .any(|k| k.start < f.start && k.end == f.end);
        if !redundant {
            kept.push(f);
        }
    }
    kept
}

/// What Left or Right does on a selected line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKey {
    /// Fold or unfold this fold.
    Toggle(usize),
    /// Move the selection to this line.
    GoTo(usize),
}

#[derive(Debug, Clone)]
pub struct Folds {
    folds: Vec<Fold>,
    collapsed: HashSet<usize>,
    line_count: usize,
    rows: Vec<Row>,
    /// Lines (zero-based, ascending) that have a note strip above them.
    notes: Vec<usize>,
    /// Changes whenever `rows` does, and is never the same for two different row lists
    /// (unique across all `Folds`), so a view can cache what it drew from them.
    generation: u64,
}

impl Folds {
    /// `collapsed` holds the start lines of the folds that start collapsed.
    pub fn new(folds: Vec<Fold>, line_count: usize, collapsed: HashSet<usize>) -> Self {
        let mut this = Self {
            folds,
            collapsed,
            line_count,
            rows: Vec::new(),
            notes: Vec::new(),
            generation: 0,
        };
        this.rebuild();
        this
    }

    /// Every fold collapsed or none (tests; the viewer starts from `Document::folded_at_open`).
    #[cfg(test)]
    pub fn default_state(folds: &[Fold], fold_by_default: bool) -> HashSet<usize> {
        if fold_by_default {
            folds.iter().map(|f| f.start).collect()
        } else {
            HashSet::new()
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// A number that changes whenever [`Folds::rows`] may have changed (see `generation`).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn folds(&self) -> &[Fold] {
        &self.folds
    }

    /// The lines with a note strip above them.
    pub fn note_lines(&self) -> &[usize] {
        &self.notes
    }

    /// Puts a note strip row above each of `lines` (zero-based; sorted and deduplicated here).
    /// Returns whether the rows changed.
    pub fn set_notes(&mut self, mut lines: Vec<usize>) -> bool {
        lines.sort_unstable();
        lines.dedup();
        if lines == self.notes {
            return false;
        }
        self.notes = lines;
        self.rebuild();
        true
    }

    /// The start lines of collapsed folds, for remembering state per file.
    pub fn collapsed(&self) -> &HashSet<usize> {
        &self.collapsed
    }

    fn rebuild(&mut self) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        self.generation = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.rows.clear();
        let mut line = 0;
        let mut next = 0; // index of the first fold that could start at or after `line`
        let mut note = 0; // index of the first note at or after `line`
        while line < self.line_count {
            while next < self.folds.len() && self.folds[next].start < line {
                next += 1;
            }
            while note < self.notes.len() && self.notes[note] < line {
                note += 1;
            }
            if self.notes.get(note) == Some(&line) {
                self.rows.push(Row::Note(note));
            }
            self.rows.push(Row::Line(line));
            match self.folds.get(next) {
                Some(f) if f.start == line && self.collapsed.contains(&line) => {
                    self.rows.push(Row::Placeholder(next));
                    line = f.end; // the closing line is shown next
                }
                _ => line += 1,
            }
        }
    }

    /// Left, as in a tree: on an open fold's first line, fold it; otherwise go to the first
    /// line of the innermost fold holding `line` (its closing line included), so a second
    /// Left folds that. `None` at top level.
    pub fn left(&self, line: usize) -> Option<FoldKey> {
        if let Some(f) = self.fold_at(line).filter(|f| !self.is_collapsed(*f)) {
            return Some(FoldKey::Toggle(f));
        }
        self.folds
            .iter()
            .filter(|f| f.start < line && line <= f.end)
            .max_by_key(|f| f.start)
            .map(|f| FoldKey::GoTo(f.start))
    }

    /// Right, as in a tree: on a folded line, unfold it; on an open fold's first line, step
    /// into its body. `None` elsewhere.
    pub fn right(&self, line: usize) -> Option<FoldKey> {
        let f = self.fold_at(line)?;
        if self.is_collapsed(f) {
            Some(FoldKey::Toggle(f))
        } else {
            (line + 1 < self.folds[f].end).then_some(FoldKey::GoTo(line + 1))
        }
    }

    /// The fold whose signature is `line`.
    pub fn fold_at(&self, line: usize) -> Option<usize> {
        self.folds.binary_search_by_key(&line, |f| f.start).ok()
    }

    pub fn is_collapsed(&self, fold: usize) -> bool {
        self.collapsed.contains(&self.folds[fold].start)
    }

    /// Collapses (or expands) every fold at once, rebuilding the rows once rather than per fold.
    /// Returns whether anything changed.
    pub fn set_all(&mut self, collapse: bool) -> bool {
        let before = self.collapsed.len();
        if collapse {
            self.collapsed.extend(self.folds.iter().map(|f| f.start));
        } else {
            self.collapsed.clear();
        }
        if self.collapsed.len() == before && (collapse || before == 0) {
            return false;
        }
        self.rebuild();
        true
    }

    /// Toggles a fold. Returns false for an unknown index.
    pub fn toggle(&mut self, fold: usize) -> bool {
        let Some(f) = self.folds.get(fold) else {
            return false;
        };
        if !self.collapsed.remove(&f.start) {
            self.collapsed.insert(f.start);
        }
        self.rebuild();
        true
    }

    fn key(&self, row: Row) -> (usize, u8) {
        match row {
            Row::Note(n) => (self.notes[n], 0),
            Row::Line(l) => (l, 1),
            Row::Placeholder(f) => (self.folds[f].start, 2),
        }
    }

    /// The buffer line row `row` stands for: its line, a note's line, or a placeholder's fold
    /// signature (the last row's when `row` is past the end).
    pub fn line_at_row(&self, row: usize) -> usize {
        self.rows
            .get(row.min(self.rows.len().saturating_sub(1)))
            .map_or(0, |r| self.key(*r).0)
    }

    /// The row showing buffer line `line`, or None while it is hidden by a fold.
    pub fn row_of_line(&self, line: usize) -> Option<usize> {
        self.rows
            .binary_search_by_key(&(line, 1), |r| self.key(*r))
            .ok()
    }

    /// The row to scroll to for `line`: its own row, or the collapsed fold's signature.
    pub fn row_near_line(&self, line: usize) -> usize {
        let ix = self.rows.partition_point(|r| self.key(*r) <= (line, 1));
        ix.saturating_sub(1)
    }

    /// Opens every collapsed fold that hides `line`. Returns whether anything changed.
    pub fn reveal(&mut self, line: usize) -> bool {
        let hiding: Vec<usize> = self
            .folds
            .iter()
            .filter(|f| f.start < line && line < f.end && self.collapsed.contains(&f.start))
            .map(|f| f.start)
            .collect();
        for s in &hiding {
            self.collapsed.remove(s);
        }
        if !hiding.is_empty() {
            self.rebuild();
        }
        !hiding.is_empty()
    }

    /// The nearest visible buffer line from `line` in direction `delta` (+1 / -1),
    /// skipping placeholders and folded lines. None at the ends.
    pub fn step(&self, line: usize, delta: isize) -> Option<usize> {
        let row = self.row_near_line(line);
        let mut ix = row as isize + delta;
        while ix >= 0 && (ix as usize) < self.rows.len() {
            if let Row::Line(l) = self.rows[ix as usize] {
                return Some(l);
            }
            ix += delta;
        }
        None
    }

    /// The line to select when a fold is collapsed over the current selection.
    pub fn visible_line(&self, line: usize) -> usize {
        match self.rows[self.row_near_line(line)] {
            Row::Line(l) => l,
            Row::Placeholder(f) => self.folds[f].start,
            Row::Note(n) => self.notes[n],
        }
    }
}

impl Folds {
    /// The folds of a large JSON file, found after it showed (`Document::folds_later`), in
    /// place of the empty ones it showed with. Collapsed start lines remembered for the file and
    /// note strips carry over; `fold_all` is a Fold All (`Some(true)`) or Unfold All
    /// (`Some(false)`) pressed while the folds were not ready, which runs now.
    pub fn arrived(&self, folds: Vec<Fold>, fold_all: Option<bool>) -> Folds {
        let collapsed = match fold_all {
            Some(true) => folds.iter().map(|f| f.start).collect(),
            Some(false) => HashSet::new(),
            None => self.collapsed.clone(),
        };
        let mut out = Folds::new(folds, self.line_count, collapsed);
        out.set_notes(self.notes.clone());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_folds_keep_the_state_and_run_a_fold_all_pressed_while_waiting() {
        let late = vec![Fold { start: 0, end: 9 }, Fold { start: 2, end: 6 }];
        // Shown without folds, with the file's remembered collapse of line 2.
        let shown = Folds::new(Vec::new(), 10, [2].into_iter().collect());
        assert_eq!(shown.rows().len(), 10);
        let f = shown.arrived(late.clone(), None);
        assert_eq!(f.folds(), late.as_slice());
        assert!(!f.is_collapsed(0) && f.is_collapsed(1), "remembered");
        let all = shown.arrived(late.clone(), Some(true));
        assert!(
            all.is_collapsed(0) && all.is_collapsed(1),
            "Fold All, pressed early"
        );
        assert_eq!(
            all.rows(),
            [Row::Line(0), Row::Placeholder(0), Row::Line(9)]
        );
        let none = shown.arrived(late, Some(false));
        assert!(none.collapsed().is_empty(), "Unfold All, pressed early");
    }

    #[test]
    fn a_row_maps_back_to_its_line() {
        // Lines 0..10, a fold 2..6 collapsed: rows 0 1 2 [⋯] 6 7 8 9.
        let f = Folds::new(
            vec![Fold { start: 2, end: 6 }],
            10,
            [2].into_iter().collect(),
        );
        let lines: Vec<usize> = (0..f.rows().len()).map(|r| f.line_at_row(r)).collect();
        assert_eq!(lines, [0, 1, 2, 2, 6, 7, 8, 9]);
        assert_eq!(f.line_at_row(99), 9);
        for (row, line) in lines.iter().enumerate().filter(|(r, _)| *r != 3) {
            assert_eq!(f.row_near_line(*line), row);
        }
    }

    #[test]
    fn note_rows_sit_above_their_lines_and_hide_with_them() {
        let f = folds_from_spans([(1, 5)]);
        let mut f = Folds::new(f, 8, HashSet::new());
        assert!(f.set_notes(vec![6, 0, 2]));
        assert!(!f.set_notes(vec![0, 2, 6]), "same notes, same rows");
        assert_eq!(&f.rows()[..3], &[Row::Note(0), Row::Line(0), Row::Line(1)]);
        assert_eq!(f.row_of_line(2), Some(4), "line 2 comes after its note");
        assert_eq!(f.rows()[3], Row::Note(1));
        assert_eq!(f.step(1, 1), Some(2), "stepping skips notes");
        f.toggle(0); // fold 1..=5 hides line 2 and its note
        assert!(!f.rows().contains(&Row::Note(1)));
        assert_eq!(
            f.row_of_line(6).map(|r| f.rows()[r - 1]),
            Some(Row::Note(2))
        );
    }

    fn folds() -> Folds {
        // 12 lines; folds 1..=5 and 7..=11; (8, 10) and (3, 4) are too short.
        let f = folds_from_spans([(1, 5), (7, 11), (8, 10), (3, 4)]);
        let all = Folds::default_state(&f, true);
        Folds::new(f, 12, all)
    }

    #[test]
    fn left_and_right_fold_and_move_like_a_tree() {
        // 0 impl { (0-12), 1 fn { (1-11), 3 if { (3-6, folded), 6 } else { (6-10)
        let list = folds_from_spans([(0, 12), (1, 11), (3, 6), (6, 10)]);
        let mut f = Folds::new(list, 13, HashSet::from([3]));
        assert_eq!(f.left(1), Some(FoldKey::Toggle(1)), "open fold: fold it");
        assert_eq!(f.left(2), Some(FoldKey::GoTo(1)), "inside: go to its block");
        assert_eq!(
            f.left(3),
            Some(FoldKey::GoTo(1)),
            "on a folded block: go up"
        );
        assert_eq!(
            f.left(12),
            Some(FoldKey::GoTo(0)),
            "a closing line belongs to its block"
        );
        assert_eq!(
            f.left(6),
            Some(FoldKey::Toggle(3)),
            "`}} else {{` folds the else"
        );
        assert_eq!(f.right(3), Some(FoldKey::Toggle(2)), "folded: unfold");
        assert_eq!(f.right(1), Some(FoldKey::GoTo(2)), "open: step in");
        assert_eq!(f.right(2), None);
        f.toggle(0);
        assert_eq!(
            f.left(0),
            None,
            "a folded top-level block has nowhere to go"
        );
    }

    #[test]
    fn a_fold_closing_with_its_enclosing_fold_is_dropped() {
        // A signature over two lines (0-1) and its body block (1-6) end together.
        let f = folds_from_spans([(0, 6), (1, 6), (2, 5)]);
        let spans: Vec<_> = f.iter().map(|f| (f.start, f.end)).collect();
        assert_eq!(spans, [(0, 6), (2, 5)]);
    }

    #[test]
    fn short_spans_are_not_folds_and_duplicates_collapse() {
        let f = folds_from_spans([(0, 2), (4, 8), (4, 9)]);
        assert_eq!(f, vec![Fold { start: 4, end: 9 }]);
        assert!(folds_from_spans([(2, 4)]).is_empty(), "3 lines is not > 3");
    }

    #[test]
    fn collapsed_rows_show_signature_placeholder_closing() {
        let f = folds();
        assert_eq!(f.folds().len(), 2, "3-line spans are not folded");
        let rows = f.rows();
        assert_eq!(
            rows,
            &[
                Row::Line(0),
                Row::Line(1),
                Row::Placeholder(0),
                Row::Line(5),
                Row::Line(6),
                Row::Line(7),
                Row::Placeholder(1),
                Row::Line(11),
            ]
        );
        assert_eq!(f.folds()[0].hidden_lines(), 3);
    }

    #[test]
    fn toggle_expands_and_collapses() {
        let mut f = folds();
        assert!(f.toggle(0));
        assert_eq!(f.rows().len(), 8 + 3 - 1); // lines 2,3,4 back, placeholder gone
        assert!(f.toggle(0));
        assert_eq!(f.rows().len(), 8);
        assert!(!f.toggle(99));
    }

    #[test]
    fn no_fold_shows_every_line() {
        let list = folds_from_spans([(1, 5)]);
        let f = Folds::new(list.clone(), 8, Folds::default_state(&list, false));
        assert_eq!(f.rows().len(), 8);
        assert!(f.rows().iter().all(|r| matches!(r, Row::Line(_))));
    }

    #[test]
    fn generation_changes_with_the_rows_and_is_unique() {
        let mut f = folds();
        let (a, b) = (f.generation(), folds().generation());
        assert_ne!(a, b, "two fold states never share a generation");
        f.toggle(0);
        assert_ne!(f.generation(), a);
        let after = f.generation();
        assert_eq!(f.clone().generation(), after, "a copy draws the same rows");
    }

    #[test]
    fn row_of_line_and_hidden_lines() {
        let f = folds();
        assert_eq!(f.row_of_line(1), Some(1));
        assert_eq!(f.row_of_line(3), None);
        assert_eq!(f.row_of_line(5), Some(3));
        assert_eq!(
            f.row_near_line(3),
            2,
            "hidden line maps to its placeholder row"
        );
        assert_eq!(f.visible_line(3), 1);
    }

    #[test]
    fn reveal_opens_enclosing_fold_only() {
        let mut f = folds();
        assert!(!f.reveal(5), "closing line is visible");
        assert!(f.reveal(3));
        assert!(!f.is_collapsed(0));
        assert!(f.is_collapsed(1));
        assert_eq!(f.row_of_line(3), Some(3));
    }

    #[test]
    fn step_skips_folded_lines_and_placeholders() {
        let f = folds();
        assert_eq!(f.step(1, 1), Some(5));
        assert_eq!(f.step(5, -1), Some(1));
        assert_eq!(f.step(0, -1), None);
        assert_eq!(f.step(11, 1), None);
        assert_eq!(f.step(7, 1), Some(11));
    }

    #[test]
    fn set_all_matches_toggling_each_fold() {
        let list = folds_from_spans([(0, 9), (2, 7), (3, 5)]);
        let mut one = Folds::new(list.clone(), 10, HashSet::new());
        let mut each = one.clone();
        assert!(one.set_all(true));
        for i in 0..list.len() {
            each.toggle(i);
        }
        assert_eq!(one.rows(), each.rows());
        assert!(!one.set_all(true), "already folded");
        assert!(one.set_all(false));
        assert_eq!(one.rows().len(), 10);
        assert!(!one.set_all(false), "already open");
    }

    /// Fold All and Unfold All on a JSON file like a 1.3 MB event log (about 60,000 lines,
    /// 15,000 folds): one rebuild, not one per fold. `cargo test --release -- --ignored`.
    #[test]
    #[ignore]
    fn time_set_all_on_a_large_file() {
        let mut spans = vec![(0, 59_999), (2, 59_998)];
        for e in 0..5_000 {
            let s = 3 + e * 12;
            spans.extend([(s, s + 11), (s + 3, s + 7), (s + 8, s + 10)]);
        }
        let list = folds_from_spans(spans);
        let mut f = Folds::new(list.clone(), 60_000, HashSet::new());
        let t = std::time::Instant::now();
        f.set_all(true);
        f.set_all(false);
        eprintln!("{} folds: set_all both ways {:?}", list.len(), t.elapsed());
        let t = std::time::Instant::now();
        for i in 0..list.len() {
            f.toggle(i);
        }
        for i in 0..list.len() {
            f.toggle(i);
        }
        eprintln!("toggling each fold both ways {:?}", t.elapsed());
    }

    #[test]
    fn nested_folds_hide_inner_signature_while_outer_is_collapsed() {
        let list = folds_from_spans([(0, 9), (2, 7)]);
        let mut f = Folds::new(list.clone(), 10, Folds::default_state(&list, true));
        assert_eq!(f.rows(), &[Row::Line(0), Row::Placeholder(0), Row::Line(9)]);
        f.toggle(0);
        assert_eq!(f.rows().len(), 7); // 0,1,2,placeholder,7,8,9: inner stays collapsed
        assert!(f.rows().contains(&Row::Placeholder(1)));
        assert_eq!(f.fold_at(2), Some(1));
        assert!(f.reveal(4));
        assert!(!f.rows().contains(&Row::Placeholder(1)));
    }

    /// xorshift64*, for generated spans.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n.max(1) as u64) as usize
        }
    }

    /// Property test (the `folds` fuzz target runs the same checks on parsed source): for
    /// any spans, nested, shared-line or crossing, and any collapsed set, the rows show every
    /// line once or hide it under one placeholder, in order; Left and Right stay in range;
    /// and pressing Left repeatedly reaches top level.
    #[test]
    fn rows_and_left_right_hold_for_generated_spans() {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for _ in 0..3_000 {
            let lines = 1 + rng.below(40);
            let spans: Vec<_> = (0..rng.below(12))
                .map(|_| {
                    let s = rng.below(lines);
                    (s, s + rng.below(lines - s))
                })
                .collect();
            let list = folds_from_spans(spans);
            let collapsed = list
                .iter()
                .filter(|_| rng.below(2) == 0)
                .map(|f| f.start)
                .collect();
            let mut f = Folds::new(list.clone(), lines, collapsed);
            for _ in 0..4 {
                let mut next = 0;
                for &row in f.rows() {
                    match row {
                        Row::Line(l) => {
                            assert_eq!(l, next, "{list:?} {:?}", f.rows());
                            next += 1;
                        }
                        Row::Placeholder(i) => {
                            assert!(f.is_collapsed(i) && next == list[i].start + 1);
                            next = list[i].end;
                        }
                        Row::Note(_) => unreachable!("no notes set"),
                    }
                }
                assert_eq!(next, lines, "{list:?} {:?}", f.rows());
                for l in 0..lines {
                    for key in [f.left(l), f.right(l)].into_iter().flatten() {
                        match key {
                            FoldKey::Toggle(i) => assert!(i < list.len()),
                            FoldKey::GoTo(to) => assert!(to < lines),
                        }
                    }
                    let (mut tree, mut at, mut steps) = (f.clone(), l, 0);
                    while let Some(key) = tree.left(at) {
                        match key {
                            FoldKey::Toggle(i) => assert!(tree.toggle(i)),
                            FoldKey::GoTo(to) => {
                                assert!(to < at);
                                at = to;
                            }
                        }
                        steps += 1;
                        assert!(steps <= 2 * list.len() + 1, "{list:?} from {l}");
                    }
                }
                if !list.is_empty() {
                    f.toggle(rng.below(list.len()));
                }
            }
        }
    }

    #[test]
    fn fold_at_finds_signatures() {
        let f = folds();
        assert_eq!(f.fold_at(7), Some(1));
        assert_eq!(f.fold_at(6), None);
    }
}

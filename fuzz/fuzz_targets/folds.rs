#![no_main]
//! Syntax folds (`cho_syntax::analyze(..).folds`) and the viewer's fold model
//! (`app/src/fold.rs`, compiled in here by path because the app is a binary crate).
//!
//! Input: a selector byte (TypeScript, TSX and JavaScript three times in four, otherwise any
//! language), a byte of collapse choices, then the source.

use libfuzzer_sys::fuzz_target;
use cho_core::SourceBuffer;
use cho_syntax::{Language, SymbolKind, analyze, outline};
use std::collections::HashSet;

#[allow(dead_code)]
#[path = "../../app/src/fold.rs"]
mod fold;

use fold::{FoldKey, Folds, Row, folds_from_spans};

fn language(sel: u8) -> Language {
    const TS: [Language; 3] = [Language::TypeScript, Language::Tsx, Language::JavaScript];
    if sel < 192 {
        TS[sel as usize % 3]
    } else {
        Language::ALL[sel as usize % Language::ALL.len()]
    }
}

fuzz_target!(|data: &[u8]| {
    let [sel, pick, rest @ ..] = data else { return };
    let lang = language(*sel);
    let Ok(buffer) = SourceBuffer::from_bytes(rest.to_vec()) else {
        return;
    };
    let src = buffer.text();
    let Some(analysis) = analyze(lang, src) else {
        return;
    };

    // Byte level: in bounds, on char boundaries, non-empty, sorted, no duplicates, and any
    // two either nest or are disjoint.
    let folds = &analysis.folds;
    for r in folds {
        assert!(
            r.start < r.end && r.end <= src.len(),
            "bounds {r:?} len {}",
            src.len()
        );
        assert!(
            src.is_char_boundary(r.start) && src.is_char_boundary(r.end),
            "{r:?}"
        );
    }
    for w in folds.windows(2) {
        let key = |r: &std::ops::Range<usize>| (r.start, std::cmp::Reverse(r.end));
        assert!(
            key(&w[0]) < key(&w[1]),
            "unsorted or duplicate {:?} {:?}",
            w[0],
            w[1]
        );
    }
    for (i, a) in folds.iter().enumerate() {
        for b in &folds[i + 1..] {
            // `b` starts at or after `a` (sorted): it nests in `a` or starts after it ends.
            assert!(
                b.end <= a.end || b.start >= a.end,
                "folds cross: {a:?} {b:?}"
            );
        }
    }

    // Lines, as `Document::from_bytes` builds them: function bodies from the outline
    // (signature line to closing line) and every syntax fold.
    let line = |b: usize| buffer.line_of(b);
    let symbols = outline(lang, src);
    let bodies = symbols
        .iter()
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .map(|s| {
            (
                line(s.name_range.start),
                line(s.range.end.saturating_sub(1)),
            )
        });
    let regions = folds
        .iter()
        .map(|r| (line(r.start), line(r.end.saturating_sub(1))));
    let list = folds_from_spans(bodies.chain(regions));
    let line_count = buffer.line_count();
    for w in list.windows(2) {
        assert!(
            w[0].start < w[1].start,
            "fold starts not increasing {list:?}"
        );
    }
    for f in &list {
        assert!(f.end < line_count, "fold {f:?} past {line_count} lines");
        assert!(
            f.end - f.start + 1 > fold::MIN_FOLD_LINES,
            "{f:?} too short"
        );
    }

    // Collapse a subset chosen by `pick` (a rotating bit pattern) and check the rows.
    let collapsed: HashSet<usize> = list
        .iter()
        .enumerate()
        .filter(|(i, _)| pick.rotate_left(*i as u32 % 8) & 1 == 1)
        .map(|(_, f)| f.start)
        .collect();
    let mut model = Folds::new(list.clone(), line_count, collapsed);
    check_rows(&model, line_count);

    // Left and Right never leave the file or name a missing fold.
    for l in 0..line_count {
        for key in [model.left(l), model.right(l)].into_iter().flatten() {
            match key {
                FoldKey::Toggle(f) => assert!(f < list.len(), "toggle {f} of {}", list.len()),
                FoldKey::GoTo(to) => assert!(to < line_count, "goto {to} of {line_count}"),
            }
        }
        if let Some(FoldKey::GoTo(to)) = model.right(l) {
            assert_eq!(to, l + 1, "Right steps into the body");
        }
    }
    // Pressing Left over and over (folding what it folds) from any line ends at top level
    // within two steps per fold.
    for l in 0..line_count.min(64) {
        let mut tree = model.clone();
        let mut at = l;
        let mut steps = 0;
        while let Some(key) = tree.left(at) {
            match key {
                FoldKey::Toggle(f) => {
                    assert!(!tree.is_collapsed(f), "Left only folds an open fold");
                    tree.toggle(f);
                }
                FoldKey::GoTo(to) => {
                    assert!(to < at, "Left goes up: {at} -> {to}");
                    at = to;
                }
            }
            steps += 1;
            assert!(
                steps <= 2 * list.len() + 1,
                "Left from {l} does not reach top level"
            );
        }
    }

    // Toggling with Left/Right keeps the rows valid.
    for (i, &b) in rest.iter().take(16).enumerate() {
        let l = (b as usize * 7 + i) % line_count;
        let key = if b & 1 == 0 {
            model.left(l)
        } else {
            model.right(l)
        };
        if let Some(FoldKey::Toggle(f)) = key {
            assert!(model.toggle(f));
            check_rows(&model, line_count);
        }
    }
});

/// Rows cover every line exactly once, in order: each line is a `Row::Line` or hidden
/// under exactly one placeholder (the lines strictly between its fold's start and end).
fn check_rows(model: &Folds, line_count: usize) {
    let mut next = 0;
    let mut prev: Option<Row> = None;
    for &row in model.rows() {
        match row {
            Row::Line(l) => {
                assert_eq!(
                    l,
                    next,
                    "row for line {l}, expected {next}: {:?}",
                    model.rows()
                );
                next += 1;
            }
            Row::Placeholder(f) => {
                let fold = model.folds()[f];
                assert_eq!(
                    prev,
                    Some(Row::Line(fold.start)),
                    "placeholder after its signature"
                );
                assert!(model.is_collapsed(f));
                assert_eq!(next, fold.start + 1);
                next = fold.end;
            }
        }
        prev = Some(row);
    }
    assert_eq!(next, line_count, "rows end at {next} of {line_count}");
}

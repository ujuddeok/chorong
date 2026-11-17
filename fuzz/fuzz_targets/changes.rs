#![no_main]
//! Symbol-level changes between two versions of a file (`cho changes`, the Changes view),
//! with and without a committed version in between. Input: a selector byte (file kind and
//! whether a committed version is given), then base, head and committed texts split at
//! `0xff`.
use libfuzzer_sys::fuzz_target;
use cho_changes::{FileChanges, SymbolStatus, compare_texts, split_hunks};
use cho_git::linediff::{DEFAULT_MAX_COST, Lines, diff};

const PATHS: [&str; 7] = ["a.rs", "a.ts", "a.tsx", "a.js", "a.py", "a.swift", "a.txt"];

fn check_side(text: &[u8], decl: &cho_changes::Decl) {
    let s = std::str::from_utf8(text).expect("symbols only for UTF-8");
    for r in [decl.range, decl.name_range] {
        assert!(r[0] <= r[1] && r[1] <= s.len(), "{r:?} len {}", s.len());
        assert!(
            s.is_char_boundary(r[0]) && s.is_char_boundary(r[1]),
            "{r:?}"
        );
    }
    assert!(
        decl.start_line >= 1 && decl.start_line <= decl.end_line,
        "{decl:?}"
    );
}

fn check(fc: &FileChanges, base: &[u8], head: &[u8]) {
    for h in &fc.hunks {
        for (span, text) in [(&h.base, base), (&h.head, head)] {
            assert!(
                span.bytes[0] <= span.bytes[1] && span.bytes[1] <= text.len(),
                "{span:?}"
            );
            assert_eq!(
                span.line_count == 0,
                span.bytes[0] == span.bytes[1],
                "{span:?}"
            );
        }
    }
    for s in &fc.symbols {
        let expected = if s.base.len() > 1 || s.head.len() > 1 {
            SymbolStatus::Ambiguous
        } else if s.base.is_empty() {
            SymbolStatus::Added
        } else if s.head.is_empty() {
            SymbolStatus::Removed
        } else {
            SymbolStatus::Changed
        };
        assert_eq!(s.status, expected, "{s:?}");
        assert!(!s.qualified_name.is_empty(), "{s:?}");
        assert!(
            s.base.iter().chain(&s.head).any(|d| d.touched),
            "untouched {s:?}"
        );
        s.base.iter().for_each(|d| check_side(base, d));
        s.head.iter().for_each(|d| check_side(head, d));
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let path = PATHS[(sel & 0x7f) as usize % PATHS.len()];
    let mut parts = rest.splitn(3, |&b| b == 0xff);
    let base = parts.next().unwrap_or_default();
    let head = parts.next().unwrap_or_default();
    let committed = (sel & 0x80 != 0).then(|| parts.next().unwrap_or_default());

    let fc = compare_texts(path, base, head, committed);
    check(&fc, base, head);
    if base == head {
        assert!(
            fc.hunks.is_empty() && fc.symbols.is_empty(),
            "no change, yet {fc:?}"
        );
    }
    if let Some(c) = committed {
        if c == head {
            // Nothing uncommitted.
            assert!(fc.hunks.iter().all(|h| !h.uncommitted), "{fc:?}");
            assert!(fc.symbols.iter().all(|s| !s.uncommitted), "{fc:?}");
        }
        if c == base {
            // Nothing committed.
            assert!(fc.hunks.iter().all(|h| h.uncommitted), "{fc:?}");
        }

        // The split on its own: pieces tile each base-to-working hunk in order, none
        // empty, and no working line HEAD-to-working changed is called committed.
        let (bl, hl, cl) = (Lines::new(base), Lines::new(head), Lines::new(c));
        let bw = diff(&bl, &hl, DEFAULT_MAX_COST).hunks;
        let bh = diff(&bl, &cl, DEFAULT_MAX_COST).hunks;
        let hw = diff(&cl, &hl, DEFAULT_MAX_COST).hunks;
        let pieces = split_hunks(&bw, &bh, &hw);
        let mut it = pieces.iter();
        for h in &bw {
            let (mut b, mut w) = (h.base.start, h.head.start);
            while b < h.base.end || w < h.head.end {
                let p = it
                    .next()
                    .unwrap_or_else(|| panic!("hunk {h:?} not covered"));
                assert_eq!(
                    (p.base.start, p.head.start),
                    (b, w),
                    "gap before {p:?} in {h:?}"
                );
                assert!(
                    !p.base.is_empty() || !p.head.is_empty(),
                    "empty piece {p:?}"
                );
                assert!(
                    p.base.end <= h.base.end && p.head.end <= h.head.end,
                    "{p:?} {h:?}"
                );
                if !p.uncommitted {
                    for line in p.head.clone() {
                        assert!(
                            !hw.iter().any(|u| u.head.contains(&line)),
                            "uncommitted working line {line} in committed piece {p:?}"
                        );
                    }
                }
                (b, w) = (p.base.end, p.head.end);
            }
        }
        assert!(it.next().is_none(), "pieces beyond the hunks");
    }
});

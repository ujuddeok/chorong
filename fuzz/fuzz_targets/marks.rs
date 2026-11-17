#![no_main]
//! Highlight groups (`app/src/marks.rs`, compiled in here by path because the app is a
//! binary crate): name groups from real identifier tokens, symbol groups from fuzzed
//! occurrence lists, then `slots_in`, `in_range`, `occurrence_at`, `step_occurrence` and
//! `remove` against brute-force references.
//!
//! Input: a selector byte (TypeScript, TSX and JavaScript three times in four), a length
//! byte for the operation script, the script, then the source.

use libfuzzer_sys::fuzz_target;
use cho_syntax::{Language, analyze, outline};
use std::ops::Range;

#[allow(dead_code)]
#[path = "../../app/src/marks.rs"]
mod marks;

use marks::{Marks, Source, enclosing, identifier_at, occurrence_at, step_occurrence};

fn language(sel: u8) -> Language {
    const TS: [Language; 3] = [Language::TypeScript, Language::Tsx, Language::JavaScript];
    if sel < 192 {
        TS[sel as usize % 3]
    } else {
        Language::ALL[sel as usize % Language::ALL.len()]
    }
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && a.end > b.start
}

fn brute_slots(m: &Marks, range: &Range<usize>) -> Vec<(usize, bool)> {
    m.groups()
        .iter()
        .filter(|g| g.occurrences.iter().any(|o| overlaps(o, range)))
        .map(|g| (g.slot, g.declarations.iter().any(|d| overlaps(d, range))))
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let [sel, len, rest @ ..] = data else { return };
    let (script, text) = rest.split_at((*len as usize).min(rest.len()));
    let lang = language(*sel);
    let src = String::from_utf8_lossy(text);
    let src = src.as_ref();
    let Some(analysis) = analyze(lang, src) else {
        return;
    };
    let ids = &analysis.identifiers;
    for w in ids.windows(2) {
        assert!(
            w[0].end <= w[1].start,
            "identifiers overlap {:?} {:?}",
            w[0],
            w[1]
        );
    }
    let symbols = outline(lang, src);
    let source = Source {
        identifiers: ids,
        bindings: &analysis.bindings,
        text: src,
    };
    let n = src.len() + 1;
    let palette = 1 + (*sel as usize % 6);

    let mut m = Marks::default();
    let mut wrapped = false;
    let mut ops = script.chunks_exact(3);
    for op in ops.by_ref() {
        let (a, b) = (op[1] as usize, op[2] as usize);
        match op[0] % 5 {
            // Double-click an identifier: a name group in its innermost declaration.
            0 if !ids.is_empty() => {
                let byte = ids[(a << 8 | b) % ids.len()].start;
                let at = identifier_at(ids, byte).expect("identifier at its own start");
                let scope = enclosing(&symbols, at.start).map_or(0..src.len(), |s| s.range.clone());
                let before = m.groups().len();
                m.add_name(&source, &at, scope.clone(), "scope", palette);
                assert_eq!(m.groups().len(), before + 1);
                let g = m.groups().last().unwrap();
                let name = &src[at.clone()];
                assert!(
                    g.occurrences.contains(&at)
                        || !(scope.start <= at.start && at.end <= scope.end)
                );
                for o in &g.occurrences {
                    assert_eq!(&src[o.clone()], name);
                    assert!(scope.start <= o.start && o.end <= scope.end);
                }
                assert!(g.declarations.len() <= 1);
            }
            // A symbol group from an index answer: real identifier tokens, in any order.
            1 if !ids.is_empty() => {
                let occ: Vec<_> = (0..1 + a % 8)
                    .map(|i| ids[(b * 31 + i * (a + 1)) % ids.len()].clone())
                    .collect();
                let defs = occ.iter().take(b % 2).cloned().collect();
                m.add_symbol("s".into(), "x".into(), occ, defs, palette);
            }
            // Remove a group by index: the others stay, in order.
            2 => {
                let before: Vec<_> = m.groups().to_vec();
                let i = a % (before.len() + 1);
                assert_eq!(m.remove(i), i < before.len());
                let mut want = before;
                if i < want.len() {
                    want.remove(i);
                }
                assert_eq!(m.groups(), want.as_slice());
            }
            // A malformed index answer: arbitrary ranges that may overlap, nest, repeat or be
            // empty. The group covers exactly the bytes they cover (checked below with the
            // other groups: sorted and disjoint).
            4 => {
                let mut seed = (a << 8 | b) as u64 | 1;
                let mut next = |m: usize| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    (seed >> 33) as usize % m.max(1)
                };
                let occ: Vec<Range<usize>> = (0..1 + a % 6)
                    .map(|_| {
                        let s = next(n);
                        s..s + next(12)
                    })
                    .collect();
                let defs = occ.iter().take(b % 3).cloned().collect();
                m.add_symbol("s".into(), "x".into(), occ.clone(), defs, palette);
                let g = m.groups().last().unwrap();
                let hi = occ.iter().map(|r| r.end).max().unwrap_or(0);
                for byte in 0..hi {
                    let want = occ.iter().any(|r| r.contains(&byte));
                    let got = g.occurrences.iter().any(|r| r.contains(&byte));
                    assert_eq!(got, want, "byte {byte}: {occ:?} -> {:?}", g.occurrences);
                }
                assert!(g.occurrences.iter().all(|r| r.start < r.end));
            }
            _ => {}
        }
        for g in m.groups() {
            assert!(
                g.occurrences.windows(2).all(|w| w[0].end <= w[1].start),
                "{g:?}"
            );
        }
        // Slots stay distinct until the palette has run out once (wrapped slots repeat).
        wrapped |= m.groups().len() > palette;
        if !wrapped {
            let mut slots: Vec<_> = m.groups().iter().map(|g| g.slot % palette).collect();
            slots.sort();
            slots.dedup();
            assert_eq!(
                slots.len(),
                m.groups().len(),
                "slots repeat before the palette runs out"
            );
        }
    }

    // `slots_in` and `in_range` against brute force, over every line and fuzzed ranges.
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut at = 0;
    for line in src.split_inclusive('\n') {
        ranges.push(at..at + line.len());
        at += line.len();
    }
    for pair in ops
        .remainder()
        .chunks(2)
        .chain(script.chunks_exact(2))
        .take(32)
    {
        let s = pair[0] as usize * n / 256;
        let e = s + pair.get(1).copied().unwrap_or(0) as usize % 64;
        ranges.push(s..e);
    }
    for r in &ranges {
        assert_eq!(m.slots_in(r), brute_slots(&m, r), "slots_in {r:?}");
        let mut want: Vec<_> = m
            .groups()
            .iter()
            .flat_map(|g| {
                g.occurrences
                    .iter()
                    .filter(|o| o.start < r.end && o.end > r.start)
                    .map(|o| {
                        (
                            g.slot,
                            o.clone(),
                            g.declarations.iter().any(|d| overlaps(d, o)),
                        )
                    })
            })
            .collect();
        want.sort_by_key(|(_, o, _)| o.start);
        assert_eq!(m.in_range(r), want, "in_range {r:?}");
    }

    // The caret helpers on each group's occurrences, at every byte up to a bound.
    for g in m.groups() {
        let occ = &g.occurrences;
        for caret in (0..n).step_by(1 + n / 512) {
            let on = occurrence_at(occ, caret);
            let want = occ.iter().rposition(|r| r.start <= caret && caret <= r.end);
            assert_eq!(on, want);
            for forward in [true, false] {
                match step_occurrence(occ, caret, forward) {
                    Some(i) => assert!(i < occ.len()),
                    None => assert!(occ.is_empty()),
                }
            }
        }
    }
});

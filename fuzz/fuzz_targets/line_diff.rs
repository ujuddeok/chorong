#![no_main]
use libfuzzer_sys::fuzz_target;
use cho_git::linediff::{Lines, diff, similarity};

// Any two texts: the hunks are in order, non-empty, and every line outside them is equal on
// both sides, with or without the cost limit.
fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else { return };
    let cut = rest.iter().position(|&b| b == 0xff).unwrap_or(rest.len() / 2);
    let (a, b) = (&rest[..cut], rest.get(cut + 1..).unwrap_or_default());
    let (la, lb) = (Lines::new(a), Lines::new(b));
    let cost = if sel & 1 == 0 { 2000 } else { (sel >> 1) as usize };
    let d = diff(&la, &lb, cost);
    let (mut x, mut y) = (0, 0);
    for h in &d.hunks {
        assert!(!h.base.is_empty() || !h.head.is_empty(), "empty hunk {h:?}");
        assert!(h.base.start >= x && h.head.start >= y, "out of order {h:?}");
        assert_eq!(h.base.start - x, h.head.start - y);
        for k in 0..h.base.start - x {
            assert_eq!(la.line(x + k), lb.line(y + k));
        }
        assert!(h.base.end <= la.len() && h.head.end <= lb.len());
        x = h.base.end;
        y = h.head.end;
    }
    assert_eq!(la.len() - x, lb.len() - y);
    for k in 0..la.len() - x {
        assert_eq!(la.line(x + k), lb.line(y + k));
    }
    assert!(similarity(a, b, cost) <= 100);
});

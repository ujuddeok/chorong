#![no_main]
use libfuzzer_sys::fuzz_target;
use cho_syntax::{Language, SymbolMatch, find_symbol, outline};

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else { return };
    let lang = Language::ALL[sel as usize % Language::ALL.len()];
    let src = String::from_utf8_lossy(rest);
    let symbols = outline(lang, &src);
    for s in &symbols {
        for r in [&s.range, &s.name_range] {
            assert!(r.start <= r.end && r.end <= src.len(), "bounds {r:?} len {}", src.len());
            assert!(src.is_char_boundary(r.start) && src.is_char_boundary(r.end), "{r:?}");
        }
        assert!(
            s.name_range.start >= s.range.start && s.name_range.end <= s.range.end,
            "name_range {:?} outside range {:?} ({})", s.name_range, s.range, s.name
        );
    }
    // Limit lookups: find_symbol re-runs the outline each time.
    for s in symbols.iter().take(4) {
        let m = find_symbol(lang, &src, &s.name);
        assert!(!matches!(m, SymbolMatch::NotFound), "symbol {:?} not found", s.name);
    }
});

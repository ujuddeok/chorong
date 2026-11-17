#![no_main]
use libfuzzer_sys::fuzz_target;
use cho_syntax::{Highlighter, Language};
use std::cell::RefCell;

thread_local! {
    static HL: RefCell<Highlighter> = RefCell::new(Highlighter::new());
}

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else { return };
    let lang = Language::ALL[sel as usize % Language::ALL.len()];
    let src = String::from_utf8_lossy(rest);
    let Ok(spans) = HL.with(|h| h.borrow_mut().highlight(lang, &src)) else { return };
    let mut prev_end = 0;
    for s in &spans {
        let r = &s.range;
        assert!(r.start < r.end || r.start == r.end, "inverted {r:?}");
        assert!(r.start >= prev_end, "unsorted/overlap: {r:?} after end {prev_end}");
        assert!(r.end <= src.len(), "out of bounds {r:?} len {}", src.len());
        assert!(src.is_char_boundary(r.start) && src.is_char_boundary(r.end), "{r:?}");
        prev_end = r.end;
    }
});

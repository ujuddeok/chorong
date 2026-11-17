#![no_main]
use libfuzzer_sys::fuzz_target;
use cho_core::{LineCol, SourceBuffer};

fuzz_target!(|data: &[u8]| {
    let Ok(buf) = SourceBuffer::from_bytes(data.to_vec()) else {
        return;
    };
    let text = buf.text();
    assert!(buf.line_count() >= 1);

    // clamp on arbitrary offsets derived from the input.
    let mut offsets: Vec<usize> = vec![0, text.len(), text.len() + 1, usize::MAX];
    offsets.extend(data.iter().map(|&b| b as usize * 7));
    for off in offsets {
        let c = buf.clamp(off);
        assert!(c <= buf.len());
        assert!(text.is_char_boundary(c));
    }

    // Round trips at every char boundary.
    for off in text.char_indices().map(|(i, _)| i).chain([text.len()]) {
        let lc = buf.byte_to_line_col(off);
        assert_eq!(buf.line_col_to_byte(lc), Some(off), "utf8 off={off}");
        let u = buf.byte_to_utf16(off);
        assert_eq!(buf.utf16_to_byte(u), Some(off), "utf16 off={off}");
        assert_eq!(u.line, lc.line);
    }

    for line in 0..buf.line_count() {
        let r = buf.line_range(line).expect("line in range");
        assert!(r.start <= r.end && r.end <= buf.len());
        assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
        assert!(buf.line_text(line).is_some());
    }
    assert!(buf.line_range(buf.line_count()).is_none());
    assert!(buf.line_col_to_byte(LineCol { line: buf.line_count(), col: 0 }).is_none());
});

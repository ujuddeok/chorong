//! An immutable, UTF-8 source file with precomputed line tables.

use crate::fs::MAX_SOURCE_BYTES;
use std::ops::Range;
use thiserror::Error;

/// How far into a file [`binary_offset`] looks for a NUL byte: git's value.
pub const BINARY_SNIFF_BYTES: usize = 8000;

/// The offset of the first NUL byte in the first [`BINARY_SNIFF_BYTES`], which is git's test
/// for a binary file; `None` for text.
pub fn binary_offset(bytes: &[u8]) -> Option<usize> {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)]
        .iter()
        .position(|&b| b == 0)
}

/// Whether `bytes` look binary (see [`binary_offset`]). Every "is this source text?" test in
/// Chorong uses this, so the viewer and the diff agree.
pub fn is_binary(bytes: &[u8]) -> bool {
    binary_offset(bytes).is_some()
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SourceError {
    #[error("file is {0} bytes, larger than the {MAX_SOURCE_BYTES}-byte limit")]
    TooLarge(usize),
    #[error("file is not valid UTF-8 (first invalid byte at offset {0})")]
    NotUtf8(usize),
    #[error("file looks binary (NUL byte at offset {0})")]
    Binary(usize),
}

/// A zero-based line and a zero-based column, both counted in the unit the
/// caller asked for (UTF-8 bytes or UTF-16 code units).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineCol {
    pub line: usize,
    pub col: usize,
}

/// The unit a column is counted in. SCIP documents declare one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnUnit {
    /// UTF-8 bytes.
    Utf8,
    /// UTF-16 code units.
    Utf16,
    /// Unicode scalar values (UTF-32 code units).
    Utf32,
}

/// Immutable source text plus line tables.
///
/// Line breaks are `\n`; a `\r` before it belongs to the line's content for
/// offset purposes but is excluded by [`SourceBuffer::line_text`].
#[derive(Debug, Clone)]
pub struct SourceBuffer {
    text: String,
    /// Byte offset of the start of each line. Always starts with `0`.
    line_starts: Vec<usize>,
    /// UTF-16 and character counts at char boundaries about every [`MARK_STEP`] bytes,
    /// so a column conversion scans at most that much of a line (minified files have
    /// megabyte lines and SCIP indexes give thousands of positions on them). Empty
    /// for ASCII text, where every unit is a byte.
    marks: Vec<Mark>,
}

/// Bytes between [`Mark`]s.
const MARK_STEP: usize = 1024;

#[derive(Debug, Clone, Copy)]
struct Mark {
    byte: usize,
    utf16: usize,
    chars: usize,
}

impl Mark {
    fn units(&self, unit: ColumnUnit) -> usize {
        match unit {
            ColumnUnit::Utf8 => self.byte,
            ColumnUnit::Utf16 => self.utf16,
            ColumnUnit::Utf32 => self.chars,
        }
    }
}

/// `ch`'s length in `unit`.
fn char_units(ch: char, unit: ColumnUnit) -> usize {
    match unit {
        ColumnUnit::Utf8 => ch.len_utf8(),
        ColumnUnit::Utf16 => ch.len_utf16(),
        ColumnUnit::Utf32 => 1,
    }
}

impl SourceBuffer {
    /// Validates and wraps raw file bytes.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, SourceError> {
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err(SourceError::TooLarge(bytes.len()));
        }
        if let Some(pos) = binary_offset(&bytes) {
            return Err(SourceError::Binary(pos));
        }
        let text = String::from_utf8(bytes)
            .map_err(|e| SourceError::NotUtf8(e.utf8_error().valid_up_to()))?;
        Ok(Self::new(text))
    }

    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        let mut marks = vec![];
        if !text.is_ascii() {
            let (mut utf16, mut chars, mut next) = (0, 0, 0);
            for (byte, ch) in text.char_indices() {
                if byte >= next {
                    marks.push(Mark { byte, utf16, chars });
                    next = byte + MARK_STEP;
                }
                utf16 += ch.len_utf16();
                chars += 1;
            }
        }
        Self {
            text,
            line_starts,
            marks,
        }
    }

    /// `unit`s from the start of the text to `byte` (a char boundary).
    fn units_at(&self, byte: usize, unit: ColumnUnit) -> usize {
        if self.marks.is_empty() || unit == ColumnUnit::Utf8 {
            return byte;
        }
        let m = self.marks[self.marks.partition_point(|m| m.byte <= byte) - 1];
        m.units(unit)
            + self.text[m.byte..byte]
                .chars()
                .map(|c| char_units(c, unit))
                .sum::<usize>()
    }

    /// Where to start scanning `range` (on one line) for the position `target` units from
    /// the start of the text: the last mark at or before it, if inside `range`, else the
    /// range's start or end. Returns the byte and its units; the units are at most `target`
    /// unless the whole range lies before it.
    fn scan_from(&self, range: &Range<usize>, target: usize, unit: ColumnUnit) -> (usize, usize) {
        let i = self.marks.partition_point(|m| m.units(unit) <= target);
        match i.checked_sub(1).map(|i| self.marks[i]) {
            Some(m) if m.byte > range.end => (range.end, self.units_at(range.end, unit)),
            Some(m) if m.byte > range.start => (m.byte, m.units(unit)),
            _ => (range.start, self.units_at(range.start, unit)),
        }
    }

    /// Column of `offset` (a char boundary) on `line`, in `unit`.
    fn column(&self, line: usize, offset: usize, unit: ColumnUnit) -> usize {
        self.units_at(offset, unit) - self.units_at(self.line_starts[line], unit)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Number of lines. A trailing newline starts a final, empty line.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte range of `line`, excluding its `\n` (and a `\r` before it).
    pub fn line_range(&self, line: usize) -> Option<Range<usize>> {
        let start = *self.line_starts.get(line)?;
        let mut end = self
            .line_starts
            .get(line + 1)
            .map(|next| next - 1)
            .unwrap_or(self.text.len());
        if end > start && self.text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        Some(start..end)
    }

    pub fn line_text(&self, line: usize) -> Option<&str> {
        self.line_range(line).map(|r| &self.text[r])
    }

    /// Moves `offset` back to the start of the character containing it, and
    /// clamps it to the end of the buffer.
    pub fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    /// The line containing byte `offset` (after clamping).
    pub fn line_of(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next - 1,
        }
    }

    /// Byte offset to line and UTF-8 byte column.
    pub fn byte_to_line_col(&self, offset: usize) -> LineCol {
        let offset = self.clamp(offset);
        let line = self.line_of(offset);
        LineCol {
            line,
            col: offset - self.line_starts[line],
        }
    }

    /// Every offset a column on `line` can address: from the line start up to
    /// and including its `\n` (or the end of the buffer on the last line). A
    /// `\r` before the `\n` is addressable, unlike in [`Self::line_range`].
    fn line_extent(&self, line: usize) -> Option<Range<usize>> {
        let start = *self.line_starts.get(line)?;
        let end = self
            .line_starts
            .get(line + 1)
            .map(|next| next - 1)
            .unwrap_or(self.text.len());
        Some(start..end)
    }

    /// Line and UTF-8 byte column to byte offset. Columns past the end of the
    /// line clamp to its newline; a missing line returns `None`. Inverse of
    /// [`Self::byte_to_line_col`] for every character boundary.
    pub fn line_col_to_byte(&self, pos: LineCol) -> Option<usize> {
        let range = self.line_extent(pos.line)?;
        Some(self.clamp((range.start + pos.col).min(range.end)))
    }

    /// Byte offset to line and UTF-16 column (the unit LSP-style indexes and
    /// many UI toolkits use).
    pub fn byte_to_utf16(&self, offset: usize) -> LineCol {
        let offset = self.clamp(offset);
        let line = self.line_of(offset);
        LineCol {
            line,
            col: self.column(line, offset, ColumnUnit::Utf16),
        }
    }

    /// Line and UTF-16 column to byte offset. A column inside a surrogate pair
    /// resolves to the start of that character; past the end clamps to the end.
    pub fn utf16_to_byte(&self, pos: LineCol) -> Option<usize> {
        let range = self.line_extent(pos.line)?;
        let unit = ColumnUnit::Utf16;
        let target = self.units_at(range.start, unit).saturating_add(pos.col);
        let (from, mut units) = self.scan_from(&range, target, unit);
        for (i, ch) in self.text[from..range.end].char_indices() {
            let next = units + ch.len_utf16();
            if next > target {
                return Some(from + i);
            }
            units = next;
        }
        Some(range.end)
    }

    /// Byte offset to line and column in `unit`.
    pub fn byte_to_line_col_in(&self, offset: usize, unit: ColumnUnit) -> LineCol {
        match unit {
            ColumnUnit::Utf8 => self.byte_to_line_col(offset),
            ColumnUnit::Utf16 => self.byte_to_utf16(offset),
            ColumnUnit::Utf32 => {
                let offset = self.clamp(offset);
                let line = self.line_of(offset);
                LineCol {
                    line,
                    col: self.column(line, offset, unit),
                }
            }
        }
    }

    /// Strict inverse of [`Self::byte_to_line_col_in`] for externally supplied
    /// positions: `None` if the line doesn't exist, the column is past the end
    /// of the line's content (`\r\n` excluded), or it falls inside a character.
    /// Unlike [`Self::line_col_to_byte`] nothing is clamped, so a position from
    /// an index that no longer matches the file is rejected, not moved.
    pub fn line_col_to_byte_exact(&self, pos: LineCol, unit: ColumnUnit) -> Option<usize> {
        let range = self.line_range(pos.line)?;
        match unit {
            ColumnUnit::Utf8 => {
                let text = &self.text[range.clone()];
                (pos.col <= text.len() && text.is_char_boundary(pos.col))
                    .then_some(range.start + pos.col)
            }
            ColumnUnit::Utf16 | ColumnUnit::Utf32 => {
                let target = self.units_at(range.start, unit).checked_add(pos.col)?;
                let (from, mut units) = self.scan_from(&range, target, unit);
                for (i, ch) in self.text[from..range.end].char_indices() {
                    if units == target {
                        return Some(from + i);
                    }
                    if units > target {
                        return None;
                    }
                    units += char_units(ch, unit);
                }
                (units == target).then_some(range.end)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_limit_is_inclusive() {
        let max = MAX_SOURCE_BYTES as usize;
        assert!(SourceBuffer::from_bytes(vec![b'a'; max]).is_ok());
        assert!(matches!(
            SourceBuffer::from_bytes(vec![b'a'; max + 1]),
            Err(SourceError::TooLarge(n)) if n == max + 1
        ));
    }

    #[test]
    fn lines_and_ranges() {
        let b = SourceBuffer::new("ab\r\ncd\n\nef");
        assert_eq!(b.line_count(), 4);
        assert_eq!(b.line_text(0), Some("ab"));
        assert_eq!(b.line_text(1), Some("cd"));
        assert_eq!(b.line_text(2), Some(""));
        assert_eq!(b.line_text(3), Some("ef"));
        assert_eq!(b.line_text(4), None);
    }

    #[test]
    fn crlf_offsets_round_trip() {
        // Found by fuzzing: the `\r` and `\n` of a CRLF must both round-trip.
        let b = SourceBuffer::new("a\r\nb\n\nlast");
        for offset in 0..=b.len() {
            assert_eq!(
                b.line_col_to_byte(b.byte_to_line_col(offset)),
                Some(offset),
                "utf8 {offset}"
            );
            assert_eq!(
                b.utf16_to_byte(b.byte_to_utf16(offset)),
                Some(offset),
                "utf16 {offset}"
            );
        }
        assert_eq!(b.line_text(0), Some("a"));
    }

    #[test]
    fn trailing_newline_makes_empty_last_line() {
        let b = SourceBuffer::new("x\n");
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line_text(1), Some(""));
    }

    #[test]
    fn clamp_moves_to_char_start() {
        let b = SourceBuffer::new("aé😀");
        // 'é' is bytes 1..3, '😀' is bytes 3..7.
        assert_eq!(b.clamp(2), 1);
        assert_eq!(b.clamp(5), 3);
        assert_eq!(b.clamp(100), 7);
    }

    #[test]
    fn byte_and_line_col_round_trip() {
        let b = SourceBuffer::new("fn a() {}\n    let x = 1;\n");
        let off = b.text().find("let").unwrap();
        let lc = b.byte_to_line_col(off);
        assert_eq!(lc, LineCol { line: 1, col: 4 });
        assert_eq!(b.line_col_to_byte(lc), Some(off));
        assert_eq!(b.line_col_to_byte(LineCol { line: 0, col: 999 }), Some(9));
        assert_eq!(b.line_col_to_byte(LineCol { line: 9, col: 0 }), None);
    }

    /// Found by the large-input fuzz runs: every column conversion scanned its line from
    /// the start, so positions on a megabyte line (a minified file, with an index giving
    /// thousands of them) took quadratic time. Marks bound each scan.
    #[test]
    fn long_lines_convert_exactly_and_quickly() {
        let line: String = (0..60_000).map(|i| ["a", "é", "😀", "漢"][i % 4]).collect();
        let b = SourceBuffer::new(format!("x\r\n{line}\r\n😀"));
        let started = std::time::Instant::now();
        let (mut u16, mut u32) = (0, 0);
        for (i, ch) in line.char_indices() {
            let off = 3 + i;
            let lc = LineCol { line: 1, col: u16 };
            assert_eq!(b.byte_to_utf16(off), lc);
            assert_eq!(b.utf16_to_byte(lc), Some(off));
            assert_eq!(b.line_col_to_byte_exact(lc, ColumnUnit::Utf16), Some(off));
            let lc32 = LineCol { line: 1, col: u32 };
            assert_eq!(b.byte_to_line_col_in(off, ColumnUnit::Utf32), lc32);
            assert_eq!(b.line_col_to_byte_exact(lc32, ColumnUnit::Utf32), Some(off));
            if ch.len_utf16() == 2 {
                let inside = LineCol {
                    line: 1,
                    col: u16 + 1,
                };
                assert_eq!(b.line_col_to_byte_exact(inside, ColumnUnit::Utf16), None);
                assert_eq!(b.utf16_to_byte(inside), Some(off));
            }
            u16 += ch.len_utf16();
            u32 += 1;
        }
        let end = 3 + line.len();
        let lc = LineCol { line: 1, col: u16 };
        assert_eq!(b.line_col_to_byte_exact(lc, ColumnUnit::Utf16), Some(end));
        let past = LineCol {
            line: 1,
            col: u16 + 1,
        };
        assert_eq!(b.line_col_to_byte_exact(past, ColumnUnit::Utf16), None);
        assert_eq!(b.utf16_to_byte(past), Some(end + 1)); // the `\n`
        assert_eq!(
            b.utf16_to_byte(LineCol {
                line: 1,
                col: usize::MAX
            }),
            Some(end + 1)
        );
        assert_eq!(b.byte_to_utf16(end + 3), LineCol { line: 2, col: 0 });
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn utf16_conversion_handles_astral_and_cjk() {
        let b = SourceBuffer::new("x😀漢y\n");
        let y = b.text().find('y').unwrap();
        // x=1 unit, 😀=2 units, 漢=1 unit.
        assert_eq!(b.byte_to_utf16(y), LineCol { line: 0, col: 4 });
        assert_eq!(b.utf16_to_byte(LineCol { line: 0, col: 4 }), Some(y));
        // Column 2 is inside the surrogate pair: resolves to the emoji's start.
        assert_eq!(b.utf16_to_byte(LineCol { line: 0, col: 2 }), Some(1));
    }

    #[test]
    fn combining_marks_count_per_scalar() {
        let b = SourceBuffer::new("e\u{301}x");
        let x = b.text().find('x').unwrap();
        assert_eq!(b.byte_to_utf16(x).col, 2);
    }

    #[test]
    fn rejects_binary_and_invalid_utf8() {
        assert_eq!(
            SourceBuffer::from_bytes(vec![b'a', 0, b'b']).unwrap_err(),
            SourceError::Binary(1)
        );
        assert_eq!(
            SourceBuffer::from_bytes(vec![b'a', 0xff]).unwrap_err(),
            SourceError::NotUtf8(1)
        );
        assert!(SourceBuffer::from_bytes(b"ok".to_vec()).is_ok());
    }

    /// The viewer and the diff (cho-git, cho-changes) call the same bytes binary: a NUL is
    /// looked for in git's first 8000 bytes, not further.
    #[test]
    fn binary_means_a_nul_in_the_first_8000_bytes() {
        let mut late = vec![b'a'; 9000];
        late[8100] = 0;
        assert!(!is_binary(&late));
        assert!(SourceBuffer::from_bytes(late).is_ok());
        let mut edge = vec![b'a'; 9000];
        edge[7999] = 0;
        assert_eq!(binary_offset(&edge), Some(7999));
        assert_eq!(
            SourceBuffer::from_bytes(edge).unwrap_err(),
            SourceError::Binary(7999)
        );
    }

    #[test]
    fn exact_positions_reject_instead_of_clamping() {
        // "é" is 2 bytes / 1 utf16 / 1 utf32; "😀" is 4 bytes / 2 utf16 / 1 utf32.
        let b = SourceBuffer::new("aé😀z\r\nq");
        let at = |line, col, unit| b.line_col_to_byte_exact(LineCol { line, col }, unit);
        assert_eq!(at(0, 3, ColumnUnit::Utf8), Some(3));
        assert_eq!(at(0, 2, ColumnUnit::Utf8), None); // inside é
        assert_eq!(at(0, 2, ColumnUnit::Utf16), Some(3));
        assert_eq!(at(0, 3, ColumnUnit::Utf16), None); // inside the surrogate pair
        assert_eq!(at(0, 4, ColumnUnit::Utf16), Some(7));
        assert_eq!(at(0, 5, ColumnUnit::Utf16), Some(8)); // end of line content
        assert_eq!(at(0, 3, ColumnUnit::Utf32), Some(7));
        assert_eq!(at(0, 4, ColumnUnit::Utf32), Some(8));
        assert_eq!(at(0, 5, ColumnUnit::Utf32), None);
        assert_eq!(at(0, 9, ColumnUnit::Utf8), None); // past the end
        assert_eq!(at(1, 1, ColumnUnit::Utf8), Some(11));
        assert_eq!(at(2, 0, ColumnUnit::Utf8), None);
    }

    #[test]
    fn column_units_round_trip() {
        let b = SourceBuffer::new("aé😀z\nq😀");
        for unit in [ColumnUnit::Utf8, ColumnUnit::Utf16, ColumnUnit::Utf32] {
            for offset in 0..=b.len() {
                if !b.text().is_char_boundary(offset) {
                    continue;
                }
                let lc = b.byte_to_line_col_in(offset, unit);
                assert_eq!(
                    b.line_col_to_byte_exact(lc, unit),
                    Some(offset),
                    "{unit:?} {offset}"
                );
            }
        }
    }
}

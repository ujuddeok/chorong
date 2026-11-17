//! Text selection in the code view: what a press, a drag, a double or triple click and
//! Shift-click select, how a selection lies on each shown line, and the text a copy puts on the
//! clipboard. Pure byte-offset maths on the file's buffer; the view maps the pointer to a byte
//! with the same hit test a click uses (`press_source`), and converts to display columns only
//! when it paints.

use crate::document::DisplayLine;
use cho_core::SourceBuffer;
use cho_syntax::Language;
use std::ops::Range;

/// What a press selects, and what a drag from it extends by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// A single press: character by character.
    Char,
    /// A double click: whole words.
    Word,
    /// A triple click: whole lines, with their line breaks.
    Line,
}

/// A selection in one document: the unit pressed first (`anchor`, already widened to `unit`)
/// and where the pointer is now (`head`). Tagged with the pane's `epoch` so a selection never
/// outlives the document it was made in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Range<usize>,
    pub head: usize,
    pub unit: Unit,
    pub epoch: u64,
}

impl Selection {
    /// A press at `byte`, selecting `unit` around it.
    pub fn new(buffer: &SourceBuffer, byte: usize, unit: Unit, epoch: u64) -> Self {
        let byte = buffer.clamp(byte);
        Selection {
            anchor: unit_at(buffer, byte, unit),
            head: byte,
            unit,
            epoch,
        }
    }

    /// The selected bytes: the anchor's unit joined with the head's.
    pub fn range(&self, buffer: &SourceBuffer) -> Range<usize> {
        let head = unit_at(buffer, self.head, self.unit);
        self.anchor.start.min(head.start)..self.anchor.end.max(head.end)
    }

    /// Moves the head (a drag, or a Shift-click) to `byte`.
    pub fn extend(&mut self, buffer: &SourceBuffer, byte: usize) {
        self.head = buffer.clamp(byte);
    }
}

/// The `unit` around `byte`: empty for a character, the word, or the whole line.
pub fn unit_at(buffer: &SourceBuffer, byte: usize, unit: Unit) -> Range<usize> {
    match unit {
        Unit::Char => byte..byte,
        Unit::Word => word_at(buffer, byte),
        Unit::Line => line_at(buffer, byte),
    }
}

/// The line holding `byte`, from its first byte to the start of the next line, so its line
/// break (`\n` or `\r\n`) is included; the last line ends at the end of the file.
pub fn line_at(buffer: &SourceBuffer, byte: usize) -> Range<usize> {
    let line = buffer.line_of(byte);
    let start = buffer.line_range(line).map_or(0, |r| r.start);
    let end = buffer
        .line_range(line + 1)
        .map_or(buffer.len(), |r| r.start);
    start..end
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c == ' ' || c == '\t' {
        Class::Space
    } else {
        Class::Other
    }
}

/// The word at `byte`, within its line: a run of letters, digits and `_`, or of spaces and
/// tabs; any other character is a word of its own. At the end of a word (or of the line) the
/// word before is taken. Empty on an empty line.
pub fn word_at(buffer: &SourceBuffer, byte: usize) -> Range<usize> {
    let byte = buffer.clamp(byte);
    let Some(line) = buffer.line_range(buffer.line_of(byte)) else {
        return byte..byte;
    };
    let text = &buffer.text()[line.clone()];
    let at = byte - line.start;
    let after = text[at..].chars().next();
    let before = text[..at].chars().next_back();
    let kind = match (before.map(class), after.map(class)) {
        (_, Some(Class::Word)) => Class::Word,
        (Some(Class::Word), _) => Class::Word,
        (_, Some(k)) => k,
        (Some(k), None) => k,
        (None, None) => return byte..byte,
    };
    if kind == Class::Other {
        // A single character: the one after the press, else the one before.
        return match after {
            Some(c) if class(c) == Class::Other => byte..byte + c.len_utf8(),
            _ => byte - before.map_or(0, char::len_utf8)..byte,
        };
    }
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| class(*c) == kind)
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .take_while(|(_, c)| class(*c) == kind)
        .last()
        .map_or(at, |(i, c)| at + i + c.len_utf8());
    line.start + start..line.start + end
}

/// How a selection lies on one shown line, in display columns (characters of the line's
/// display text, tabs expanded): `start..end`, plus one more cell when the selection runs on
/// past the line's end (its line break is selected). None when the line has nothing selected.
pub fn columns_on(line: &DisplayLine, sel: &Range<usize>) -> Option<Range<usize>> {
    let bytes = &line.bytes;
    if sel.start >= sel.end || sel.start > bytes.end || sel.end < bytes.start {
        return None;
    }
    let newline = sel.end > bytes.end;
    let column = |b: usize| {
        let display = match &line.tab_map {
            Some(map) => map[b - bytes.start],
            None => b - bytes.start,
        };
        crate::markers::column(&line.text, display)
    };
    let start = column(sel.start.max(bytes.start));
    let end = column(sel.end.min(bytes.end)) + usize::from(newline);
    (start < end).then_some(start..end)
}

/// Whether a fold's hidden lines (`hidden`, the bytes from the first hidden line's start to the
/// last one's end) are inside the selection: its placeholder row is painted as selected.
pub fn covers(sel: &Range<usize>, hidden: &Range<usize>) -> bool {
    sel.start < sel.end && sel.start <= hidden.start && sel.end >= hidden.end
}

/// Where a drag over a fold's `⋯ N lines` row puts the head, so the hidden lines are selected
/// whole: past them (the closing line's start) when the selection began above the fold,
/// before them (the signature line's end) when it began below.
pub fn head_over_fold(
    buffer: &SourceBuffer,
    anchor: &Range<usize>,
    signature: usize,
    closing: usize,
) -> usize {
    let after_signature = buffer.line_range(signature).map_or(0, |r| r.end);
    let closing_start = buffer.line_range(closing).map_or(buffer.len(), |r| r.start);
    if anchor.start <= after_signature {
        closing_start
    } else {
        after_signature
    }
}

/// What Copy puts on the clipboard: the selection, or the caret's whole line (with its line
/// break) when nothing is selected. Exactly the file's bytes, folded lines included.
pub fn copy_range(buffer: &SourceBuffer, sel: Option<&Range<usize>>, caret: usize) -> Range<usize> {
    match sel.filter(|r| r.start < r.end) {
        Some(r) => r.clone(),
        None => line_at(buffer, caret),
    }
}

/// The 1-based first and last line of `range` for a location header. A range ending at a
/// line's start (a whole-line selection) does not count that line.
pub fn line_span(buffer: &SourceBuffer, range: &Range<usize>) -> (usize, usize) {
    let first = buffer.line_of(range.start);
    let last = buffer.line_of(range.end.saturating_sub(1).max(range.start));
    (first + 1, last.max(first) + 1)
}

/// The fence's info string for a file: its language's usual Markdown name, else its
/// extension when that is plain letters and digits, else nothing.
pub fn fence_tag(language: Option<Language>, path: &str) -> String {
    let named = language.map(|l| match l {
        Language::Rust => "rust",
        Language::Swift => "swift",
        Language::TypeScript => "typescript",
        Language::Tsx => "tsx",
        Language::JavaScript => "javascript",
        Language::Python => "python",
        Language::Java => "java",
        Language::Json => "json",
        Language::Toml => "toml",
        Language::Xml => "xml",
        Language::Markdown => "markdown",
    });
    if let Some(n) = named {
        return n.to_string();
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && !ext.is_empty()
                && ext.len() <= 16
                && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            ext.to_ascii_lowercase()
        }
        _ => String::new(),
    }
}

/// Copy with Location: `path:first-last` (or `path:line`), then the text in a fenced block
/// tagged `tag`. The fence is longer than any run of backticks in the text, so the block
/// holds whatever the file does.
pub fn with_location(path: &str, (first, last): (usize, usize), tag: &str, text: &str) -> String {
    let header = if first == last {
        format!("{path}:{first}")
    } else {
        format!("{path}:{first}-{last}")
    };
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let newline = if text.ends_with('\n') || text.is_empty() {
        ""
    } else {
        "\n"
    };
    format!("{header}\n{fence}{tag}\n{text}{newline}{fence}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::expand_tabs;
    use crate::fold::{Folds, Row, folds_from_spans};

    fn buf(s: &str) -> SourceBuffer {
        SourceBuffer::new(s)
    }

    fn text(b: &SourceBuffer, r: Range<usize>) -> &str {
        &b.text()[r]
    }

    /// A shown line as the document builds it, tabs expanded.
    fn line(b: &SourceBuffer, ix: usize) -> DisplayLine {
        let bytes = b.line_range(ix).unwrap();
        let raw = &b.text()[bytes.clone()];
        let (shown, map) = expand_tabs(raw);
        DisplayLine {
            text: shown.into(),
            highlights: Vec::new(),
            bytes,
            tab_map: raw.contains('\t').then_some(map),
            calls: Vec::new(),
            call_ids: Vec::new(),
        }
    }

    #[test]
    fn a_drag_selects_from_press_to_pointer_either_way() {
        let b = buf("fn main() {\n    let x = 1;\n}\n");
        let mut s = Selection::new(&b, 3, Unit::Char, 0);
        assert!(s.range(&b).is_empty());
        s.extend(&b, 7);
        assert_eq!(text(&b, s.range(&b)), "main");
        // Back past the press: the range turns round the anchor.
        s.extend(&b, 0);
        assert_eq!(text(&b, s.range(&b)), "fn ");
    }

    #[test]
    fn a_drag_across_lines_keeps_the_line_breaks() {
        let b = buf("alpha\nbeta\r\ngamma\n");
        let mut s = Selection::new(&b, 2, Unit::Char, 0);
        s.extend(&b, 15);
        assert_eq!(text(&b, s.range(&b)), "pha\nbeta\r\ngam");
    }

    #[test]
    fn a_head_inside_a_character_is_moved_to_its_start() {
        let b = buf("é = ü;\n");
        let mut s = Selection::new(&b, 1, Unit::Char, 0);
        assert_eq!(s.anchor, 0..0);
        s.extend(&b, 6);
        assert_eq!(text(&b, s.range(&b)), "é = ");
    }

    #[test]
    fn double_click_selects_the_word_and_drags_by_words() {
        let b = buf("let total_count = a.len();\n");
        let mut s = Selection::new(&b, 6, Unit::Word, 0);
        assert_eq!(text(&b, s.range(&b)), "total_count");
        s.extend(&b, 20);
        assert_eq!(text(&b, s.range(&b)), "total_count = a.len");
        s.extend(&b, 1);
        assert_eq!(text(&b, s.range(&b)), "let total_count");
    }

    #[test]
    fn words_spaces_and_punctuation() {
        let b = buf("  foo(bar)  \n\nx");
        assert_eq!(text(&b, word_at(&b, 0)), "  ");
        // At a word's end the word before is taken.
        assert_eq!(text(&b, word_at(&b, 5)), "foo");
        assert_eq!(text(&b, word_at(&b, 9)), "bar");
        assert_eq!(text(&b, word_at(&b, 10)), "  ");
        assert_eq!(text(&b, word_at(&b, 12)), "  ");
        // An empty line has no word; the last line has one.
        assert_eq!(word_at(&b, 13), 13..13);
        assert_eq!(text(&b, word_at(&b, 14)), "x");
        // Punctuation is a word of one character, even in a run.
        let p = buf("a::b");
        assert_eq!(text(&p, word_at(&p, 1)), "a");
        assert_eq!(text(&p, word_at(&p, 2)), ":");
        // Non-ASCII letters belong to words.
        let u = buf("naïve café");
        assert_eq!(text(&u, word_at(&u, 3)), "naïve");
    }

    #[test]
    fn triple_click_selects_whole_lines_with_their_breaks() {
        let b = buf("one\r\ntwo\nthree");
        let mut s = Selection::new(&b, 1, Unit::Line, 0);
        assert_eq!(text(&b, s.range(&b)), "one\r\n");
        s.extend(&b, 6);
        assert_eq!(text(&b, s.range(&b)), "one\r\ntwo\n");
        // The last line has no break to take.
        assert_eq!(text(&b, line_at(&b, 11)), "three");
    }

    #[test]
    fn columns_follow_tabs() {
        let b = buf("\tlet x;\nab\tc\n");
        let l0 = line(&b, 0);
        // "let" is display columns 4..7 after the tab's four spaces.
        assert_eq!(columns_on(&l0, &(1..4)), Some(4..7));
        // The tab itself is four columns.
        assert_eq!(columns_on(&l0, &(0..1)), Some(0..4));
        let l1 = line(&b, 1);
        // "b\tc": the tab after "ab" reaches column 4.
        assert_eq!(columns_on(&l1, &(9..12)), Some(1..5));
    }

    #[test]
    fn columns_count_characters_and_a_selected_line_break() {
        let b = buf("é1\n\nz\n");
        let l0 = line(&b, 0);
        // From inside line 0 on into the next line: its break is one more cell.
        assert_eq!(columns_on(&l0, &(2..5)), Some(1..3));
        // An empty line inside the selection shows one cell.
        let l1 = line(&b, 1);
        assert_eq!(columns_on(&l1, &(0..6)), Some(0..1));
        // A selection ending exactly at a line's start leaves that line unpainted.
        let l2 = line(&b, 2);
        assert_eq!(columns_on(&l2, &(0..5)), None);
        assert_eq!(columns_on(&l0, &(1..1)), None);
    }

    #[test]
    fn folded_lines_are_copied_and_their_row_is_painted() {
        let src = "fn a() {\n    one;\n    two;\n    three;\n}\nfn b() {}\n";
        let b = buf(src);
        let folds = Folds::new(
            folds_from_spans([(0, 4)]),
            b.line_count(),
            [0].into_iter().collect(),
        );
        // Line 0, its placeholder, then line 4.
        assert_eq!(folds.rows()[1], Row::Placeholder(0));
        // A drag from line 0 down over the placeholder takes every hidden line.
        let mut s = Selection::new(&b, 3, Unit::Char, 0);
        s.extend(&b, head_over_fold(&b, &s.anchor, 0, 4));
        assert_eq!(
            text(&b, s.range(&b)),
            "a() {\n    one;\n    two;\n    three;\n"
        );
        // From below, up over it: from the signature line's end.
        let below = b.line_range(5).unwrap().start + 2;
        let mut up = Selection::new(&b, below, Unit::Char, 0);
        up.extend(&b, head_over_fold(&b, &up.anchor, 0, 4));
        assert_eq!(
            text(&b, up.range(&b)),
            "\n    one;\n    two;\n    three;\n}\nfn"
        );
        // Copy is the file's own bytes across the fold.
        let r = copy_range(&b, Some(&s.range(&b)), 0);
        assert_eq!(&src[r], "a() {\n    one;\n    two;\n    three;\n");
        let hidden = b.line_range(1).unwrap().start..b.line_range(3).unwrap().end;
        assert!(covers(&s.range(&b), &hidden));
        assert!(!covers(&(0..10), &hidden));
    }

    #[test]
    fn copy_without_a_selection_takes_the_caret_line() {
        let b = buf("one\ntwo\nthree");
        assert_eq!(text(&b, copy_range(&b, None, 5)), "two\n");
        assert_eq!(text(&b, copy_range(&b, Some(&(5..5)), 5)), "two\n");
        assert_eq!(text(&b, copy_range(&b, Some(&(0..2)), 5)), "on");
        assert_eq!(text(&b, copy_range(&b, None, 10)), "three");
    }

    #[test]
    fn the_location_header_names_the_lines() {
        let b = buf("a\nb\nc\nd\n");
        assert_eq!(line_span(&b, &(2..5)), (2, 3));
        // Whole lines 2 and 3, ending at line 4's start.
        assert_eq!(line_span(&b, &(2..6)), (2, 3));
        assert_eq!(line_span(&b, &(2..3)), (2, 2));
        assert_eq!(line_span(&b, &(2..2)), (2, 2));
        assert_eq!(
            with_location("src/server.rs", (25, 29), "rust", "let x = 1;\n"),
            "src/server.rs:25-29\n```rust\nlet x = 1;\n```\n"
        );
        assert_eq!(
            with_location("a.py", (3, 3), "python", "x"),
            "a.py:3\n```python\nx\n```\n"
        );
        // A fence inside the text gets a longer fence round it.
        assert_eq!(
            with_location("r.md", (1, 2), "markdown", "````\ncode\n"),
            "r.md:1-2\n`````markdown\n````\ncode\n`````\n"
        );
    }

    #[test]
    fn the_fence_tag_comes_from_the_language_or_a_plain_extension() {
        assert_eq!(fence_tag(Some(Language::Rust), "x.rs"), "rust");
        assert_eq!(fence_tag(Some(Language::Tsx), "x.tsx"), "tsx");
        assert_eq!(fence_tag(None, "build/Makefile"), "");
        assert_eq!(fence_tag(None, "conf/app.YAML"), "yaml");
        assert_eq!(fence_tag(None, "dir.d/.env"), "");
        assert_eq!(fence_tag(None, "weird.a`b"), "");
    }
}

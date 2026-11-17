//! The inline parser behind [`crate::investigation::note_blocks`]: `` `code` ``, `[text](target)` links that show their
//! target, and `**bold**`, `__bold__`, `*italic*`, `_italic_` emphasis. Every other character,
//! raw HTML and image syntax included, stays as text; nothing is loaded.
//!
//! Emphasis follows CommonMark's delimiter-run rules (left- and right-flanking runs, no
//! intraword `_`, the "multiple of 3" rule), so `snake_case_names`, `a * b` and unmatched
//! `*` stay literal. Code spans, links and raw HTML tags are atomic: no emphasis inside them.

use crate::investigation::{Inline, LinkKind, link_at, note_blocks};
use std::ops::Range;

/// A note block's inline pieces, emphasis applied.
pub(crate) fn inlines(s: &str) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for (piece, bold, italic) in pieces(s) {
        match piece {
            Piece::Text(t) => {
                // Delimiters left as text split runs of one style: join them again.
                match out.last_mut() {
                    Some(Inline::Text(prev)) if !bold && !italic => prev.push_str(&t),
                    Some(Inline::Styled {
                        text,
                        bold: b,
                        italic: i,
                    }) if *b == bold && *i == italic => text.push_str(&t),
                    _ if !bold && !italic => out.push(Inline::Text(t)),
                    _ => out.push(Inline::Styled {
                        text: t,
                        bold,
                        italic,
                    }),
                }
            }
            Piece::Code(text) => out.push(Inline::Code(text)),
            Piece::Link { text, target, kind } => out.push(Inline::Link { text, target, kind }),
        }
    }
    out
}

/// How a stretch of [`Text`] is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Look {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// A link's shown text (never opened from these surfaces).
    pub link: Option<LinkKind>,
}

/// A note as one run of text and the looks of its stretches, for surfaces drawn as a single
/// text element (highlight strips, chunk lists, comparison notes, tooltips). It is the same
/// parse as [`note_blocks`]: emphasis loses its markers, code spans lose their backticks and
/// keep a code look, links show as [`link_shown`]; raw HTML and images stay text. Blocks are
/// joined with `sep` (a space on one-line strips, a newline where text may wrap).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Text {
    pub text: String,
    /// Byte ranges of `text` with a look other than plain, in order.
    pub runs: Vec<(Range<usize>, Look)>,
}

impl Text {
    pub(crate) fn of(md: &str, sep: &str) -> Text {
        let mut out = Text::default();
        for (k, block) in note_blocks(md).iter().enumerate() {
            if k > 0 {
                out.push(sep, Look::default());
            }
            for piece in block {
                match piece {
                    Inline::Text(t) => out.push(t, Look::default()),
                    Inline::Styled { text, bold, italic } => out.push(
                        text,
                        Look {
                            bold: *bold,
                            italic: *italic,
                            ..Look::default()
                        },
                    ),
                    Inline::Code(t) => out.push(
                        t,
                        Look {
                            code: true,
                            ..Look::default()
                        },
                    ),
                    Inline::Link { text, target, kind } => out.push(
                        &link_shown(text, target),
                        Look {
                            link: Some(*kind),
                            ..Look::default()
                        },
                    ),
                }
            }
        }
        out
    }

    /// Several notes as one, each parsed on its own, joined with `sep` (plain).
    pub(crate) fn join<'a>(notes: impl IntoIterator<Item = &'a str>, sep: &str) -> Text {
        let mut out = Text::default();
        for (k, md) in notes.into_iter().enumerate() {
            if k > 0 {
                out.push(sep, Look::default());
            }
            out.append(Text::of(md, " "));
        }
        out
    }

    /// Chorong's own text, not parsed.
    pub(crate) fn plain(text: &str) -> Text {
        Text {
            text: text.to_string(),
            runs: Vec::new(),
        }
    }

    /// Adds `text` as written (not parsed), drawn with `look`.
    pub(crate) fn push(&mut self, text: &str, look: Look) {
        let start = self.text.len();
        self.text.push_str(text);
        if look != Look::default() && !text.is_empty() {
            match self.runs.last_mut() {
                Some((r, l)) if r.end == start && *l == look => r.end = self.text.len(),
                _ => self.runs.push((start..self.text.len(), look)),
            }
        }
    }

    pub(crate) fn append(&mut self, other: Text) {
        let at = self.text.len();
        self.text.push_str(&other.text);
        self.runs.extend(
            other
                .runs
                .into_iter()
                .map(|(r, l)| (r.start + at..r.end + at, l)),
        );
    }
}

/// How a link reads: its text and its real target (just the target when they are the same).
pub(crate) fn link_shown(text: &str, target: &str) -> String {
    if text == target {
        target.to_string()
    } else {
        format!("{text} ({target})")
    }
}

/// A parsed piece, before styles are applied.
enum Piece {
    Text(String),
    Code(String),
    Link {
        text: String,
        target: String,
        kind: LinkKind,
    },
}

/// A token: a piece, or a run of `*` or `_` that may open or close emphasis.
enum Tok {
    Piece(Piece),
    Delim(Delim),
}

struct Delim {
    ch: char,
    /// Characters of the run not used by emphasis: shown as text.
    count: usize,
    /// The run's length as written (the "multiple of 3" rule reads it).
    orig: usize,
    can_open: bool,
    can_close: bool,
    /// Still on CommonMark's delimiter stack.
    active: bool,
    /// Sizes (1 = italic, 2 = bold) this run opens, innermost first.
    opens: Vec<usize>,
    /// Sizes this run closes, innermost first.
    closes: Vec<usize>,
}

/// CommonMark's "Unicode punctuation" for flanking, approximated outside ASCII as anything that
/// is neither alphanumeric nor whitespace.
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace())
}

fn tokens(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut at = 0;
    let flush = |text: &mut String, out: &mut Vec<Tok>| {
        if !text.is_empty() {
            out.push(Tok::Piece(Piece::Text(std::mem::take(text))));
        }
    };
    while let Some(c) = s[at..].chars().next() {
        let rest = &s[at..];
        if c == '`'
            && let Some(end) = rest[1..].find('`')
        {
            flush(&mut text, &mut out);
            out.push(Tok::Piece(Piece::Code(rest[1..1 + end].to_string())));
            at += end + 2;
            continue;
        }
        // An image (`![…](…)`) stays text: nothing is loaded.
        let image = text.ends_with('!');
        if c == '['
            && !image
            && let Some((label, target, used)) = link_at(rest)
        {
            flush(&mut text, &mut out);
            out.push(Tok::Piece(Piece::Link {
                text: label.to_string(),
                target: target.to_string(),
                kind: LinkKind::of(target),
            }));
            at += used;
            continue;
        }
        // A raw HTML tag (or `<autolink>`) stays text as written, with no emphasis inside it.
        if c == '<'
            && let Some(len) = tag_at(rest)
        {
            text.push_str(&rest[..len]);
            at += len;
            continue;
        }
        // `\*` and `\_` are a literal `*` and `_`.
        if c == '\\'
            && let Some(e @ ('*' | '_')) = rest[1..].chars().next()
        {
            text.push(e);
            at += 2;
            continue;
        }
        if c == '*' || c == '_' {
            let len = rest.len() - rest.trim_start_matches(c).len();
            let before = s[..at].chars().next_back();
            let after = rest[len..].chars().next();
            let ws = |c: Option<char>| c.is_none_or(char::is_whitespace);
            let punct = |c: Option<char>| c.is_some_and(is_punct);
            let left = !ws(after) && (!punct(after) || ws(before) || punct(before));
            let right = !ws(before) && (!punct(before) || ws(after) || punct(after));
            let (can_open, can_close) = if c == '*' {
                (left, right)
            } else {
                (
                    left && (!right || punct(before)),
                    right && (!left || punct(after)),
                )
            };
            flush(&mut text, &mut out);
            out.push(Tok::Delim(Delim {
                ch: c,
                count: len,
                orig: len,
                can_open,
                can_close,
                active: true,
                opens: Vec::new(),
                closes: Vec::new(),
            }));
            at += len;
            continue;
        }
        text.push(c);
        at += c.len_utf8();
    }
    flush(&mut text, &mut out);
    out
}

fn delim(toks: &[Tok], i: usize) -> &Delim {
    match &toks[i] {
        Tok::Delim(d) => d,
        Tok::Piece(_) => unreachable!("not a delimiter run"),
    }
}

fn delim_mut(toks: &mut [Tok], i: usize) -> &mut Delim {
    match &mut toks[i] {
        Tok::Delim(d) => d,
        Tok::Piece(_) => unreachable!("not a delimiter run"),
    }
}

/// Matches openers to closers: CommonMark's "process emphasis", without its `openers_bottom`
/// shortcut (notes are short).
fn match_emphasis(toks: &mut [Tok]) {
    let delims: Vec<usize> = (0..toks.len())
        .filter(|&i| matches!(toks[i], Tok::Delim(_)))
        .collect();
    for (ci, &c) in delims.iter().enumerate() {
        if !delim(toks, c).can_close || !delim(toks, c).active {
            continue;
        }
        while delim(toks, c).count > 0 {
            let closer = delim(toks, c);
            let opener = (0..ci).rev().find(|&oi| {
                let o = delim(toks, delims[oi]);
                let multiple_of_3 = (o.can_close || closer.can_open)
                    && (o.orig + closer.orig).is_multiple_of(3)
                    && !(o.orig.is_multiple_of(3) && closer.orig.is_multiple_of(3));
                o.active && o.can_open && o.ch == closer.ch && o.count > 0 && !multiple_of_3
            });
            let Some(oi) = opener else {
                if !closer.can_open {
                    delim_mut(toks, c).active = false;
                }
                break;
            };
            let n = if delim(toks, delims[oi]).count >= 2 && closer.count >= 2 {
                2
            } else {
                1
            };
            let o = delim_mut(toks, delims[oi]);
            o.count -= n;
            o.opens.push(n);
            o.active = o.count > 0;
            let cl = delim_mut(toks, c);
            cl.count -= n;
            cl.closes.push(n);
            for &between in &delims[oi + 1..ci] {
                delim_mut(toks, between).active = false;
            }
        }
    }
}

/// The note's pieces with the style each is under (bold, italic).
fn pieces(s: &str) -> Vec<(Piece, bool, bool)> {
    let mut toks = tokens(s);
    match_emphasis(&mut toks);
    // Open spans of each size: 2 is bold, 1 italic.
    let (mut strong, mut em) = (0usize, 0usize);
    let mut out = Vec::new();
    for t in toks {
        match t {
            Tok::Piece(p) => out.push((p, strong > 0, em > 0)),
            Tok::Delim(d) => {
                for &n in &d.closes {
                    let depth = if n == 2 { &mut strong } else { &mut em };
                    *depth = depth.saturating_sub(1);
                }
                if d.count > 0 {
                    let text = d.ch.to_string().repeat(d.count);
                    out.push((Piece::Text(text), strong > 0, em > 0));
                }
                for &n in d.opens.iter().rev() {
                    *(if n == 2 { &mut strong } else { &mut em }) += 1;
                }
            }
        }
    }
    out
}

/// The length of a raw HTML tag, comment or `<autolink>` at the start of `s`: `<` then a
/// letter, `/` or `!`, up to the next `>` with no `<` before it.
fn tag_at(s: &str) -> Option<usize> {
    let next = s[1..].chars().next()?;
    if !(next.is_ascii_alphabetic() || next == '/' || next == '!') {
        return None;
    }
    let end = s[1..].find(['<', '>'])? + 1;
    (s.as_bytes()[end] == b'>').then_some(end + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Inline {
        Inline::Text(s.into())
    }
    fn b(s: &str) -> Inline {
        Inline::Styled {
            text: s.into(),
            bold: true,
            italic: false,
        }
    }
    fn i(s: &str) -> Inline {
        Inline::Styled {
            text: s.into(),
            bold: false,
            italic: true,
        }
    }
    fn bi(s: &str) -> Inline {
        Inline::Styled {
            text: s.into(),
            bold: true,
            italic: true,
        }
    }
    fn one(s: &str) -> Vec<Inline> {
        let mut blocks = note_blocks(s);
        assert_eq!(blocks.len(), 1, "{s}");
        blocks.remove(0)
    }

    #[test]
    fn bold_and_italic_with_either_delimiter() {
        assert_eq!(
            one("Chorong **never** runs these"),
            [t("Chorong "), b("never"), t(" runs these")]
        );
        assert_eq!(one("a __b__ c"), [t("a "), b("b"), t(" c")]);
        assert_eq!(one("a *b* c"), [t("a "), i("b"), t(" c")]);
        assert_eq!(one("a _b c_ d"), [t("a "), i("b c"), t(" d")]);
        assert_eq!(one("**all**"), [b("all")]);
    }

    #[test]
    fn emphasis_nests() {
        assert_eq!(one("***both***"), [bi("both")]);
        assert_eq!(
            one("**bold *and italic* bold**"),
            [b("bold "), bi("and italic"), b(" bold")]
        );
        assert_eq!(one("*it **both** it*"), [i("it "), bi("both"), i(" it")]);
        assert_eq!(one("_a **b** c_"), [i("a "), bi("b"), i(" c")]);
        // Bold within a word with `*` (CommonMark allows it; `_` does not).
        assert_eq!(
            one("un**frigging**believable"),
            [t("un"), b("frigging"), t("believable")]
        );
    }

    #[test]
    fn unmatched_and_spaced_delimiters_stay_literal() {
        for s in [
            "a * b * c",
            "2 * 3 = 6",
            "**not closed",
            "not opened**",
            "* not a list here",
            "a ** b",
            "*",
            "__",
            "a*",
        ] {
            assert_eq!(one(s), [t(s)], "{s}");
        }
        // The extra delimiter stays as text.
        assert_eq!(one("**a*"), [t("*"), i("a")]);
        assert_eq!(one("*a**"), [i("a"), t("*")]);
        // The "multiple of 3" rule: `*foo**bar*` is one italic span.
        assert_eq!(one("*foo**bar*"), [i("foo**bar")]);
        // Escapes.
        assert_eq!(one(r"\*not\* \_this\_"), [t("*not* _this_")]);
    }

    #[test]
    fn intraword_underscores_stay_literal() {
        for s in [
            "snake_case_names",
            "call do_this_now() first",
            "foo_bar_",
            "a_b_c and x_y",
        ] {
            let got = one(s);
            assert!(
                got.iter().all(|p| !matches!(p, Inline::Styled { .. })),
                "{s}: {got:?}"
            );
        }
        // `__init__` at the start of a word is bold in CommonMark (and on GitHub); inside a word
        // it is not. Authors write it as code: `__init__`.
        assert_eq!(one("__init__.py"), [b("init"), t(".py")]);
        assert_eq!(one("x__init__y"), [t("x__init__y")]);
        assert_eq!(one("snake_case_names"), [t("snake_case_names")]);
    }

    #[test]
    fn code_spans_are_atomic() {
        assert_eq!(one("`a*b*c`"), [Inline::Code("a*b*c".into())]);
        assert_eq!(one("`**x**`"), [Inline::Code("**x**".into())]);
        // A delimiter in code cannot pair with one outside.
        assert_eq!(one("*a `b* c`"), [t("*a "), Inline::Code("b* c".into())]);
        // Emphasis around a code span still applies to the text beside it.
        assert_eq!(
            one("**see `push` here**"),
            [b("see "), Inline::Code("push".into()), b(" here")]
        );
    }

    #[test]
    fn links_and_raw_html_beside_emphasis() {
        let got = one("**read** [the *docs*](https://example.com/a_b_c) *now*");
        assert_eq!(got[0], b("read"));
        assert_eq!(
            got[2],
            Inline::Link {
                text: "the *docs*".into(),
                target: "https://example.com/a_b_c".into(),
                kind: LinkKind::Web,
            }
        );
        assert_eq!(got[3..], [t(" "), i("now")]);
        // Bold around a link: the link stays a link (with its own look).
        let got = one("**[x](chorong://y)**");
        assert!(matches!(
            got[..],
            [Inline::Link {
                kind: LinkKind::Chorong,
                ..
            }]
        ));
        // Raw HTML stays text, delimiters inside a tag included.
        assert_eq!(one("<b>Raw</b> **bold**"), [t("<b>Raw</b> "), b("bold")]);
        assert_eq!(one("<a title=\"*x*\">t</a>"), [t("<a title=\"*x*\">t</a>")]);
        // `a < b` is not a tag.
        assert_eq!(one("a < *b*"), [t("a < "), i("b")]);
        // The fixture note from the action-step guide shot.
        let got = one(
            "then watch `Queue::push` while you read. Chorong **never** runs these: copy each one. <b>Raw HTML</b> shows as text.",
        );
        assert!(got.contains(&b("never")));
        assert!(
            got.iter()
                .any(|p| matches!(p, Inline::Text(t) if t.contains("<b>Raw HTML</b>")))
        );
    }

    #[test]
    fn list_markers_stay_text() {
        let blocks = note_blocks("* first *one*\n* second");
        assert_eq!(blocks[0], [t("* first "), i("one")]);
        assert_eq!(blocks[1], [t("* second")]);
    }

    #[test]
    fn text_styles_emphasis_code_and_links_without_their_markers() {
        let bold = Look {
            bold: true,
            ..Look::default()
        };
        let t = Text::of(
            "Chorong **never** runs `a*b*c`, see [x](https://e.org) _now_",
            " ",
        );
        assert_eq!(
            t.text,
            "Chorong never runs a*b*c, see x (https://e.org) now"
        );
        let at = |s: &str| t.text.find(s).unwrap();
        assert_eq!(
            t.runs,
            [
                (8..13, bold),
                (
                    at("a*b*c")..at("a*b*c") + 5,
                    Look {
                        code: true,
                        ..Look::default()
                    }
                ),
                (
                    at("x (")..at(" now"),
                    Look {
                        link: Some(LinkKind::Web),
                        ..Look::default()
                    }
                ),
                (
                    t.text.len() - 3..t.text.len(),
                    Look {
                        italic: true,
                        ..Look::default()
                    }
                ),
            ]
        );
        // Nothing to style: the text as written.
        let t = Text::of(
            "snake_case and 2 * 3 <b>x</b> ![i](https://e.org/i.png)",
            " ",
        );
        assert_eq!(
            t.text,
            "snake_case and 2 * 3 <b>x</b> ![i](https://e.org/i.png)"
        );
        assert!(t.runs.is_empty());
        // Other schemes are shown but marked inert.
        let t = Text::of("[run](javascript:alert(1))", " ");
        assert_eq!(t.runs[0].1.link, Some(LinkKind::Inert));
    }

    #[test]
    fn text_joins_blocks_and_notes() {
        let t = Text::of("**a**\n\n*b*", "\n");
        assert_eq!(t.text, "a\nb");
        assert_eq!(t.runs.len(), 2);
        let t = Text::join(["**one**", "two", "*three*"], " \u{b7} ");
        assert_eq!(t.text, "one \u{b7} two \u{b7} three");
        assert_eq!(t.runs[0].0, 0..3);
        assert_eq!(&t.text[t.runs[1].0.clone()], "three");
        let mut p = Text::plain("**cho's own**");
        p.push(" ", Look::default());
        p.append(Text::of("*x*", " "));
        assert_eq!(p.text, "**cho's own** x");
        assert_eq!(p.runs.len(), 1);
    }
}

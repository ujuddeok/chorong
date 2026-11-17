//! JSON with comments (Zed-style JSONC): a parser that remembers byte spans, and text edits
//! that change one value without disturbing anything else.
//!
//! Accepted beyond JSON: `//` and `/* */` comments and trailing commas. Positions are UTF-8
//! byte offsets into the text. The edit functions return new text and never reformat the
//! rest of the file: comments, key order, blank lines and unknown keys stay where they were.
//! What is not preserved: a comment sitting directly above a key that is *removed* stays
//! behind (it is a separate token); and a key inserted into a file goes at the end of its
//! object.
//!
//! This is Chorong's one JSONC reader: settings files (`cho-settings`) and repository files
//! such as `tsconfig.json` ([`parse_value`]) both use it. Malformed text (an unterminated
//! comment or string, a comment between two numbers) is an error, never read leniently.

use std::fmt;
use std::ops::Range;

/// The deepest nesting [`parse`] accepts; deeper text is an error, not a stack overflow.
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub span: Range<usize>,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Null,
    Bool(bool),
    /// The number as written.
    Number(String),
    Str(String),
    Array(Vec<Node>),
    Object(Vec<Member>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub key: String,
    pub key_span: Range<usize>,
    pub value: Node,
}

impl Node {
    pub fn members(&self) -> Option<&[Member]> {
        match &self.kind {
            NodeKind::Object(m) => Some(m),
            _ => None,
        }
    }

    /// The last member with this key, as JSON readers do when a key repeats.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.members()?
            .iter()
            .rev()
            .find(|m| m.key == key)
            .map(|m| &m.value)
    }
}

/// A syntax error, with a position for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// Byte offset of the error.
    pub offset: usize,
    /// One-based line.
    pub line: usize,
    /// One-based column, in characters.
    pub column: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for ParseError {}

/// Why an edit couldn't be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    Parse(ParseError),
    /// The document has a different shape than the path needs (for example a number where an
    /// object is expected). The file is left alone.
    Shape(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::Parse(e) => e.fmt(f),
            EditError::Shape(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for EditError {}

impl From<ParseError> for EditError {
    fn from(e: ParseError) -> Self {
        EditError::Parse(e)
    }
}

/// Parse a document. Empty text, or text with only comments, is `Ok(None)`.
pub fn parse(text: &str) -> Result<Option<Node>, ParseError> {
    let mut p = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
    };
    p.skip_trivia()?;
    if p.pos >= p.bytes.len() {
        return Ok(None);
    }
    let node = p.value(0)?;
    p.skip_trivia()?;
    if p.pos < p.bytes.len() {
        return Err(p.error("unexpected text after the top-level value"));
    }
    Ok(Some(node))
}

/// The document as a `serde_json` value, for readers that want the data and not the spans.
/// `None` for a syntax error, empty text or a number `serde_json` cannot represent. A key
/// that repeats keeps its last value, as in [`Node::get`].
pub fn parse_value(text: &str) -> Option<serde_json::Value> {
    to_value(&parse(text).ok()??)
}

/// A parsed node as a `serde_json` value; `None` for a number `serde_json` cannot represent.
pub fn to_value(node: &Node) -> Option<serde_json::Value> {
    use serde_json::Value;
    Some(match &node.kind {
        NodeKind::Null => Value::Null,
        NodeKind::Bool(b) => Value::Bool(*b),
        NodeKind::Number(n) => Value::Number(n.parse().ok()?),
        NodeKind::Str(s) => Value::String(s.clone()),
        NodeKind::Array(items) => Value::Array(items.iter().map(to_value).collect::<Option<_>>()?),
        NodeKind::Object(members) => {
            let mut map = serde_json::Map::new();
            for m in members {
                map.insert(m.key.clone(), to_value(&m.value)?);
            }
            Value::Object(map)
        }
    })
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn error_at(&self, offset: usize, message: &str) -> ParseError {
        let before = &self.text[..offset.min(self.text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        ParseError {
            message: message.to_string(),
            offset,
            line,
            column,
        }
    }

    fn error(&self, message: &str) -> ParseError {
        self.error_at(self.pos, message)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_trivia(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.pos += 1,
                Some(b'/') => match self.bytes.get(self.pos + 1) {
                    Some(b'/') => {
                        while self.peek().is_some_and(|b| b != b'\n') {
                            self.pos += 1;
                        }
                    }
                    Some(b'*') => {
                        let start = self.pos;
                        self.pos += 2;
                        loop {
                            match self.peek() {
                                None => {
                                    return Err(self.error_at(start, "unterminated comment"));
                                }
                                Some(b'*') if self.bytes.get(self.pos + 1) == Some(&b'/') => {
                                    self.pos += 2;
                                    break;
                                }
                                Some(_) => self.pos += 1,
                            }
                        }
                    }
                    _ => return Err(self.error("unexpected '/'")),
                },
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        let start = self.pos;
        let kind = match self.peek() {
            None => return Err(self.error("expected a value")),
            Some(b'{') => self.object(depth)?,
            Some(b'[') => self.array(depth)?,
            Some(b'"') => NodeKind::Str(self.string()?),
            Some(b'-' | b'0'..=b'9') => {
                while self
                    .peek()
                    .is_some_and(|b| matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
                {
                    self.pos += 1;
                }
                NodeKind::Number(self.text[start..self.pos].to_string())
            }
            Some(b'a'..=b'z') => {
                while self.peek().is_some_and(|b| b.is_ascii_lowercase()) {
                    self.pos += 1;
                }
                match &self.text[start..self.pos] {
                    "true" => NodeKind::Bool(true),
                    "false" => NodeKind::Bool(false),
                    "null" => NodeKind::Null,
                    _ => return Err(self.error_at(start, "expected a value")),
                }
            }
            Some(_) => return Err(self.error("expected a value")),
        };
        Ok(Node {
            span: start..self.pos,
            kind,
        })
    }

    fn object(&mut self, depth: usize) -> Result<NodeKind, ParseError> {
        let open = self.pos;
        self.pos += 1;
        let mut members = Vec::new();
        loop {
            self.skip_trivia()?;
            match self.peek() {
                None => return Err(self.error_at(open, "unterminated object")),
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(NodeKind::Object(members));
                }
                Some(b'"') => {}
                Some(_) => return Err(self.error("expected a quoted key or '}'")),
            }
            let key_start = self.pos;
            let key = self.string()?;
            let key_span = key_start..self.pos;
            self.skip_trivia()?;
            if self.peek() != Some(b':') {
                return Err(self.error("expected ':' after the key"));
            }
            self.pos += 1;
            self.skip_trivia()?;
            let value = self.value(depth + 1)?;
            members.push(Member {
                key,
                key_span,
                value,
            });
            self.skip_trivia()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {}
                None => return Err(self.error_at(open, "unterminated object")),
                Some(_) => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<NodeKind, ParseError> {
        let open = self.pos;
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip_trivia()?;
            match self.peek() {
                None => return Err(self.error_at(open, "unterminated array")),
                Some(b']') => {
                    self.pos += 1;
                    return Ok(NodeKind::Array(items));
                }
                Some(_) => {}
            }
            items.push(self.value(depth + 1)?);
            self.skip_trivia()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {}
                None => return Err(self.error_at(open, "unterminated array")),
                Some(_) => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        let open = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let run = self.pos;
            while self
                .peek()
                .is_some_and(|b| b != b'"' && b != b'\\' && b >= 0x20)
            {
                self.pos += 1;
            }
            out.push_str(&self.text[run..self.pos]);
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let c = self
                        .peek()
                        .ok_or_else(|| self.error_at(open, "unterminated string"))?;
                    self.pos += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let ch = if (0xD800..0xDC00).contains(&hi)
                                && self.bytes.get(self.pos) == Some(&b'\\')
                                && self.bytes.get(self.pos + 1) == Some(&b'u')
                            {
                                self.pos += 2;
                                let lo = self.hex4()?;
                                char::from_u32(
                                    0x10000
                                        + ((hi - 0xD800) << 10)
                                        + (lo.wrapping_sub(0xDC00) & 0x3FF),
                                )
                            } else {
                                char::from_u32(hi)
                            };
                            out.push(ch.unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err(self.error("invalid escape in string")),
                    }
                }
                _ => return Err(self.error_at(open, "unterminated string")),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let digits = self
            .text
            .get(self.pos..self.pos + 4)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .ok_or_else(|| self.error("expected four hex digits"))?;
        self.pos += 4;
        Ok(digits)
    }
}

/// Quote a string as JSON.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------------------

/// Set the value at `path` (object keys from the root) to `literal`, which must already be
/// valid JSON text (`true`, `300`, `"x"`). Missing objects along the path are created. Only
/// the value's own bytes change, or one member is added at the end of its object.
pub fn set(text: &str, path: &[&str], literal: &str) -> Result<String, EditError> {
    assert!(!path.is_empty());
    let Some(root) = parse(text)? else {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&nested_object(path, literal, 0));
        out.push('\n');
        return Ok(out);
    };
    if root.members().is_none() {
        return Err(EditError::Shape(
            "the top-level value is not an object".into(),
        ));
    }
    let mut obj = &root;
    for (i, key) in path.iter().enumerate() {
        let found = obj
            .members()
            .and_then(|ms| ms.iter().rev().find(|m| m.key == *key));
        let last = i + 1 == path.len();
        match found {
            Some(m) if last => {
                let mut out = String::with_capacity(text.len() + literal.len());
                out.push_str(&text[..m.value.span.start]);
                out.push_str(literal);
                out.push_str(&text[m.value.span.end..]);
                return Ok(out);
            }
            Some(m) if m.value.members().is_some() => obj = &m.value,
            Some(_) => {
                return Err(EditError::Shape(format!(
                    "`{}` is not an object, so `{}` can't be stored inside it",
                    path[..=i].join("."),
                    path[i + 1]
                )));
            }
            None => return Ok(insert_member(text, obj, &path[i..], literal)),
        }
    }
    unreachable!("the loop returns on the last path segment")
}

/// Remove the value at `path`, and any objects it leaves empty (except the root). Returns the
/// text unchanged when the path isn't there.
pub fn remove(text: &str, path: &[&str]) -> Result<String, EditError> {
    assert!(!path.is_empty());
    let Some(root) = parse(text)? else {
        return Ok(text.to_string());
    };
    if root.members().is_none() {
        return Err(EditError::Shape(
            "the top-level value is not an object".into(),
        ));
    }
    // chain[i] = (the object holding path[i], index of path[i] within it)
    let mut chain: Vec<(&Node, usize)> = Vec::new();
    let mut obj = &root;
    for key in path {
        let Some(ms) = obj.members() else {
            return Ok(text.to_string());
        };
        let Some(idx) = ms.iter().rposition(|m| m.key == *key) else {
            return Ok(text.to_string());
        };
        chain.push((obj, idx));
        obj = &ms[idx].value;
    }
    // Climb while the parent would be left empty (never remove the root).
    let mut level = chain.len() - 1;
    while level > 0 && chain[level].0.members().is_some_and(|m| m.len() == 1) {
        level -= 1;
    }
    let (holder, idx) = chain[level];
    Ok(remove_member(text, holder, idx))
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn indent_of_line(text: &str, at: usize) -> &str {
    let s = line_start(text, at);
    let rest = &text[s..];
    let n = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    &rest[..n]
}

/// The indentation members of `obj` use, or one level deeper than the line the brace is on.
fn member_indent(text: &str, obj: &Node) -> String {
    if let Some(first) = obj.members().and_then(|m| m.first()) {
        let s = line_start(text, first.key_span.start);
        if text[s..first.key_span.start]
            .chars()
            .all(|c| c == ' ' || c == '\t')
        {
            return text[s..first.key_span.start].to_string();
        }
    }
    format!("{}  ", indent_of_line(text, obj.span.start))
}

fn nested_object(path: &[&str], literal: &str, depth: usize) -> String {
    let pad = "  ".repeat(depth + 1);
    let close = "  ".repeat(depth);
    let mut out = String::from("{\n");
    out.push_str(&pad);
    out.push_str(&member_text(path, literal, &pad));
    out.push('\n');
    out.push_str(&close);
    out.push('}');
    out
}

/// `"a": { "b": literal }` laid out over lines, starting at indentation `pad`.
fn member_text(path: &[&str], literal: &str, pad: &str) -> String {
    if path.len() == 1 {
        return format!("{}: {literal}", quote(path[0]));
    }
    let inner = format!("{pad}  ");
    format!(
        "{}: {{\n{inner}{}\n{pad}}}",
        quote(path[0]),
        member_text(&path[1..], literal, &inner)
    )
}

fn insert_member(text: &str, obj: &Node, path: &[&str], literal: &str) -> String {
    let pad = member_indent(text, obj);
    let member = member_text(path, literal, &pad);
    let members = obj.members().unwrap_or_default();
    let (at, insert) = match members.last() {
        Some(last) => (last.value.span.end, format!(",\n{pad}{member}")),
        None => {
            let inner = obj.span.start + 1..obj.span.end - 1;
            if text[inner.clone()].trim().is_empty() {
                let close = indent_of_line(text, obj.span.start);
                let mut out = text[..inner.start].to_string();
                out.push_str(&format!("\n{pad}{member}\n{close}"));
                out.push_str(&text[inner.end..]);
                return out;
            }
            // Comments inside an otherwise empty object: put the member right after `{`.
            (obj.span.start + 1, format!("\n{pad}{member}"))
        }
    };
    format!("{}{insert}{}", &text[..at], &text[at..])
}

fn remove_member(text: &str, holder: &Node, idx: usize) -> String {
    let members = holder.members().unwrap_or_default();
    let m = &members[idx];
    let start = m.key_span.start;
    let mut end = m.value.span.end;

    // A comma after the member goes with it; failing that, the one before it.
    let after = skip_blank(text, end);
    if text.as_bytes().get(after) == Some(&b',') {
        end = after + 1;
    } else if let Some(c) = comma_after(text, end) {
        // The comma is on a later line or behind a comment: drop it alone, so the comments
        // between stay, then cut the member (offsets before `c` are unchanged).
        let without = format!("{}{}", &text[..c], &text[c + 1..]);
        return cut_lines(&without, start, end);
    } else if idx > 0 {
        let prev_end = members[idx - 1].value.span.end;
        if let Some(c) = find_comma(text, prev_end, start) {
            // Drop the comma, then the member (offsets after `c` shift down by one).
            let without = format!("{}{}", &text[..c], &text[c + 1..]);
            return cut_lines(&without, start - 1, end - 1);
        }
    }
    cut_lines(text, start, end)
}

/// Delete `start..end`, and the whole line(s) if only blanks remain around it.
fn cut_lines(text: &str, start: usize, mut end: usize) -> String {
    // A `// comment` trailing the member on its own line describes it, so it goes too.
    let rest = skip_blank(text, end);
    if text[rest..].starts_with("//") {
        end = text[rest..].find('\n').map_or(text.len(), |i| rest + i);
    }
    let ls = line_start(text, start);
    let only_blank_before = text[ls..start].chars().all(|c| c == ' ' || c == '\t');
    let after = skip_blank(text, end);
    let at_eol = matches!(text.as_bytes().get(after), None | Some(b'\n' | b'\r'));
    let (from, to) = if only_blank_before && at_eol {
        let mut to = after;
        if text.as_bytes().get(to) == Some(&b'\r') {
            to += 1;
        }
        if text.as_bytes().get(to) == Some(&b'\n') {
            to += 1;
        }
        (ls, to)
    } else {
        (start, end)
    };
    format!("{}{}", &text[..from], &text[to..])
}

fn skip_blank(text: &str, mut at: usize) -> usize {
    while matches!(text.as_bytes().get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    at
}

/// The comma that follows `from` with only whitespace and comments between, if any.
fn comma_after(text: &str, from: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b',' => return Some(i),
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            _ => return None,
        }
    }
    None
}

/// The first comma in `from..to` that isn't inside a comment.
fn find_comma(text: &str, from: usize, to: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = from;
    while i < to {
        match b[i] {
            b',' => return Some(i),
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < to && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < to && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// tsconfig's dialect, read as data (cases from the reader `cho-semantic` used to have).
    #[test]
    fn values_skip_comments_and_trailing_commas() {
        let v = parse_value("{ // c\n \"a\": \"x//y\", /* b */ \"b\": [1,2,],\n}");
        assert_eq!(v, Some(json!({"a": "x//y", "b": [1, 2]})));
        // Found by the `workspace` fuzz target: a trailing comma before a commented-out last
        // option.
        let t = "{\"compilerOptions\": {\"outDir\": \"dist\", // built\n /* \"rootDir\": \"src\" */\n},}";
        assert_eq!(
            parse_value(t),
            Some(json!({"compilerOptions": {"outDir": "dist"}}))
        );
        assert_eq!(parse_value("[1, /* a */ // b\n]"), Some(json!([1])));
        assert_eq!(parse_value("[\"a,//b\", 2]"), Some(json!(["a,//b", 2])));
        assert_eq!(parse_value("{\"a\": 1, \"a\": 2}"), Some(json!({"a": 2})));
        assert_eq!(parse_value("  // only a comment\n"), None);
    }

    /// The comment-stripping reader this replaced accepted these: `[1/**/2]` as `[12]`, and
    /// an unterminated comment or string as whatever came before it.
    #[test]
    fn malformed_text_is_no_value() {
        for t in [
            "[1/**/2]",
            "{\"a\": 1} /* open",
            "{\"a\": \"open}",
            "[1e999]",
        ] {
            assert_eq!(parse_value(t), None, "{t}");
        }
    }

    fn obj(text: &str) -> Node {
        parse(text).unwrap().unwrap()
    }

    /// Found by the `settings_jsonc` fuzz target: a comma on a later line than the removed
    /// member (behind a comment) was left behind, so the file no longer parsed.
    #[test]
    fn remove_first_member_whose_comma_is_on_a_later_line() {
        let text = "{\"b\": null // c\n, \"a\": 1}\n";
        let out = remove(text, &["b"]).unwrap();
        assert_eq!(out, "{\n \"a\": 1}\n");
        assert!(parse(&out).is_ok());
        let text = "{\"b\": 1 /* x */\n  ,\n}";
        let out = remove(text, &["b"]).unwrap();
        assert!(parse(&out).is_ok() && out.contains("/* x */"), "{out:?}");
    }

    #[test]
    fn parses_comments_and_trailing_commas() {
        let n = obj("// head\n{\n  /* c */ \"a\": true, // tail\n  \"b\": [1, 2,],\n}\n");
        assert_eq!(n.get("a").unwrap().kind, NodeKind::Bool(true));
        assert!(matches!(n.get("b").unwrap().kind, NodeKind::Array(ref v) if v.len() == 2));
    }

    #[test]
    fn empty_and_comment_only_are_none() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("  // nothing\n/* here */").unwrap(), None);
    }

    #[test]
    fn errors_have_positions() {
        let e = parse("{\n  \"a\": tru\n}").unwrap_err();
        assert_eq!((e.line, e.column), (2, 8));
        assert!(parse("{\"a\": 1").is_err());
        assert!(parse("{\"a\": 1} x").is_err());
        assert!(parse("{\"a\" 1}").is_err());
        assert!(parse("{\"a\": \"x}").is_err());
        assert!(parse("/* open").is_err());
        assert!(parse("{'a': 1}").is_err());
        let deep = "[".repeat(100);
        assert!(parse(&deep).unwrap_err().message.contains("deeply"));
    }

    #[test]
    fn strings_unescape() {
        let n = obj(r#"{"kA": "a\n\"é😀"}"#);
        assert_eq!(n.get("kA").unwrap().kind, NodeKind::Str("a\n\"é😀".into()));
    }

    #[test]
    fn non_ascii_columns_count_characters() {
        let e = parse("{\"é\": x}").unwrap_err();
        assert_eq!((e.line, e.column), (1, 7));
    }

    #[test]
    fn replace_keeps_everything_else() {
        let src = "// keep me\n{\n  \"a\": true, // why\n  \"zzz\": 1, /* x */\n  \"b\": 5000\n}\n";
        let out = set(src, &["a"], "false").unwrap();
        assert_eq!(out, src.replace("\"a\": true", "\"a\": false"));
        let out = set(src, &["b"], "7").unwrap();
        assert!(
            out.contains("\"b\": 7\n") && out.contains("// keep me") && out.contains("/* x */")
        );
    }

    #[test]
    fn insert_appends_with_commas_and_indent() {
        let out = set("{\n    \"a\": 1\n}\n", &["b"], "2").unwrap();
        assert_eq!(out, "{\n    \"a\": 1,\n    \"b\": 2\n}\n");
        // Trailing comma already there.
        let out = set("{\n  \"a\": 1, // c\n}\n", &["b"], "2").unwrap();
        assert_eq!(out, "{\n  \"a\": 1,\n  \"b\": 2, // c\n}\n");
        assert!(parse(&out).is_ok());
        // Empty objects.
        assert_eq!(set("{}", &["a"], "1").unwrap(), "{\n  \"a\": 1\n}");
        assert_eq!(set("{ }\n", &["a"], "1").unwrap(), "{\n  \"a\": 1\n}\n");
        let out = set("{ // nothing yet\n}", &["a"], "1").unwrap();
        assert!(parse(&out).is_ok() && out.contains("// nothing yet"));
    }

    #[test]
    fn empty_file_gets_an_object_after_its_comments() {
        let out = set("// settings\n", &["a"], "true").unwrap();
        assert_eq!(out, "// settings\n{\n  \"a\": true\n}\n");
        assert_eq!(set("", &["a"], "true").unwrap(), "{\n  \"a\": true\n}\n");
    }

    #[test]
    fn nested_paths_create_objects() {
        let out = set("{\n  \"repos\": {}\n}\n", &["repos", "/x/y", "k"], "false").unwrap();
        let n = obj(&out);
        assert_eq!(
            n.get("repos")
                .unwrap()
                .get("/x/y")
                .unwrap()
                .get("k")
                .unwrap()
                .kind,
            NodeKind::Bool(false)
        );
        let out2 = set(&out, &["repos", "/x/y", "j"], "1").unwrap();
        assert!(
            obj(&out2)
                .get("repos")
                .unwrap()
                .get("/x/y")
                .unwrap()
                .get("j")
                .is_some()
        );
        let fresh = set("", &["repos", "/p", "k"], "1").unwrap();
        assert!(parse(&fresh).is_ok());
    }

    #[test]
    fn shape_errors_leave_the_file_alone() {
        assert!(matches!(set("[1]", &["a"], "1"), Err(EditError::Shape(_))));
        assert!(matches!(
            set("{\"repos\": 3}", &["repos", "x"], "1"),
            Err(EditError::Shape(_))
        ));
        assert!(matches!(set("{", &["a"], "1"), Err(EditError::Parse(_))));
    }

    #[test]
    fn duplicate_keys_edit_the_one_readers_use() {
        let out = set("{\"a\": 1, \"a\": 2}", &["a"], "9").unwrap();
        assert_eq!(out, "{\"a\": 1, \"a\": 9}");
    }

    #[test]
    fn remove_takes_the_comma_and_line() {
        let src = "{\n  \"a\": 1,\n  \"b\": 2, // note\n  \"c\": 3\n}\n";
        assert_eq!(
            remove(src, &["a"]).unwrap(),
            "{\n  \"b\": 2, // note\n  \"c\": 3\n}\n"
        );
        assert_eq!(
            remove(src, &["b"]).unwrap(),
            "{\n  \"a\": 1,\n  \"c\": 3\n}\n"
        );
        // Last member, no trailing comma: the comma before it goes.
        assert_eq!(
            remove(src, &["c"]).unwrap(),
            "{\n  \"a\": 1,\n  \"b\": 2 // note\n}\n"
        );
        assert!(parse(&remove(src, &["c"]).unwrap()).is_ok());
        assert_eq!(remove(src, &["missing"]).unwrap(), src);
        assert_eq!(remove("{\n  \"a\": 1\n}\n", &["a"]).unwrap(), "{\n}\n");
        // Single-line objects keep a stray space but stay valid.
        assert_eq!(
            remove("{\"a\": 1, \"b\": 2}", &["b"]).unwrap(),
            "{\"a\": 1 }"
        );
    }

    #[test]
    fn remove_collapses_emptied_parents_but_not_the_root() {
        let src = "{\n  \"repos\": {\n    \"/x\": {\n      \"k\": 1\n    },\n    \"/y\": {\n      \"k\": 2\n    }\n  }\n}\n";
        let out = remove(src, &["repos", "/x", "k"]).unwrap();
        let n = obj(&out);
        assert!(n.get("repos").unwrap().get("/x").is_none());
        assert!(n.get("repos").unwrap().get("/y").is_some());
        let out = remove(&out, &["repos", "/y", "k"]).unwrap();
        assert!(parse(&out).unwrap().unwrap().members().unwrap().is_empty());
    }
}

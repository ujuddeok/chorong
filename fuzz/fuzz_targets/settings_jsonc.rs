#![no_main]
//! Chorong edits the user's own settings file in place (`jsonc::set`, `jsonc::remove`, and the
//! store's `set`/`unset`), keeping their comments. The first byte picks the mode:
//!
//! - 0..=3 (edits): a generated JSONC document (comments, trailing commas, nesting, unicode,
//!   duplicate and escaped keys, CRLF) and up to four edits. After each: the text parses; the
//!   edited path has exactly the new value or is gone (compared as a whole-document model,
//!   so every other value is unchanged too); every comment outside the removed or replaced
//!   member survives verbatim; an edit that changes nothing leaves the bytes identical.
//! - 4..=5 (arbitrary text): `parse` never panics, its spans are in bounds, and edits on it
//!   either fail or produce text that parses.
//! - 6 (deep nesting): hundreds of thousands of `[` or `{"a":` return an error; they don't
//!   overflow the stack.
//! - 7 (store): the same generated document is Chorong's `settings.json`; `SettingsStore::set`
//!   then `unset` of a known key change only that key and keep comments, and a file with a
//!   syntax error is left untouched.
use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use cho_settings::jsonc::{self, EditError, Node, NodeKind};
use cho_settings::{Kind, Scope, SettingsStore, Value, defs};
use std::path::PathBuf;

// ---- the document model ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Val>),
    Obj(Vec<(String, Val)>),
}

fn val(n: &Node) -> Val {
    match &n.kind {
        NodeKind::Null => Val::Null,
        NodeKind::Bool(b) => Val::Bool(*b),
        NodeKind::Number(s) => Val::Num(s.clone()),
        NodeKind::Str(s) => Val::Str(s.clone()),
        NodeKind::Array(v) => Val::Arr(v.iter().map(val).collect()),
        NodeKind::Object(m) => Val::Obj(m.iter().map(|m| (m.key.clone(), val(&m.value))).collect()),
    }
}

/// What `set` should produce, mirroring its documentation: the last member with a key is
/// the one edited; missing objects are created; a non-object on the way is a shape error.
fn model_set(v: &mut Val, path: &[String], new: &Val) -> Result<(), ()> {
    let Val::Obj(ms) = v else { return Err(()) };
    let Some((key, rest)) = path.split_first() else {
        unreachable!()
    };
    match ms.iter().rposition(|(k, _)| k == key) {
        Some(i) if rest.is_empty() => {
            ms[i].1 = new.clone();
            Ok(())
        }
        Some(i) => match ms[i].1 {
            Val::Obj(_) => model_set(&mut ms[i].1, rest, new),
            _ => Err(()),
        },
        None => {
            let mut leaf = new.clone();
            for k in path[1..].iter().rev() {
                leaf = Val::Obj(vec![(k.clone(), leaf)]);
            }
            ms.push((key.clone(), leaf));
            Ok(())
        }
    }
}

/// The member `remove` deletes: the last one matching the path, or the outermost ancestor it
/// would leave empty. `None` when the path isn't there. Returns the member index at each
/// depth down to the removed one.
fn remove_target(root: &Node, path: &[String]) -> Option<Vec<usize>> {
    let mut idxs: Vec<usize> = Vec::new();
    let mut obj = root;
    let mut lens: Vec<usize> = Vec::new();
    for key in path {
        let ms = obj.members()?;
        let i = ms.iter().rposition(|m| &m.key == key)?;
        idxs.push(i);
        lens.push(ms.len());
        obj = &ms[i].value;
    }
    let mut level = idxs.len() - 1;
    while level > 0 && lens[level] == 1 {
        level -= 1;
    }
    idxs.truncate(level + 1);
    Some(idxs)
}

fn member_at<'a>(root: &'a Node, idxs: &[usize]) -> &'a jsonc::Member {
    let mut obj = root;
    let mut m = None;
    for &i in idxs {
        let ms = obj.members().unwrap();
        m = Some(&ms[i]);
        obj = &ms[i].value;
    }
    m.unwrap()
}

fn model_remove(v: &mut Val, idxs: &[usize]) {
    let Val::Obj(ms) = v else { unreachable!() };
    if idxs.len() == 1 {
        ms.remove(idxs[0]);
    } else {
        model_remove(&mut ms[idxs[0]].1, &idxs[1..]);
    }
}

fn find<'a>(root: &'a Node, path: &[String]) -> Option<&'a Node> {
    let mut n = root;
    for k in path {
        n = n.members()?.iter().rev().find(|m| &m.key == k)?.value_ref();
    }
    Some(n)
}

trait ValueRef {
    fn value_ref(&self) -> &Node;
}
impl ValueRef for jsonc::Member {
    fn value_ref(&self) -> &Node {
        &self.value
    }
}

fn object_paths(n: &Node, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    for m in n.members().unwrap_or_default() {
        prefix.push(m.key.clone());
        out.push(prefix.clone());
        object_paths(&m.value, prefix, out);
        prefix.pop();
    }
}

// ---- the generator ---------------------------------------------------------------------

const KEYS: [&str; 11] = [
    "a", "b", "c", "ü", "k.k", "", "x y", "\"q\"", "a\\b", "😀", "repos",
];

struct Gen<'a> {
    u: Unstructured<'a>,
    crlf: bool,
    comments: Vec<String>,
    out: String,
}

impl<'a> Gen<'a> {
    fn pick(&mut self, n: usize) -> usize {
        self.u.int_in_range(0..=n - 1).unwrap_or(0)
    }

    fn nl(&mut self) {
        self.out.push_str(if self.crlf { "\r\n" } else { "\n" });
    }

    fn comment(&mut self, line: bool) {
        let id = self.comments.len();
        let body = match self.pick(3) {
            0 => format!("c{id}"),
            1 => format!("c{id} ü😀 */ not the end"),
            _ => format!("c{id} \"quoted\" , : {{ }}"),
        };
        if line {
            let text = format!("// {body}");
            self.out.push_str(&text);
            self.comments.push(text);
            self.nl();
        } else {
            let body = body.replace("*/", "* /");
            let nl = if self.pick(4) == 0 {
                if self.crlf { "\r\n" } else { "\n" }
            } else {
                " "
            };
            let text = format!("/* {body}{nl}*/");
            self.out.push_str(&text);
            self.comments.push(text);
        }
    }

    fn ws(&mut self) {
        for _ in 0..self.pick(3) {
            match self.pick(7) {
                0 | 1 => self.out.push(' '),
                2 => {
                    self.nl();
                    let pad = ["", "  ", "\t", "    "][self.pick(4)];
                    self.out.push_str(pad);
                }
                3 | 4 => {
                    self.out.push(' ');
                    self.comment(false);
                }
                5 => {
                    self.out.push(' ');
                    self.comment(true);
                }
                _ => {
                    self.out.push_str("  ");
                    self.comment(true);
                    self.out.push_str("  ");
                }
            }
        }
    }

    fn key(&mut self) {
        let k = KEYS[self.pick(KEYS.len())];
        if k == "a" && self.pick(4) == 0 {
            self.out.push_str("\"\\u0061\""); // the same key, written differently
        } else if k == "ü" && self.pick(3) == 0 {
            self.out.push_str("\"\\u00fc\"");
        } else {
            self.out.push_str(&jsonc::quote(k));
        }
    }

    fn scalar(&mut self) {
        match self.pick(9) {
            0 => self.out.push_str("null"),
            1 => self.out.push_str("true"),
            2 => self.out.push_str("false"),
            3 => {
                let n = ["0", "-1", "12", "3.5", "1e3", "-0.25E-2"][self.pick(6)];
                self.out.push_str(n)
            }
            4 => self.out.push_str("300"),
            5 => self
                .out
                .push_str("\"héllo 😀 \\n \\\" \\u00e9 \\ud83d\\ude00\""),
            6 => self.out.push_str("\"\""),
            7 => self.out.push_str("\"// not a comment /* nor this */\""),
            _ => self.out.push_str("\"a, b: {c}\""),
        }
    }

    fn value(&mut self, depth: usize) {
        let kind = if depth >= 4 {
            self.pick(2) + 2
        } else {
            self.pick(5)
        };
        match kind {
            0 | 1 => self.object(depth),
            2 => self.array(depth),
            _ => self.scalar(),
        }
    }

    fn list(&mut self, depth: usize, open: char, close: char, keyed: bool) {
        self.out.push(open);
        let n = self.pick(5);
        let trailing = self.pick(3) == 0;
        for i in 0..n {
            self.ws();
            if keyed {
                self.key();
                self.ws();
                self.out.push(':');
                self.ws();
            }
            self.value(depth + 1);
            self.ws();
            if i + 1 < n || trailing {
                self.out.push(',');
            }
        }
        self.ws();
        self.out.push(close);
    }

    fn object(&mut self, depth: usize) {
        self.list(depth, '{', '}', true);
    }

    fn array(&mut self, depth: usize) {
        self.list(depth, '[', ']', false);
    }

    /// A whole document.
    fn document(&mut self) {
        self.ws();
        match self.pick(32) {
            0 => {} // empty, or only comments
            1 => self.array(0),
            2 => self.scalar(),
            _ => self.object(0),
        }
        self.ws();
        if self.pick(2) == 0 {
            self.nl();
        }
    }

    /// A JSON literal (no comments) for `set`.
    fn literal(&mut self, depth: usize) -> String {
        match self.pick(if depth > 1 { 7 } else { 9 }) {
            0 => "null".into(),
            1 => "true".into(),
            2 => "false".into(),
            3 => ["0", "-1", "12", "3.5", "1e3", "300"][self.pick(6)].into(),
            4 => "\"\"".into(),
            5 | 6 => {
                let s: String = self.u.arbitrary().unwrap_or_default();
                jsonc::quote(&s)
            }
            7 => format!("[{}, {}]", self.literal(depth + 1), self.literal(depth + 1)),
            _ => format!(
                "{{{}: {}}}",
                jsonc::quote(KEYS[self.pick(KEYS.len())]),
                self.literal(depth + 1)
            ),
        }
    }
}

fn new_gen(data: &[u8]) -> Gen<'_> {
    let mut u = Unstructured::new(data);
    let crlf = u.arbitrary().unwrap_or(false);
    Gen {
        u,
        crlf,
        comments: Vec::new(),
        out: String::new(),
    }
}

// ---- modes -----------------------------------------------------------------------------

fn pick_path(g: &mut Gen, root: Option<&Node>) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(r) = root {
        object_paths(r, &mut Vec::new(), &mut paths);
    }
    let key = |g: &mut Gen| KEYS[g.pick(KEYS.len())].to_string();
    let mut path = match g.pick(4) {
        0 | 1 if !paths.is_empty() => paths[g.pick(paths.len())].clone(),
        2 if !paths.is_empty() => {
            let mut p = paths[g.pick(paths.len())].clone();
            for _ in 0..=g.pick(2) {
                p.push(key(g));
            }
            p
        }
        _ => (0..=g.pick(3)).map(|_| key(g)).collect(),
    };
    path.truncate(5);
    path
}

fn edits(data: &[u8]) {
    let mut g = new_gen(data);
    g.document();
    let mut text = std::mem::take(&mut g.out);
    let mut comments = std::mem::take(&mut g.comments);
    // Sometimes break the document: the edit must then fail cleanly (or, if it still
    // parses, behave as usual).
    if g.pick(12) == 0 && !text.is_empty() {
        let a = g.pick(text.len());
        let b = (a + 1 + g.pick(4)).min(text.len());
        if text.is_char_boundary(a) && text.is_char_boundary(b) {
            text.replace_range(a..b, "");
            comments.clear();
        }
    }
    for _ in 0..=g.pick(4) {
        let parsed = jsonc::parse(&text);
        let root = parsed.as_ref().ok().and_then(Option::as_ref);
        let path = pick_path(&mut g, root);
        let refs: Vec<&str> = path.iter().map(String::as_str).collect();
        let is_set = g.pick(5) != 0;
        let literal = g.literal(0);
        let result = if is_set {
            jsonc::set(&text, &refs, &literal)
        } else {
            jsonc::remove(&text, &refs)
        };
        let ctx = || {
            format!(
                "{} {path:?} {literal}\n{text:?}",
                if is_set { "set" } else { "remove" }
            )
        };

        let root = match parsed {
            Err(_) => {
                assert!(
                    matches!(result, Err(EditError::Parse(_))),
                    "{}: {result:?}",
                    ctx()
                );
                return;
            }
            Ok(r) => r,
        };
        // The model of the document before the edit.
        let mut model = match &root {
            Some(r) => val(r),
            None => Val::Obj(Vec::new()),
        };
        if root.as_ref().is_some_and(|r| r.members().is_none()) {
            assert!(
                matches!(result, Err(EditError::Shape(_))),
                "{}: {result:?}",
                ctx()
            );
            return;
        }
        let mut lost_ok: Vec<std::ops::Range<usize>> = Vec::new();
        let new_text = if is_set {
            let lit_val = val(&jsonc::parse(&literal).unwrap().unwrap());
            if let Some(r) = &root {
                if let Some(n) = find(r, &path) {
                    lost_ok.push(n.span.clone()); // comments inside the replaced value
                    // Setting a value to its own text changes nothing.
                    let same = jsonc::set(&text, &refs, &text[n.span.clone()]);
                    assert_eq!(same.as_deref().ok(), Some(text.as_str()), "{}", ctx());
                }
            }
            match model_set(&mut model, &path, &lit_val) {
                Err(()) => {
                    assert!(
                        matches!(result, Err(EditError::Shape(_))),
                        "{}: {result:?}",
                        ctx()
                    );
                    return;
                }
                Ok(()) => {}
            }
            let new = result.unwrap_or_else(|e| panic!("{}: {e}", ctx()));
            // Setting twice is the same as once.
            let twice = jsonc::set(&new, &refs, &literal).expect("second set");
            assert_eq!(twice, new, "set twice differs: {}", ctx());
            new
        } else {
            let new = result.unwrap_or_else(|e| panic!("{}: {e}", ctx()));
            match root
                .as_ref()
                .and_then(|r| remove_target(r, &path).map(|t| (r, t)))
            {
                None => assert_eq!(
                    new,
                    text,
                    "removing a missing path changed the text: {}",
                    ctx()
                ),
                Some((r, idxs)) => {
                    let m = member_at(r, &idxs);
                    let end = m.value.span.end;
                    lost_ok.push(m.key_span.start..end);
                    // A `// comment` trailing the member on its line goes with it.
                    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
                    lost_ok.push(end..line_end);
                    model_remove(&mut model, &idxs);
                }
            }
            new
        };

        let again = jsonc::parse(&new_text).unwrap_or_else(|e| {
            panic!(
                "edit made unparsable text ({e}): {}\n-> {new_text:?}",
                ctx()
            )
        });
        let got = again.as_ref().map_or(Val::Obj(Vec::new()), val);
        assert_eq!(got, model, "{}\n-> {new_text:?}", ctx());

        // Comments outside the removed member are still there, verbatim.
        let mut kept = Vec::new();
        for c in &comments {
            if new_text.contains(c.as_str()) {
                kept.push(c.clone());
                continue;
            }
            let ok = text.match_indices(c.as_str()).any(|(at, _)| {
                lost_ok
                    .iter()
                    .any(|r| r.start <= at && at < r.end.max(r.start + 1))
            });
            assert!(ok, "comment {c:?} lost: {}\n-> {new_text:?}", ctx());
        }
        comments = kept;
        text = new_text;
    }
}

fn raw_text(s: &str) {
    let Ok(parsed) = jsonc::parse(s) else {
        // A broken document is never edited.
        assert!(jsonc::set(s, &["a"], "1").is_err());
        assert!(jsonc::remove(s, &["a"]).is_err());
        return;
    };
    if let Some(n) = &parsed {
        check_spans(s, n);
    }
    for r in [jsonc::set(s, &["a", "b"], "[1]"), jsonc::remove(s, &["a"])] {
        if let Ok(t) = r {
            assert!(jsonc::parse(&t).is_ok(), "edit of {s:?} made {t:?}");
        }
    }
}

fn check_spans(s: &str, n: &Node) {
    let r = &n.span;
    assert!(r.start <= r.end && r.end <= s.len());
    assert!(s.is_char_boundary(r.start) && s.is_char_boundary(r.end));
    match &n.kind {
        NodeKind::Array(v) => v.iter().for_each(|c| check_spans(s, c)),
        NodeKind::Object(ms) => {
            for m in ms {
                assert!(m.key_span.start <= m.key_span.end && m.key_span.end <= m.value.span.start);
                assert!(s.is_char_boundary(m.key_span.start) && s.is_char_boundary(m.key_span.end));
                check_spans(s, &m.value);
            }
        }
        _ => {}
    }
}

fn deep(data: &[u8]) {
    let n = match data.first().copied().unwrap_or(0) % 4 {
        0 => 100_000,
        1 => 300_000,
        2 => 1_000,
        _ => 70,
    };
    let close = data.get(1).is_some_and(|b| b & 1 == 1);
    for (open, shut) in [("[", "]"), ("{\"a\":", "}")] {
        let mut s = open.repeat(n);
        if close {
            s.push('1');
            s.push_str(&shut.repeat(n));
        }
        assert!(jsonc::parse(&s).is_err(), "{n} levels parsed");
        assert!(jsonc::set(&s, &["a"], "1").is_err());
    }
    // A shallow one still works.
    let ok = format!("{}1{}", "[".repeat(10), "]".repeat(10));
    assert!(jsonc::parse(&ok).is_ok());
}

fn scratch() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cho-fuzz-settings-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

fn store(data: &[u8]) {
    let dir = scratch();
    let file = dir.join("settings.json");
    let _ = std::fs::remove_file(dir.join("repos.json"));
    let mut g = new_gen(data);
    // Known keys make most members meaningful.
    g.document();
    let text = std::mem::take(&mut g.out);
    let comments = std::mem::take(&mut g.comments);
    // Rewrite some member keys to real setting keys.
    let text = {
        let mut t = text;
        for (i, d) in defs().iter().take(8).enumerate() {
            let from = format!("\"{}\"", KEYS[i % KEYS.len()]);
            if g.pick(2) == 0 {
                t = t.replacen(&from, &jsonc::quote(d.key), 1);
            }
        }
        t
    };
    std::fs::write(&file, &text).unwrap();
    let mut s = SettingsStore::open(Some(dir.clone()));
    let d = &defs()[g.pick(defs().len())];
    let new = match d.kind {
        Kind::Bool => Value::Bool(g.pick(2) == 0),
        Kind::UInt { min, max } => {
            let span = max - min;
            Value::UInt(
                min + if span == 0 {
                    0
                } else {
                    g.u.int_in_range(0..=span).unwrap_or(0)
                },
            )
        }
        Kind::Font { .. } => {
            let t: String = g.u.arbitrary().unwrap_or_default();
            let t = t.trim().chars().take(40).collect::<String>();
            Value::text(if t.is_empty() { "Mono".to_string() } else { t })
        }
    };
    let parsed = jsonc::parse(&text);
    let healthy =
        matches!(&parsed, Ok(None)) || matches!(&parsed, Ok(Some(r)) if r.members().is_some());
    let others = |s: &SettingsStore| -> Vec<(&'static str, Option<Value>)> {
        defs()
            .iter()
            .filter(|o| o.key != d.key)
            .map(|o| (o.key, s.global_value(o.key)))
            .collect()
    };
    let before = others(&s);
    let dup = parsed
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .is_some_and(|r| {
            r.members()
                .unwrap_or_default()
                .iter()
                .filter(|m| m.key == d.key)
                .count()
                > 1
        });

    let r = s.set(Scope::Global, d.key, new.clone());
    if !healthy {
        assert!(r.is_err(), "set on a broken file succeeded: {text:?}");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            text,
            "broken file was rewritten"
        );
        return;
    }
    r.unwrap_or_else(|e| panic!("set {}: {e}\n{text:?}", d.key));
    assert_eq!(
        s.global_value(d.key),
        Some(new),
        "{} after set\n{text:?}",
        d.key
    );
    assert_eq!(
        others(&s),
        before,
        "set {} changed another setting\n{text:?}",
        d.key
    );
    let set_text = std::fs::read_to_string(&file).unwrap();
    assert!(jsonc::parse(&set_text).is_ok(), "{set_text:?}");
    // Reopening gives the same answer as the live store.
    let reopened = SettingsStore::open(Some(dir.clone()));
    assert_eq!(reopened.global_value(d.key), s.global_value(d.key));

    s.unset(Scope::Global, d.key)
        .unwrap_or_else(|e| panic!("unset {}: {e}\n{set_text:?}", d.key));
    let after = std::fs::read_to_string(&file).unwrap();
    assert!(jsonc::parse(&after).is_ok(), "{after:?}");
    if !dup {
        assert_eq!(
            s.global_value(d.key),
            None,
            "unset left {} behind\n{after:?}",
            d.key
        );
    }
    assert_eq!(
        others(&s),
        before,
        "unset {} changed another setting\n{set_text:?}",
        d.key
    );
    for c in &comments {
        // A comment inside the removed member or on its line may go.
        if !set_text.contains(c.as_str()) {
            continue; // lost to the set (inside a replaced value): covered by the edits mode
        }
        if !after.contains(c.as_str()) {
            let at = set_text.find(c.as_str()).unwrap();
            let inside = jsonc::parse(&set_text).ok().flatten().is_some_and(|r| {
                r.members().unwrap_or_default().iter().any(|m| {
                    m.key == d.key
                        && at >= m.key_span.start
                        && at
                            < set_text[m.value.span.end..]
                                .find('\n')
                                .map_or(set_text.len(), |i| m.value.span.end + i)
                })
            });
            assert!(
                inside,
                "comment {c:?} lost by unset {}\n{set_text:?}\n-> {after:?}",
                d.key
            );
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    match mode % 8 {
        0..=3 => edits(rest),
        4 | 5 => {
            if let Ok(s) = std::str::from_utf8(rest) {
                raw_text(s);
            }
        }
        6 => deep(rest),
        _ => store(rest),
    }
});

//! Finds the icon names a crate's sources pass to `icon()` / `small_icon()`, so a test can
//! check each one is bundled. `icon()` only `debug_assert!`s, so a missing icon panics in a
//! debug build and draws nothing in a release one; no test opens every list that draws one.
//!
//! A source scan, not a parser. An icon name counts as used when it is a string literal:
//! - in an icon argument: the first argument of `icon(..)` / `small_icon(..)`, or the argument
//!   in the place of a parameter named `icon`, `glyph` or `icon_name` of a function (any file)
//!   or closure (until the end of its block) defined in the scanned sources;
//! - the value of a field named `icon`, `glyph` or `icon_name`;
//! - a value an `fn *_icon` / `*_glyph` returning `&'static str` returns (after `=>`/`return`);
//! - the matching element of each tuple in `let (.., icon, ..) = ..;` (or `glyph`).
//!
//! Shared by `crates/cho-layout-ui/tests/icon_names.rs` and chorong-app's `assets.rs` tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const ICON_PARAMS: &[&str] = &["icon", "glyph", "icon_name"];

/// One literal icon name in the sources.
#[derive(Debug, Clone)]
pub struct Use {
    pub file: PathBuf,
    pub line: usize,
    pub name: String,
    /// The call path when the name is the argument of a direct icon call, such as
    /// `crate::assets::icon` or `small_icon`; empty otherwise.
    pub via: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
}

#[derive(Debug, Clone)]
struct T {
    tok: Tok,
    line: usize,
}

/// Every `.rs` file under `dir`, sorted.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The icon names used in `files` (test modules excluded).
pub fn icon_uses(files: &[PathBuf]) -> Vec<Use> {
    #[allow(clippy::disallowed_methods)] // reads this repository's own sources
    let sources: Vec<(PathBuf, Vec<T>)> = files
        .iter()
        .map(|f| {
            let text = std::fs::read_to_string(f).unwrap();
            (f.clone(), strip_test_mods(tokenize(&text)))
        })
        .collect();
    // Functions with an icon parameter, from every file: name -> argument index.
    let mut fns: HashMap<String, usize> = HashMap::new();
    for (_, toks) in &sources {
        for i in 0..toks.len() {
            if ident(toks, i) == Some("fn")
                && let Some(name) = ident(toks, i + 1)
                && let Some(open) = find_punct(toks, i + 2, '(')
                && let Some(idx) = icon_param(toks, &split_args(toks, open, '(', ')'))
            {
                fns.insert(name.to_string(), idx);
            }
        }
    }
    let mut uses = Vec::new();
    for (file, toks) in &sources {
        scan_file(file, toks, &fns, &mut uses);
    }
    uses
}

fn scan_file(file: &Path, toks: &[T], fns: &HashMap<String, usize>, uses: &mut Vec<Use>) {
    let mut push = |t: &T, via: &str| {
        if let Tok::Str(s) = &t.tok {
            uses.push(Use {
                file: file.to_path_buf(),
                line: t.line,
                name: s.clone(),
                via: via.to_string(),
            });
        }
    };
    // Closures with an icon parameter in scope: (name, arg index, brace depth at definition).
    let mut closures: Vec<(String, usize, usize)> = Vec::new();
    let mut depth = 0usize;
    // The brace depth at which an `fn *_icon -> &'static str` body opened, while inside it.
    let mut icon_fn_depth: Option<usize> = None;
    let mut icon_fn_pending = false;
    for i in 0..toks.len() {
        match toks[i].tok {
            Tok::Punct('{') => {
                depth += 1;
                if icon_fn_pending {
                    icon_fn_depth = Some(depth);
                    icon_fn_pending = false;
                }
            }
            Tok::Punct('}') => {
                if icon_fn_depth == Some(depth) {
                    icon_fn_depth = None;
                }
                depth = depth.saturating_sub(1);
                closures.retain(|c| c.2 <= depth);
            }
            _ => {}
        }
        // fn *_icon / *_glyph(..) -> &'static str { .. }
        if ident(toks, i) == Some("fn")
            && let Some(name) = ident(toks, i + 1)
            && (name.ends_with("_icon") || name.ends_with("_glyph"))
            && let Some(open) = find_punct(toks, i + 2, '(')
        {
            let close = matching(toks, open, '(', ')');
            let ret: Vec<&Tok> = toks[close + 1..].iter().take(6).map(|t| &t.tok).collect();
            icon_fn_pending = ret.len() == 6
                && ret[0] == &Tok::Punct('-')
                && ret[1] == &Tok::Punct('>')
                && ret[2] == &Tok::Punct('&')
                && ret[3] == &Tok::Punct('\'')
                && ret[4] == &Tok::Ident("static".into())
                && ret[5] == &Tok::Ident("str".into());
        }
        if icon_fn_depth.is_some()
            && let Tok::Str(_) = toks[i].tok
        {
            let prev = |k: usize| i.checked_sub(k).map(|j| &toks[j].tok);
            let after_arrow =
                prev(1) == Some(&Tok::Punct('>')) && prev(2) == Some(&Tok::Punct('='));
            let after_arrow_brace = prev(1) == Some(&Tok::Punct('{'))
                && prev(2) == Some(&Tok::Punct('>'))
                && prev(3) == Some(&Tok::Punct('='));
            let after_return = prev(1) == Some(&Tok::Ident("return".into()));
            if after_arrow || after_arrow_brace || after_return {
                push(&toks[i], "");
            }
        }
        // let NAME = |.., icon: .., ..| / let NAME = move |..|
        if ident(toks, i) == Some("let")
            && let Some(name) = ident(toks, i + 1)
            && toks.get(i + 2).map(|t| &t.tok) == Some(&Tok::Punct('='))
        {
            let bar = if ident(toks, i + 3) == Some("move") {
                i + 4
            } else {
                i + 3
            };
            if toks.get(bar).map(|t| &t.tok) == Some(&Tok::Punct('|'))
                && let Some(end) = find_punct(toks, bar + 1, '|')
                && let Some(idx) = icon_param(toks, &split_top(toks, bar + 1, end))
            {
                closures.push((name.to_string(), idx, depth));
            }
        }
        // let (icon, ..) = ..;
        if ident(toks, i) == Some("let")
            && toks.get(i + 1).map(|t| &t.tok) == Some(&Tok::Punct('('))
        {
            let pattern = split_args(toks, i + 1, '(', ')');
            if let Some(k) = icon_param(toks, &pattern) {
                // Each tuple of the pattern's size in the statement: its k-th item.
                let mut j = matching(toks, i + 1, '(', ')') + 1;
                let mut d = 0i32;
                while j < toks.len() {
                    match toks[j].tok {
                        Tok::Punct('(' | '{' | '[') => d += 1,
                        Tok::Punct(')' | '}' | ']') => d -= 1,
                        Tok::Punct(';') if d == 0 => break,
                        _ => {}
                    }
                    let is_tuple = toks[j].tok == Tok::Punct('(') && ident(toks, j - 1).is_none();
                    if is_tuple {
                        let items = split_args(toks, j, '(', ')');
                        if items.len() == pattern.len() {
                            for t in direct_strs(toks, items[k].clone()) {
                                push(t, "");
                            }
                        }
                    }
                    j += 1;
                }
            }
        }
        // icon: "name" (a field)
        if let Some(n) = ident(toks, i)
            && ICON_PARAMS.contains(&n)
            && toks.get(i + 1).map(|t| &t.tok) == Some(&Tok::Punct(':'))
            && toks.get(i + 2).map(|t| &t.tok) != Some(&Tok::Punct(':'))
            && let Some(t) = toks.get(i + 2)
            && matches!(t.tok, Tok::Str(_))
        {
            push(t, "");
        }
        // Calls: icon(..), small_icon(..), and functions or closures with an icon parameter.
        if let Some(name) = ident(toks, i)
            && toks.get(i + 1).map(|t| &t.tok) == Some(&Tok::Punct('('))
            && (i == 0 || ident(toks, i - 1) != Some("fn"))
        {
            let idx = match name {
                "icon" | "small_icon" => Some(0),
                _ => closures
                    .iter()
                    .rev()
                    .find(|c| c.0 == name)
                    .map(|c| c.1)
                    .or_else(|| fns.get(name).copied()),
            };
            if let Some(idx) = idx {
                let via = if matches!(name, "icon" | "small_icon") {
                    call_path(toks, i)
                } else {
                    String::new()
                };
                let args = split_args(toks, i + 1, '(', ')');
                if let Some(arg) = args.get(idx) {
                    for t in direct_strs(toks, arg.clone()) {
                        push(t, &via);
                    }
                }
            }
        }
    }
}

/// The string literals of an expression that are not inside a nested call or index, such as
/// both names in `if open { "chevron-down" } else { "chevron-right" }` but not the `""` in
/// `file_icon(name.map_or("", ..))`.
fn direct_strs(toks: &[T], range: std::ops::Range<usize>) -> impl Iterator<Item = &T> {
    let mut d = 0i32;
    toks[range].iter().filter(move |t| {
        match t.tok {
            Tok::Punct('(' | '[') => d += 1,
            Tok::Punct(')' | ']') => d -= 1,
            _ => {}
        }
        d == 0 && matches!(t.tok, Tok::Str(_))
    })
}

/// `a::b::name` for the identifier at `i`.
fn call_path(toks: &[T], i: usize) -> String {
    let mut parts = vec![ident(toks, i).unwrap().to_string()];
    let mut j = i;
    while j >= 3
        && toks[j - 1].tok == Tok::Punct(':')
        && toks[j - 2].tok == Tok::Punct(':')
        && let Some(p) = ident(toks, j - 3)
    {
        parts.push(p.to_string());
        j -= 3;
    }
    parts.reverse();
    parts.join("::")
}

/// The index of the parameter named `icon`, `glyph` or `icon_name`, not counting `self`.
fn icon_param(toks: &[T], params: &[std::ops::Range<usize>]) -> Option<usize> {
    let mut idx = 0;
    for p in params {
        let mut j = p.start;
        while j < p.end && (toks[j].tok == Tok::Punct('&') || ident(toks, j) == Some("mut")) {
            j += 1;
        }
        match ident(toks, j) {
            Some("self") => continue,
            Some(n) if ICON_PARAMS.contains(&n) => {
                let next = toks.get(j + 1).map(|t| &t.tok);
                if j + 1 >= p.end || next == Some(&Tok::Punct(':')) {
                    return Some(idx);
                }
            }
            _ => {}
        }
        idx += 1;
    }
    None
}

fn ident(toks: &[T], i: usize) -> Option<&str> {
    match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Ident(s)) => Some(s),
        _ => None,
    }
}

/// The first `c` at or after `from`, unless a `{` or `;` comes first.
fn find_punct(toks: &[T], from: usize, c: char) -> Option<usize> {
    for (j, t) in toks.iter().enumerate().skip(from) {
        match t.tok {
            Tok::Punct(p) if p == c => return Some(j),
            Tok::Punct('{' | ';') => return None,
            _ => {}
        }
    }
    None
}

/// The index of the bracket closing the one at `open`.
fn matching(toks: &[T], open: usize, o: char, c: char) -> usize {
    let mut d = 0;
    for (j, t) in toks.iter().enumerate().skip(open) {
        if t.tok == Tok::Punct(o) {
            d += 1;
        } else if t.tok == Tok::Punct(c) {
            d -= 1;
            if d == 0 {
                return j;
            }
        }
    }
    toks.len() - 1
}

/// The comma-separated items (as token ranges) inside the brackets that open at `open`.
fn split_args(toks: &[T], open: usize, o: char, c: char) -> Vec<std::ops::Range<usize>> {
    split_top(toks, open + 1, matching(toks, open, o, c))
}

/// The top-level comma-separated items (token ranges) of `toks[from..to]`.
fn split_top(toks: &[T], from: usize, to: usize) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = from;
    let mut d = 0i32;
    for (j, t) in toks.iter().enumerate().take(to).skip(from) {
        match t.tok {
            Tok::Punct('(' | '{' | '[') => d += 1,
            Tok::Punct(')' | '}' | ']') => d -= 1,
            // Generic angle brackets in a parameter type: `Option<(A, B)>` is bracketed, but
            // `HashMap<A, B>` isn't, so a type may add a spurious item after the icon one.
            Tok::Punct(',') if d == 0 => {
                out.push(start..j);
                start = j + 1;
            }
            _ => {}
        }
    }
    if start < to {
        out.push(start..to);
    }
    out
}

/// Drops `#[cfg(test)] mod name { .. }`.
fn strip_test_mods(toks: Vec<T>) -> Vec<T> {
    let pat = [
        Tok::Punct('#'),
        Tok::Punct('['),
        Tok::Ident("cfg".into()),
        Tok::Punct('('),
        Tok::Ident("test".into()),
        Tok::Punct(')'),
        Tok::Punct(']'),
    ];
    let mut out = Vec::with_capacity(toks.len());
    let mut i = 0;
    while i < toks.len() {
        let is_test_mod = toks.len() > i + pat.len() + 2
            && toks[i..i + pat.len()].iter().map(|t| &t.tok).eq(pat.iter())
            && ident(&toks, i + pat.len()) == Some("mod")
            && toks[i + pat.len() + 2].tok == Tok::Punct('{');
        if is_test_mod {
            i = matching(&toks, i + pat.len() + 2, '{', '}') + 1;
        } else {
            out.push(toks[i].clone());
            i += 1;
        }
    }
    out
}

/// Identifiers, string literals (cooked contents, escapes kept as written) and single
/// punctuation characters; comments, numbers and char literals are dropped, and a lifetime
/// is `'` followed by its name.
fn tokenize(src: &str) -> Vec<T> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        let ch = b[i];
        if ch == b'\n' {
            line += 1;
            i += 1;
        } else if ch.is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            let mut d = 0;
            while i < b.len() {
                if b[i..].starts_with(b"/*") {
                    d += 1;
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
            }
        } else if ch == b'"' || (ch == b'r' && is_raw_string(&b[i + 1..])) {
            let start_line = line;
            let raw = ch == b'r';
            let mut hashes = 0;
            if raw {
                i += 1;
                while b[i] == b'#' {
                    hashes += 1;
                    i += 1;
                }
            }
            i += 1; // opening quote
            let start = i;
            loop {
                if b[i] == b'\n' {
                    line += 1;
                }
                if !raw && b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' && b[i + 1..].iter().take(hashes).all(|&h| h == b'#') {
                    break;
                }
                i += 1;
            }
            out.push(T {
                tok: Tok::Str(src[start..i].to_string()),
                line: start_line,
            });
            i += 1 + hashes;
        } else if ch == b'\'' {
            // A char literal ('a', '\n', '\'', '…') or a lifetime ('static).
            let len = src[i + 1..].chars().next().map_or(1, char::len_utf8);
            if b.get(i + 1) == Some(&b'\\') {
                i += 2;
                while b[i] != b'\'' {
                    i += 1;
                }
                i += 1;
            } else if b.get(i + 1 + len) == Some(&b'\'') {
                i += 2 + len;
            } else {
                out.push(T {
                    tok: Tok::Punct('\''),
                    line,
                });
                i += 1;
            }
        } else if ch.is_ascii_alphabetic() || ch == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(T {
                tok: Tok::Ident(src[start..i].to_string()),
                line,
            });
        } else if ch.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                if b[i] == b'.' && !b.get(i + 1).is_some_and(u8::is_ascii_digit) {
                    break;
                }
                i += 1;
            }
        } else {
            // Multi-byte UTF-8 characters only occur in strings and comments, handled above.
            out.push(T {
                tok: Tok::Punct(ch as char),
                line,
            });
            i += 1;
        }
    }
    out
}

/// Whether the text after an `r` starts a raw string (`r"`, `r#"`), not a raw identifier.
fn is_raw_string(rest: &[u8]) -> bool {
    let hashes = rest.iter().take_while(|&&c| c == b'#').count();
    rest.get(hashes) == Some(&b'"')
}

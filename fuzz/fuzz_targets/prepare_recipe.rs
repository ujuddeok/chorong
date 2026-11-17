#![no_main]
//! `.chorong/prepare.json`: a repository's own recipe, shown to the reader for approval and
//! then run. An accepted recipe stays in the worktree, hashes stably, and its displayed
//! command reads back as exactly the argv that runs, with nothing (control characters,
//! bidirectional overrides) that could make the approval prompt show something else.
use libfuzzer_sys::fuzz_target;
use cho_sources::prepare::Recipe;
use std::path::{Component, Path};

/// Characters that change how the rest of a line displays.
fn deceptive(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
}

/// Splits a displayed command the way a reader (and a POSIX shell) would: words at
/// unquoted whitespace, `'…'` literal, `'\''` an escaped quote, `$'…'` with escapes.
fn split_display(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur: Option<String> = None;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let w = cur.get_or_insert_with(String::new);
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    w.push(q);
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                let w = cur.get_or_insert_with(String::new);
                while let Some(q) = chars.next() {
                    match q {
                        '\'' => break,
                        '\\' => match chars.next() {
                            Some(e @ ('x' | 'u')) => {
                                let n = if e == 'x' { 2 } else { 4 };
                                let hex: String = chars.by_ref().take(n).collect();
                                let v = u32::from_str_radix(&hex, 16).expect("hex escape");
                                w.push(char::from_u32(v).expect("a char"));
                            }
                            Some(e) => w.push(e),
                            None => {}
                        },
                        q => w.push(q),
                    }
                }
            }
            '\\' if chars.peek() == Some(&'\'') => {
                chars.next();
                cur.get_or_insert_with(String::new).push('\'');
            }
            c if c.is_whitespace() => out.extend(cur.take()),
            c => cur.get_or_insert_with(String::new).push(c),
        }
    }
    out.extend(cur);
    out
}

fuzz_target!(|s: &str| {
    let Ok(r) = Recipe::parse(s) else { return };
    for step in &r.steps {
        assert!(step.argv.first().is_some_and(|p| !p.trim().is_empty()));
        assert!(step.timeout_s > 0);
        let cwd = Path::new(&step.cwd);
        assert!(
            cwd.components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "cwd {:?} leaves the worktree",
            step.cwd
        );
        let shown = step.display();
        assert!(
            !shown.chars().any(deceptive),
            "display {shown:?} hides text"
        );
        let cmd = if step.cwd == "." {
            shown.as_str()
        } else {
            shown
                .strip_prefix(&format!("(in {}) ", step.cwd))
                .expect("display names the folder")
        };
        assert_eq!(
            split_display(cmd),
            step.argv,
            "display {shown:?} misreads the argv"
        );
    }
    let again = Recipe::parse(&serde_json::to_string(&r).unwrap()).expect("re-parses");
    assert_eq!(again, r);
    assert_eq!(again.hash(), r.hash());
});

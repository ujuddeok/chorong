#![no_main]
//! Parsers for `git diff -U0` and `git worktree list --porcelain` output.
//!
//! The first byte picks the mode. Structured: a diff is generated (paths with spaces, quotes,
//! ` b/`, tabs, unicode; added, deleted, renamed, binary and mode-only files; hunks whose
//! body lines look like file headers) and rendered the way git does it (names with a space end
//! in a tab; names with control characters, quotes, backslashes or non-ASCII bytes are C-quoted
//! under `core.quotePath`, or only the first three kinds without it), then parsed back: every
//! hunk is attributed to the right file with the right lines and deleted paths come back
//! exactly. Worktrees: a generated porcelain listing parses back to the same entries. Raw:
//! arbitrary bytes parse without panicking and hunks stay one-based and ordered.
use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use cho_git::{DiffHunk, Worktree, parse_diff, parse_worktrees};
use std::path::PathBuf;

const PARTS: [&str; 18] = [
    "a",
    "b",
    "src",
    " ",
    " b/",
    "a/",
    "\"",
    "\\",
    "\t",
    "\u{e9}",
    "\u{65e5}\u{672c}",
    "-- ",
    "+++ ",
    "..",
    "x y",
    "\u{1}",
    "\u{7f}",
    "\u{202e}",
];

fn path(u: &mut Unstructured<'_>) -> Result<String> {
    let mut out = String::new();
    for i in 0..u.int_in_range(1..=3)? {
        if i > 0 {
            out.push('/');
        }
        let before = out.len();
        for _ in 0..u.int_in_range(1..=3)? {
            match u.int_in_range(0..=9u8)? {
                0 => out.extend(
                    u.arbitrary::<String>()?
                        .chars()
                        .filter(|c| !matches!(c, '\n' | '\r' | '\0' | '/'))
                        .take(8),
                ),
                _ => out.push_str(PARTS[u.choose_index(PARTS.len())?]),
            }
        }
        if out.len() == before {
            out.push('a');
        }
    }
    Ok(out)
}

/// Text of one body line: never a newline; often something that looks like a header.
fn body_text(u: &mut Unstructured<'_>) -> Result<String> {
    const LOOKALIKES: [&str; 9] = [
        "-- x",
        "++ y",
        "- a/f",
        "+ b/f",
        "@@ -1 +1 @@",
        "diff --git a/x b/x",
        "-- a/other.rs",
        "++ b/other.rs",
        "-- /dev/null",
    ];
    Ok(match u.int_in_range(0..=3u8)? {
        0 => LOOKALIKES[u.choose_index(LOOKALIKES.len())?].to_string(),
        1 => u.arbitrary::<String>()?.replace(['\n'], ""),
        2 => String::new(),
        _ => "text".to_string(),
    })
}

/// A name as git prints it after the `a/` or `b/` prefix, with the tab for names with spaces.
fn name(prefix: &str, p: &str, quote_path: bool) -> String {
    let needs_quote = p
        .chars()
        .any(|c| c.is_ascii_control() || c == '"' || c == '\\' || (quote_path && !c.is_ascii()));
    let mut s = format!("{prefix}{p}");
    if needs_quote {
        let mut q = String::from("\"");
        for c in s.chars() {
            match c {
                '\u{7}' => q.push_str("\\a"),
                '\u{8}' => q.push_str("\\b"),
                '\t' => q.push_str("\\t"),
                '\n' => q.push_str("\\n"),
                '\u{b}' => q.push_str("\\v"),
                '\u{c}' => q.push_str("\\f"),
                '\r' => q.push_str("\\r"),
                '"' => q.push_str("\\\""),
                '\\' => q.push_str("\\\\"),
                c if c.is_ascii_control() => q.push_str(&format!("\\{:03o}", c as u8)),
                c if quote_path && !c.is_ascii() => {
                    for b in c.to_string().bytes() {
                        q.push_str(&format!("\\{b:03o}"));
                    }
                }
                c => q.push(c),
            }
        }
        q.push('"');
        s = q;
    }
    if s.contains(' ') {
        s.push('\t');
    }
    s
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Modify,
    Add,
    Delete,
    Rename,
    Binary,
    ModeOnly,
}

fn diff(u: &mut Unstructured<'_>) -> Result<()> {
    let quote_path = u.arbitrary()?;
    let mut text = String::new();
    let (mut want_hunks, mut want_removed) = (vec![], vec![]);
    for _ in 0..u.int_in_range(0..=5)? {
        let old = path(u)?;
        let kind = [
            Kind::Modify,
            Kind::Modify,
            Kind::Add,
            Kind::Delete,
            Kind::Rename,
            Kind::Binary,
            Kind::ModeOnly,
        ][u.choose_index(7)?];
        let new = if kind == Kind::Rename {
            path(u)?
        } else {
            old.clone()
        };
        text.push_str(&format!(
            "diff --git {} {}\n",
            name("a/", &old, quote_path).trim_end_matches('\t'),
            name("b/", &new, quote_path).trim_end_matches('\t')
        ));
        match kind {
            Kind::Add => text.push_str("new file mode 100644\n"),
            Kind::Delete => text.push_str("deleted file mode 100644\n"),
            Kind::ModeOnly => text.push_str("old mode 100644\nnew mode 100755\n"),
            Kind::Rename => text.push_str(&format!(
                "similarity index 90%\nrename from {}\nrename to {}\n",
                name("", &old, quote_path).trim_end_matches('\t'),
                name("", &new, quote_path).trim_end_matches('\t')
            )),
            _ => {}
        }
        if kind != Kind::ModeOnly {
            text.push_str("index 1234567..89abcde 100644\n");
        }
        if kind == Kind::Binary {
            text.push_str(&format!(
                "Binary files {} and {} differ\n",
                name("a/", &old, quote_path).trim_end_matches('\t'),
                name("b/", &new, quote_path).trim_end_matches('\t')
            ));
            continue;
        }
        let nhunks = u.int_in_range(0..=4)?;
        // A rename without changes, or a mode change, has no ---/+++ lines.
        if kind == Kind::ModeOnly || (kind == Kind::Rename && nhunks == 0) {
            continue;
        }
        let nhunks = nhunks.max(1);
        text.push_str(&match kind {
            Kind::Add => format!("--- /dev/null\n+++ {}\n", name("b/", &new, quote_path)),
            Kind::Delete => format!("--- {}\n+++ /dev/null\n", name("a/", &old, quote_path)),
            _ => format!(
                "--- {}\n+++ {}\n",
                name("a/", &old, quote_path),
                name("b/", &new, quote_path)
            ),
        });
        if kind == Kind::Delete {
            want_removed.push(old.clone());
        }
        let mut next = 1usize;
        for _ in 0..nhunks {
            let (oc, nc) = match kind {
                Kind::Add => (0, u.int_in_range(1..=4usize)?),
                Kind::Delete => (u.int_in_range(1..=4usize)?, 0),
                _ => (u.int_in_range(0..=4usize)?, u.int_in_range(0..=4usize)?),
            };
            if oc == 0 && nc == 0 {
                continue;
            }
            next += u.int_in_range(0..=50)?;
            let (os, ns) = match kind {
                Kind::Add => (0, 1),
                Kind::Delete => (1, 0),
                _ => (
                    if oc == 0 { next - 1 } else { next },
                    if nc == 0 { next - 1 } else { next },
                ),
            };
            let count = |n: usize| {
                if n == 1 {
                    String::new()
                } else {
                    format!(",{n}")
                }
            };
            let ctx = if u.arbitrary()? {
                format!(" {}", body_text(u)?)
            } else {
                String::new()
            };
            text.push_str(&format!(
                "@@ -{os}{} +{ns}{} @@{ctx}\n",
                count(oc),
                count(nc)
            ));
            let mut lines = vec![];
            for _ in 0..oc {
                lines.push(format!("-{}", body_text(u)?));
            }
            if u.ratio(1u8, 6)? && oc > 0 {
                lines.push("\\ No newline at end of file".to_string());
            }
            for _ in 0..nc {
                lines.push(format!("+{}", body_text(u)?));
            }
            if u.ratio(1u8, 6)? && nc > 0 {
                lines.push("\\ No newline at end of file".to_string());
            }
            for l in lines {
                text.push_str(&l);
                text.push('\n');
            }
            if kind != Kind::Delete {
                let (s, e) = if nc == 0 {
                    (ns.max(1), ns + 1)
                } else {
                    (ns, ns + nc - 1)
                };
                want_hunks.push((new.clone(), s, e));
            }
            next += nc.max(1);
        }
    }
    // `diff_hunks` reads git's output through `git()`, which trims trailing whitespace.
    let text = text.trim_end().to_string();
    let (hunks, removed) = parse_diff(&text);
    let got: Vec<(String, usize, usize)> = hunks
        .into_iter()
        .map(|h| (h.path, h.start_line, h.end_line))
        .collect();
    assert_eq!(got, want_hunks, "hunks of:\n{text}");
    assert_eq!(removed, want_removed, "removed paths of:\n{text}");
    Ok(())
}

fn worktrees(u: &mut Unstructured<'_>) -> Result<()> {
    let mut text = String::new();
    let mut want = vec![];
    for _ in 0..u.int_in_range(0..=4)? {
        let p = path(u)?;
        let head: String = (0..40)
            .map(|_| {
                "0123456789abcdef".as_bytes()[usize::from(u.int_in_range(0..=15u8).unwrap_or(0))]
                    as char
            })
            .collect();
        text.push_str(&format!("worktree /{p}\n"));
        let (mut h, mut b) = (None, None);
        match u.int_in_range(0..=2u8)? {
            0 => {
                text.push_str(&format!("HEAD {head}\n"));
                h = Some(head);
                let branch = path(u)?;
                text.push_str(&format!("branch refs/heads/{branch}\n"));
                b = Some(branch);
            }
            1 => {
                text.push_str(&format!("HEAD {head}\ndetached\n"));
                h = Some(head);
            }
            _ => text.push_str("bare\n"),
        }
        if u.arbitrary()? {
            text.push_str("locked a reason: HEAD x\n");
        }
        if u.arbitrary()? {
            text.push_str("prunable gitdir file points to non-existent location\n");
        }
        text.push('\n');
        want.push(Worktree {
            path: PathBuf::from(format!("/{p}")),
            head: h,
            branch: b,
        });
    }
    assert_eq!(parse_worktrees(&text), want, "worktrees of:\n{text}");
    Ok(())
}

fn raw(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    let (hunks, removed) = parse_diff(&text);
    for DiffHunk {
        start_line,
        end_line,
        path,
    } in &hunks
    {
        assert!(start_line <= end_line, "{path}: {start_line}-{end_line}");
        assert!(!path.is_empty());
    }
    assert!(removed.iter().all(|r| !r.is_empty()));
    let _ = parse_worktrees(&text);
}

fuzz_target!(|data: &[u8]| {
    let Some((mode, rest)) = data.split_first() else {
        return;
    };
    let mut u = Unstructured::new(rest);
    let _ = match mode % 3 {
        0 => diff(&mut u),
        1 => worktrees(&mut u),
        _ => {
            raw(rest);
            Ok(())
        }
    };
});

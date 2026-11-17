#![no_main]
//! Repository text search over fuzzed files on disk.
//!
//! Files are built from fragments (CRLF, tabs, multibyte text, NULs, invalid UTF-8, long
//! lines) or raw bytes. The pattern is fuzzed text or a slice of a file (optionally case
//! flipped), literal or regex, either case option. Checks: every range is inside its line on
//! char boundaries; the line number and byte offset agree with an independent scan of the
//! file bytes; the text is the line without its terminator; literal matches equal the needle
//! (up to case when insensitive); for clean files every line containing a case-sensitive
//! literal is found; hits are in path then line order; the cap returns a prefix; a second run
//! gives the same answer.
use libfuzzer_sys::arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use cho_search::{CancelToken, Hit, SearchOptions, search_all};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const FILES: [&str; 3] = ["a.txt", "dir/b.rs", "dir/sub/c.md"];

const FRAGMENTS: [&str; 14] = [
    "\n",
    "\r\n",
    "\r",
    "\t",
    "abc",
    "ABC",
    "Abc",
    "h\u{e9}llo",
    "\u{65e5}\u{672c}",
    "\0",
    "\u{feff}",
    "K\u{212a}",
    "fn main() {}",
    "  ",
];

fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        // Memory-backed when the system has one: the files are rewritten for every input.
        let base = PathBuf::from("/dev/shm");
        let base = if base.is_dir() {
            base
        } else {
            std::env::temp_dir()
        };
        let r = base.join(format!("cho-fuzz-search-{}", std::process::id()));
        std::fs::create_dir_all(r.join("dir/sub")).unwrap();
        r
    })
}

fn content(u: &mut Unstructured<'_>) -> Result<Vec<u8>> {
    if u.ratio(1u8, 6)? {
        return Ok(u.arbitrary::<Vec<u8>>()?);
    }
    let mut out = vec![];
    for _ in 0..u.int_in_range(0..=40)? {
        match u.int_in_range(0..=9u8)? {
            0 => out.push(0xff),
            1 => {
                // A long line.
                let n = u.int_in_range(1..=3000usize)?;
                out.extend(std::iter::repeat_n(b'a', n));
                out.extend(b"needle");
            }
            2 => out.extend(u.arbitrary::<String>()?.bytes()),
            _ => out.extend(FRAGMENTS[u.choose_index(FRAGMENTS.len())?].bytes()),
        }
    }
    Ok(out)
}

fn flip_case(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_lowercase() {
                c.to_uppercase().next().unwrap_or(c)
            } else {
                c.to_lowercase().next().unwrap_or(c)
            }
        })
        .collect()
}

fn lower(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// The lines of `bytes` with their byte offsets, split on `\n` like ripgrep.
fn lines(bytes: &[u8]) -> Vec<(usize, &[u8])> {
    let mut out = vec![];
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |p| start + p + 1);
        out.push((start, &bytes[start..end]));
        start = end;
    }
    out
}

fn trim(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &line[..end]
}

fn check_hit(h: &Hit, files: &[(String, Vec<u8>)], opts: &SearchOptions) {
    let (_, bytes) = files
        .iter()
        .find(|(p, _)| *p == h.path)
        .unwrap_or_else(|| panic!("hit in unknown file {:?}", h.path));
    for r in &h.ranges {
        assert!(
            r.start <= r.end && r.end <= h.text.len(),
            "range {r:?} in {:?}",
            h.text
        );
        assert!(
            h.text.is_char_boundary(r.start) && h.text.is_char_boundary(r.end),
            "range {r:?} splits a char of {:?}",
            h.text
        );
    }
    assert!(!h.ranges.is_empty());
    // Independent line count.
    let all = lines(bytes);
    let (start, raw) = all
        .get(h.line.wrapping_sub(1))
        .unwrap_or_else(|| panic!("line {} beyond {} lines of {}", h.line, all.len(), h.path));
    assert_eq!(
        h.line_start, *start,
        "line_start of line {} in {}",
        h.line, h.path
    );
    assert!(
        trim(raw) == h.text.as_bytes(),
        "text of line {} in {}",
        h.line,
        h.path
    );
    assert!(
        h.line_start + h.text.len() <= bytes.len(),
        "hit past the end of {}",
        h.path
    );
    if !opts.regex {
        for r in &h.ranges {
            let m = &h.text[r.clone()];
            if opts.case_sensitive {
                assert_eq!(m, opts.pattern, "literal match differs");
            } else {
                assert_eq!(
                    lower(m),
                    lower(&opts.pattern),
                    "literal match differs: {m:?}"
                );
            }
        }
    }
}

/// `None` for a pattern the engine rejects (bad regex, or a literal with a line break).
fn run(opts: &SearchOptions) -> Option<(Vec<Hit>, bool)> {
    let Ok((hits, summary)) = search_all(root(), opts, &CancelToken::new()) else {
        assert!(
            opts.regex || opts.pattern.contains(['\n', '\r']),
            "literal {:?} rejected",
            opts.pattern
        );
        return None;
    };
    assert_eq!(summary.hits, hits.len());
    assert!(!summary.cancelled);
    Some((hits, summary.truncated))
}

fn fuzz(u: &mut Unstructured<'_>) -> Result<()> {
    let mut files = vec![];
    for f in FILES {
        if u.ratio(1u8, 5)? {
            let _ = std::fs::remove_file(root().join(f));
            continue;
        }
        let c = content(u)?;
        std::fs::write(root().join(f), &c).unwrap();
        files.push((f.to_string(), c));
    }
    let regex = u.arbitrary()?;
    let case_sensitive = u.arbitrary()?;
    let pattern = match u.int_in_range(0..=3u8)? {
        0 => u.arbitrary::<String>()?,
        _ => {
            // A slice of a file that is valid text, so literals have something to find.
            let text = files
                .get(u.choose_index(files.len().max(1))?)
                .map(|(_, c)| String::from_utf8_lossy(c).into_owned())
                .unwrap_or_default();
            let chars: Vec<char> = text.chars().collect();
            let a = u.int_in_range(0..=chars.len())?;
            let b = u.int_in_range(a..=chars.len().min(a + 12))?;
            let s: String = chars[a..b].iter().collect();
            if u.arbitrary()? { flip_case(&s) } else { s }
        }
    };
    let opts = SearchOptions {
        pattern,
        regex,
        case_sensitive,
        globs: vec![],
        max_results: usize::MAX >> 1,
    };
    let Some((hits, truncated)) = run(&opts) else {
        return Ok(());
    };
    assert!(!truncated);
    for h in &hits {
        check_hit(h, &files, &opts);
    }
    // Order: by path, then line.
    let key = |h: &Hit| (h.path.clone(), h.line);
    assert!(
        hits.windows(2).all(|w| key(&w[0]) < key(&w[1])),
        "hits out of order"
    );
    // Deterministic.
    assert_eq!(Some((hits.clone(), false)), run(&opts));

    // The cap returns a prefix.
    let cap = u.int_in_range(1..=3usize)?;
    let (capped, truncated) = run(&SearchOptions {
        max_results: cap,
        ..opts.clone()
    })
    .expect("same pattern");
    assert_eq!(truncated, hits.len() > cap);
    assert_eq!(capped[..], hits[..hits.len().min(cap)]);

    // Completeness for clean files and a case-sensitive literal.
    if !regex && case_sensitive && !opts.pattern.is_empty() && !opts.pattern.contains(['\n', '\r'])
    {
        for (path, bytes) in &files {
            let Ok(text) = std::str::from_utf8(bytes) else {
                continue;
            };
            if bytes.contains(&0) || text.starts_with('\u{feff}') {
                continue;
            }
            // BOM sniffing is only at the start; leave files with one out above.
            let expected: Vec<usize> = lines(bytes)
                .iter()
                .enumerate()
                .filter(|(_, (_, l))| {
                    std::str::from_utf8(trim(l)).is_ok_and(|t| t.contains(&opts.pattern))
                })
                .map(|(i, _)| i + 1)
                .collect();
            let got: Vec<usize> = hits
                .iter()
                .filter(|h| h.path == *path)
                .map(|h| h.line)
                .collect();
            assert_eq!(
                got, expected,
                "lines found in {path} for {:?}",
                opts.pattern
            );
        }
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = fuzz(&mut Unstructured::new(data));
});

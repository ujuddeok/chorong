//! `git blame` for one file: which commit last changed each line, as git reports it.
//!
//! [`blame`] runs `git blame --porcelain --root --contents -` on the exact bytes a pane shows,
//! starting from a resolved commit, so the answer always describes that text: lines that are
//! not in the commit's history come back as not committed rather than as stale data.
//! [`parse_porcelain`] is the pure part, tested on its own. Nothing here guesses: every
//! attribution is git's.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

/// One commit that some line of the file is attributed to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlameCommit {
    /// The full commit id (all zeros for lines that are not committed).
    pub id: String,
    pub author: String,
    /// The address between `<` and `>`; may be empty.
    pub author_mail: String,
    /// Seconds since the Unix epoch.
    pub author_time: i64,
    /// As git writes it: `+0100`, `-0730`.
    pub author_tz: String,
    pub summary: String,
    /// git stopped here: history before this commit is not available (a shallow clone).
    /// With `--root`, a repository's first commit is not a boundary.
    pub boundary: bool,
}

impl BlameCommit {
    /// Lines that differ from the commit blamed from: git's all-zero id.
    pub fn uncommitted(&self) -> bool {
        !self.id.is_empty() && self.id.bytes().all(|b| b == b'0')
    }

    /// The first seven characters of the id.
    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(7)]
    }
}

/// The blame of one file: its commits, and for each line (zero-based) the index of its commit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Blame {
    /// In the order git first mentions them.
    pub commits: Vec<BlameCommit>,
    /// `lines[i]` indexes `commits` for line `i`.
    pub lines: Vec<u32>,
}

impl Blame {
    /// The index into `commits` of `line`'s commit, if git blamed that line.
    pub fn commit_index(&self, line: usize) -> Option<usize> {
        self.lines.get(line).map(|&c| c as usize)
    }

    pub fn commit_of(&self, line: usize) -> Option<&BlameCommit> {
        self.commits.get(self.commit_index(line)?)
    }
}

/// Parses `git blame --porcelain` output. Every line of the file must be accounted for once.
pub fn parse_porcelain(out: &[u8]) -> Result<Blame, String> {
    let mut commits: Vec<BlameCommit> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut lines: Vec<Option<u32>> = Vec::new();
    // The commit of the group being read and the final line its next content line goes to.
    let mut current: Option<(usize, usize)> = None;
    let mut raw = out.split(|&b| b == b'\n').enumerate().peekable();
    while let Some((n, line)) = raw.next() {
        if line.is_empty() && raw.peek().is_none() {
            break;
        }
        let bad = |why: &str| Err(format!("git blame output line {}: {why}", n + 1));
        if let Some(_content) = line.strip_prefix(b"\t") {
            let Some((commit, final_line)) = current.take() else {
                return bad("a line of the file before its header");
            };
            if lines.len() <= final_line {
                lines.resize(final_line + 1, None);
            }
            if lines[final_line].replace(commit as u32).is_some() {
                return bad("the same line blamed twice");
            }
            continue;
        }
        let text = String::from_utf8_lossy(line);
        if current.is_none() {
            // A header: `<id> <original line> <final line> [<lines in group>]`.
            let mut parts = text.split(' ');
            let (Some(id), Some(_orig), Some(fin)) = (parts.next(), parts.next(), parts.next())
            else {
                return bad("expected a header");
            };
            if !(id.len() == 40 || id.len() == 64) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return bad("expected a commit id");
            }
            let fin: usize = match fin.parse() {
                Ok(f) if f >= 1 => f,
                _ => return bad("expected a line number"),
            };
            let ix = *by_id.entry(id.to_string()).or_insert_with(|| {
                commits.push(BlameCommit {
                    id: id.to_string(),
                    ..Default::default()
                });
                commits.len() - 1
            });
            current = Some((ix, fin - 1));
            continue;
        }
        // A field of the current commit: `key value`, or a bare key.
        let ix = current.map_or(0, |(c, _)| c);
        let (key, value) = text.split_once(' ').unwrap_or((&text, ""));
        let c = &mut commits[ix];
        match key {
            "author" => c.author = value.to_string(),
            "author-mail" => {
                let v = value.strip_prefix('<').unwrap_or(value);
                c.author_mail = v.strip_suffix('>').unwrap_or(v).to_string();
            }
            "author-time" => c.author_time = value.trim().parse().unwrap_or(0),
            "author-tz" => c.author_tz = value.to_string(),
            "summary" => c.summary = value.to_string(),
            "boundary" => c.boundary = true,
            // committer-*, previous, filename and fields a newer git adds.
            _ => {}
        }
    }
    if current.is_some() {
        return Err("git blame output ends inside a group".into());
    }
    let lines = lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| l.ok_or_else(|| format!("git blame output skips line {}", i + 1)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Blame { commits, lines })
}

/// The full id of the commit `rev` names (`None`: `HEAD`), in the repository around `dir`.
/// `--end-of-options` keeps a `rev` from a link from being read as an option.
pub fn resolve_commit(dir: &Path, rev: Option<&str>) -> Result<String, String> {
    let rev = format!("{}^{{commit}}", rev.unwrap_or("HEAD"));
    let out = cho_core::git::command(dir)
        .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
        .arg(&rev)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && is_commit_id(&id) {
        Ok(id)
    } else if cho_core::git::command(dir)
        .args(["rev-parse", "--git-dir"])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        Err(format!(
            "{} is not a commit here",
            rev.trim_end_matches("^{commit}")
        ))
    } else {
        Err("not in a git repository".into())
    }
}

fn is_commit_id(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Blames `contents` as the version of `file` (a name inside `dir`) that follows `commit` (a
/// full id from [`resolve_commit`]). Lines of `contents` that are not in `commit`'s version
/// come back as not committed.
pub fn blame(dir: &Path, commit: &str, file: &Path, contents: &[u8]) -> Result<Blame, String> {
    if !is_commit_id(commit) {
        return Err(format!("`{commit}` is not a full commit id"));
    }
    let mut child = cho_core::git::command(dir)
        .args([
            "blame",
            "--porcelain",
            "--root",
            "--contents",
            "-",
            commit,
            "--",
        ])
        .arg(file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    // Written from another thread so a full stdout pipe cannot stall the write.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let out = std::thread::scope(|s| {
        s.spawn(move || {
            // git may exit early (no such path); its stderr says why.
            let _ = stdin.write_all(contents);
        });
        child.wait_with_output()
    })
    .map_err(|e| format!("git blame failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err
            .lines()
            .next()
            .unwrap_or("git blame failed")
            .trim_start_matches("fatal: ");
        return Err(first.to_string());
    }
    parse_porcelain(&out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "f61e02151b03d0b9b85375e5da52eba5891d7d69";
    const B: &str = "7910a925e794cd331dedc3554557502bb9bbb80a";
    const Z: &str = "0000000000000000000000000000000000000000";

    fn header(id: &str, author: &str, mail: &str, time: i64, summary: &str) -> String {
        format!(
            "author {author}\nauthor-mail <{mail}>\nauthor-time {time}\nauthor-tz +0100\n\
             committer c\ncommitter-mail <c@x>\ncommitter-time {time}\ncommitter-tz +0000\n\
             summary {summary}\nprevious {id} f.txt\nfilename f.txt\n"
        )
    }

    #[test]
    fn parses_groups_repeated_commits_and_uncommitted_lines() {
        // What git 2.43 prints for `--contents -` with one line edited and one appended.
        let out = format!(
            "{A} 1 1 1\n{}\ta\n\
             {Z} 2 2 1\nauthor External file (--contents)\nauthor-mail <external.file>\n\
             author-time 1793748212\nauthor-tz +0000\nsummary Version of f.txt from standard input\n\
             previous {B} f.txt\nfilename f.txt\n\tX\n\
             {A} 2 3 1\n\tb\n\
             {B} 3 4 1\n{}\tc\n\
             {Z} 5 5 1\n\td\n",
            header(A, "José \"Pepe\" Núñez", "j@x", 1_700_000_000, "first"),
            header(B, "李小龍", "", 1_750_000_000, "second: with summary"),
        );
        let b = parse_porcelain(out.as_bytes()).unwrap();
        assert_eq!(b.lines, [0, 1, 0, 2, 1]);
        assert_eq!(b.commits.len(), 3);
        let a = &b.commits[0];
        assert_eq!(a.author, "José \"Pepe\" Núñez");
        assert_eq!(a.author_mail, "j@x");
        assert_eq!(a.author_time, 1_700_000_000);
        assert_eq!(a.author_tz, "+0100");
        assert_eq!(a.summary, "first");
        assert_eq!(a.short_id(), "f61e021");
        assert!(!a.uncommitted() && !a.boundary);
        assert!(b.commits[1].uncommitted());
        assert_eq!(b.commit_of(4).unwrap().id, Z);
        let cjk = &b.commits[2];
        assert_eq!(cjk.author, "李小龍");
        assert_eq!(cjk.author_mail, "");
        assert_eq!(cjk.summary, "second: with summary");
        assert_eq!(b.commit_of(5), None);
    }

    #[test]
    fn a_group_header_covers_lines_listed_one_by_one() {
        // `<id> 1 1 2` announces two lines; each still has its own short header.
        let out = format!(
            "{A} 1 1 2\n{}boundary\nfilename f.txt\n\ta\n{A} 2 2\n\tb\n",
            "author t\nauthor-mail <t@x>\nauthor-time 5\nauthor-tz -0730\nsummary s\n"
        );
        let b = parse_porcelain(out.as_bytes()).unwrap();
        assert_eq!(b.lines, [0, 0]);
        assert!(b.commits[0].boundary, "a shallow clone's boundary is kept");
        assert_eq!(b.commits[0].author_tz, "-0730");
    }

    #[test]
    fn odd_author_names_and_bytes_survive() {
        let out = [
            format!("{A} 1 1 1\n").into_bytes(),
            b"author \nauthor-mail <>\nauthor-time x\nsummary \xff bad utf8\n\t\xfe\n".to_vec(),
            format!("{B} 1 2 1\nauthor  two  spaces \nauthor-mail <a b@c>\nsummary\n\t\n")
                .into_bytes(),
        ]
        .concat();
        let b = parse_porcelain(&out).unwrap();
        assert_eq!(b.commits[0].author, "");
        assert_eq!(b.commits[0].author_time, 0, "an unreadable time is 0");
        assert_eq!(b.commits[0].summary, "\u{fffd} bad utf8");
        assert_eq!(b.commits[1].author, " two  spaces ");
        assert_eq!(b.commits[1].author_mail, "a b@c");
        assert_eq!(b.commits[1].summary, "");
        assert_eq!(b.lines, [0, 1]);
    }

    #[test]
    fn empty_output_is_an_empty_file() {
        assert_eq!(parse_porcelain(b"").unwrap(), Blame::default());
    }

    #[test]
    fn malformed_output_is_refused() {
        for bad in [
            "not a header\n".to_string(),
            "\ta line with no header\n".to_string(),
            format!("{A} 1 0 1\n\tzero is not a line\n"),
            format!("{A} 1 1 1\nauthor x\n"),
            format!("{A} 1 2 1\n\tline one is missing\n"),
            format!("{A} 1 1 1\n\ta\n{B} 1 1 1\n\ta again\n"),
        ] {
            assert!(parse_porcelain(bad.as_bytes()).is_err(), "{bad:?}");
        }
    }
}

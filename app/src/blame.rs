//! Blame beside the gutter: what each line's label says, how strongly it is shaded, the hover
//! text, and the per-window cache of answers. Pure, apart from [`compute`], which runs git (in
//! the background, through `cho_git::blame`).
//!
//! The label is the author's initials and the commit's age (`JD 3w`), shown on the first line
//! of each run of lines from one commit. Lines that differ from the commit blamed from say
//! `new`. Shading is by age rank within the file, newest strongest. A cached answer is
//! keyed by the folder, file, resolved commit and a fingerprint of the shown text, so it is
//! never reused for other text or another commit.

use crate::features::Probe;
use cho_git::blame::{self, Blame, BlameCommit};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Width of the blame column in character cells: a first name of up to [`NAME_CELLS`] and an
/// age of up to four (`Claude 11mo`), or `new`.
pub const COLUMN_CELLS: f32 = 11.;
/// Room for the author's name in the column; longer names end in `…`.
const NAME_CELLS: usize = 6;

/// What the column shows for an uncommitted line.
pub const UNCOMMITTED: &str = "new";

/// A file's blame, ready to draw.
#[derive(Debug)]
pub struct BlameView {
    pub blame: Blame,
    /// Per commit: 0.0 for the oldest in this file, 1.0 for the newest (and uncommitted).
    pub heat: Vec<f32>,
    /// The commit blamed from (short id), for the hover text of uncommitted lines.
    pub from: String,
    /// Blamed from the checkout's `HEAD` (not another revision).
    pub from_head: bool,
}

impl BlameView {
    pub fn new(blame: Blame, from: &str, from_head: bool) -> Self {
        let heat = heat(&blame.commits);
        Self {
            blame,
            heat,
            from: cho_git::short(from),
            from_head,
        }
    }

    /// Whether `line` starts a run: the line above it on screen (`above`, `None` at the top
    /// or under a fold) is from another commit.
    pub fn starts_run(&self, above: Option<usize>, line: usize) -> bool {
        let this = self.blame.commit_index(line);
        this.is_some() && above.and_then(|a| self.blame.commit_index(a)) != this
    }

    /// The label for `line` (initials and age), or `None` past the blamed lines.
    pub fn label(&self, line: usize, now: i64) -> Option<(String, String)> {
        let c = self.blame.commit_of(line)?;
        Some(if c.uncommitted() {
            (UNCOMMITTED.to_string(), String::new())
        } else {
            (short_name(&c.author), age(now - c.author_time))
        })
    }

    pub fn heat_of(&self, line: usize) -> Option<f32> {
        self.heat.get(self.blame.commit_index(line)?).copied()
    }

    /// The hover text for `line`'s commit.
    pub fn details(&self, line: usize, now: i64) -> Option<String> {
        let c = self.blame.commit_of(line)?;
        if c.uncommitted() {
            let from = if self.from_head {
                format!("HEAD ({})", self.from)
            } else {
                self.from.clone()
            };
            return Some(format!("Not committed yet\nThis line differs from {from}."));
        }
        let mail = if c.author_mail.is_empty() {
            String::new()
        } else {
            format!(" <{}>", c.author_mail)
        };
        let mut text = format!(
            "{}  {}\n{}{mail}\n{} ({})",
            c.short_id(),
            c.summary,
            c.author,
            date(c.author_time, &c.author_tz),
            ago(now - c.author_time),
        );
        if c.boundary {
            text.push_str(
                "\nHistory before this commit is not in this clone: the line may be older.",
            );
        }
        Some(text)
    }
}

/// Age rank of each commit, newest 1.0. Uncommitted lines count as newest.
fn heat(commits: &[BlameCommit]) -> Vec<f32> {
    let mut times: Vec<i64> = commits
        .iter()
        .filter(|c| !c.uncommitted())
        .map(|c| c.author_time)
        .collect();
    times.sort_unstable();
    times.dedup();
    let top = times.len().saturating_sub(1).max(1) as f32;
    commits
        .iter()
        .map(|c| {
            if c.uncommitted() || times.len() <= 1 {
                1.0
            } else {
                times.binary_search(&c.author_time).unwrap_or(0) as f32 / top
            }
        })
        .collect()
}

/// The author as the column shows them: their first name, cut to [`NAME_CELLS`] with `…`
/// (`Claude`, `Barth…`), or their initials when the name is not plain ASCII (wide scripts take
/// two cells a character).
pub fn short_name(name: &str) -> String {
    let Some(first) = name
        .split_whitespace()
        .find(|w| w.chars().any(char::is_alphanumeric))
    else {
        return "?".into();
    };
    if !first.is_ascii() {
        return initials(name);
    }
    if first.len() <= NAME_CELLS {
        first.to_string()
    } else {
        format!("{}…", &first[..NAME_CELLS - 1])
    }
}

/// Up to two initials: the first letter or digit of the first and last words, or of a single
/// word its first two (one, if it is not plain ASCII, as wide scripts take two cells).
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .collect();
    let first = |w: &str| w.chars().find(|c| c.is_alphanumeric());
    let out: String = match words.as_slice() {
        [] => return "?".into(),
        [one] => {
            let mut cs = one.chars().filter(|c| c.is_alphanumeric());
            match (cs.next(), cs.next()) {
                (Some(a), Some(b)) if a.is_ascii() && b.is_ascii() => {
                    return format!("{}{}", a.to_ascii_uppercase(), b.to_ascii_lowercase());
                }
                (Some(a), _) => a.to_string(),
                _ => return "?".into(),
            }
        }
        [a, .., b] => first(a).into_iter().chain(first(b)).collect(),
    };
    out.to_uppercase()
}

/// The largest whole unit of `secs`, as (count, short unit, long unit).
fn unit(secs: i64) -> Option<(i64, &'static str, &'static str)> {
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    Some(match secs {
        s if s < MIN => return None,
        s if s < HOUR => (s / MIN, "m", "minute"),
        s if s < DAY => (s / HOUR, "h", "hour"),
        s if s < 7 * DAY => (s / DAY, "d", "day"),
        s if s < 30 * DAY => (s / (7 * DAY), "w", "week"),
        s if s < 365 * DAY => (s / (30 * DAY), "mo", "month"),
        s => (s / (365 * DAY), "y", "year"),
    })
}

/// A short age: `now`, `5m`, `3h`, `4d`, `3w`, `5mo`, `2y`. A time in the future (clock skew)
/// reads as `now`.
pub fn age(secs: i64) -> String {
    match unit(secs) {
        None => "now".into(),
        Some((n, short, _)) => format!("{n}{short}"),
    }
}

/// A spelled-out age: `just now`, `1 week ago`, `2 years ago`.
pub fn ago(secs: i64) -> String {
    match unit(secs) {
        None => "just now".into(),
        Some((1, _, long)) => format!("1 {long} ago"),
        Some((n, _, long)) => format!("{n} {long}s ago"),
    }
}

/// `2024-05-01 14:03 +0100`: the time in the author's own zone, as git records it.
pub fn date(time: i64, tz: &str) -> String {
    let offset = parse_tz(tz).unwrap_or(0);
    let local = time + offset;
    let (days, secs) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let tz = if parse_tz(tz).is_some() { tz } else { "+0000" };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} {tz}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// `+0100` → 3600 seconds.
fn parse_tz(tz: &str) -> Option<i64> {
    let (sign, digits) = match tz.as_bytes().first()? {
        b'+' => (1, &tz[1..]),
        b'-' => (-1, &tz[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i64, i64) = (digits[..2].parse().ok()?, digits[2..].parse().ok()?);
    Some(sign * (h * 3600 + m * 60))
}

/// Days since 1970-01-01 to a (year, month, day) in the proleptic Gregorian calendar
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// What a cached answer is for: the same folder, file, commit and text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub dir: PathBuf,
    pub file: PathBuf,
    pub commit: String,
    pub text: u64,
}

/// A fingerprint of the shown text (stable within the process).
pub fn fingerprint(text: &[u8]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The answers of one window, the most recently used last, at most `CAP`.
#[derive(Default)]
pub struct Cache {
    entries: VecDeque<(Key, Arc<BlameView>)>,
}

impl Cache {
    pub const CAP: usize = 32;

    pub fn get(&mut self, key: &Key) -> Option<Arc<BlameView>> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(i)?;
        let view = entry.1.clone();
        self.entries.push_back(entry);
        Some(view)
    }

    pub fn insert(&mut self, key: Key, view: Arc<BlameView>) {
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push_back((key, view));
        while self.entries.len() > Self::CAP {
            self.entries.pop_front();
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The blame of `text`, the version of `path` the pane shows, from `at_rev` (`None`: the
/// checkout's `HEAD`). Runs git; call it off the UI thread. Each run that is not answered from
/// `cache` is timed under `probe` (`blame.enabled`).
pub fn compute(
    cache: &Mutex<Cache>,
    path: &Path,
    at_rev: Option<&str>,
    text: &[u8],
    probe: Option<&Probe>,
) -> Result<Arc<BlameView>, String> {
    let (Some(dir), Some(file)) = (path.parent(), path.file_name()) else {
        return Err("not a file in a folder".into());
    };
    let commit = blame::resolve_commit(dir, at_rev)?;
    let key = Key {
        dir: dir.to_path_buf(),
        file: PathBuf::from(file),
        commit,
        text: fingerprint(text),
    };
    if let Some(hit) = cache.lock().ok().and_then(|mut c| c.get(&key)) {
        return Ok(hit);
    }
    let _span = probe.map(|p| p.background());
    let blamed = blame::blame(dir, &key.commit, Path::new(file), text)?;
    let view = Arc::new(BlameView::new(blamed, &key.commit, at_rev.is_none()));
    if let Ok(mut c) = cache.lock() {
        c.insert(key, view.clone());
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    fn commit(id: char, author: &str, time: i64) -> BlameCommit {
        BlameCommit {
            id: std::iter::repeat_n(id, 40).collect(),
            author: author.into(),
            author_mail: "a@x".into(),
            author_time: time,
            author_tz: "+0100".into(),
            summary: "Fix it".into(),
            boundary: false,
        }
    }

    fn view() -> BlameView {
        BlameView::new(
            Blame {
                commits: vec![
                    commit('a', "Ada Lovelace", 1_000 * DAY),
                    commit('0', "External file (--contents)", 2_000 * DAY),
                    commit('b', "linus", 1_500 * DAY),
                    BlameCommit {
                        boundary: true,
                        ..commit('c', "Grace Hopper", 500 * DAY)
                    },
                ],
                lines: vec![0, 0, 1, 2, 3],
            },
            "1234567890abcdef1234567890abcdef12345678",
            true,
        )
    }

    #[test]
    fn initials_from_ordinary_and_unusual_names() {
        for (name, want) in [
            ("Ada Lovelace", "AL"),
            ("Ada King Lovelace", "AL"),
            ("José \"Pepe\" Núñez", "JN"),
            ("linus", "Li"),
            ("dependabot[bot]", "De"),
            ("李小龍", "李"),
            ("Émile", "É"),
            ("  ", "?"),
            ("", "?"),
            ("-- ..", "?"),
            ("o'brien smith-jones", "OS"),
            ("x", "X"),
        ] {
            assert_eq!(initials(name), want, "{name:?}");
        }
    }

    #[test]
    fn the_column_shows_a_first_name_that_fits() {
        for (name, want) in [
            ("Claude", "Claude"),
            ("Ada Lovelace", "Ada"),
            ("Bartholomew Ng", "Barth…"),
            ("  ", "?"),
            ("李小龙", "李"),
        ] {
            assert_eq!(short_name(name), want, "{name:?}");
        }
    }

    #[test]
    fn ages_pick_the_largest_whole_unit() {
        for (secs, short, long) in [
            (-50, "now", "just now"),
            (59, "now", "just now"),
            (60, "1m", "1 minute ago"),
            (3 * 3600, "3h", "3 hours ago"),
            (DAY, "1d", "1 day ago"),
            (21 * DAY, "3w", "3 weeks ago"),
            (29 * DAY, "4w", "4 weeks ago"),
            (45 * DAY, "1mo", "1 month ago"),
            (364 * DAY, "12mo", "12 months ago"),
            (800 * DAY, "2y", "2 years ago"),
        ] {
            assert_eq!(age(secs), short, "{secs}");
            assert_eq!(ago(secs), long, "{secs}");
        }
    }

    #[test]
    fn dates_are_in_the_authors_zone() {
        assert_eq!(date(0, "+0000"), "1970-01-01 00:00 +0000");
        assert_eq!(date(1_714_572_180, "+0100"), "2024-05-01 15:03 +0100");
        assert_eq!(date(1_714_572_180, "-0730"), "2024-05-01 06:33 -0730");
        assert_eq!(date(951_782_400, "+0000"), "2000-02-29 00:00 +0000");
        assert_eq!(date(-86_400, "+0000"), "1969-12-31 00:00 +0000");
        assert_eq!(date(0, "bogus"), "1970-01-01 00:00 +0000");
    }

    #[test]
    fn labels_runs_and_shading() {
        let v = view();
        let now = 1_021 * DAY;
        assert_eq!(v.label(0, now), Some(("Ada".into(), "3w".into())));
        assert_eq!(v.label(2, now), Some((UNCOMMITTED.into(), String::new())));
        assert_eq!(v.label(9, now), None);
        // The label shows once per run of lines from one commit.
        assert!(v.starts_run(None, 0));
        assert!(!v.starts_run(Some(0), 1));
        assert!(v.starts_run(Some(1), 2));
        assert!(
            v.starts_run(None, 1),
            "the first line under a fold shows it again"
        );
        assert!(!v.starts_run(None, 9));
        // Newest strongest: uncommitted, then b, a, the boundary c.
        let heat: Vec<f32> = (0..5).map(|l| v.heat_of(l).unwrap()).collect();
        assert_eq!(heat, [0.5, 0.5, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn hover_text_names_the_commit_or_says_it_is_not_committed() {
        let v = view();
        let now = 1_021 * DAY;
        let d = v.details(0, now).unwrap();
        assert!(
            d.starts_with("aaaaaaa  Fix it\nAda Lovelace <a@x>\n"),
            "{d}"
        );
        assert!(d.ends_with("(3 weeks ago)"), "{d}");
        assert_eq!(
            v.details(2, now).unwrap(),
            "Not committed yet\nThis line differs from HEAD (1234567)."
        );
        assert!(v.details(4, now).unwrap().contains("not in this clone"));
    }

    #[test]
    fn the_cache_keeps_the_most_recent_answers() {
        let key = |n: u64| Key {
            dir: "/r".into(),
            file: "a.rs".into(),
            commit: "c".into(),
            text: n,
        };
        let mut cache = Cache::default();
        let v = Arc::new(view());
        for n in 0..Cache::CAP as u64 {
            cache.insert(key(n), v.clone());
        }
        assert!(cache.get(&key(0)).is_some(), "used: now the most recent");
        cache.insert(key(100), v.clone());
        assert_eq!(cache.len(), Cache::CAP);
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(1)).is_none(), "the least recently used went");
        assert_ne!(fingerprint(b"a"), fingerprint(b"b"));
    }

    #[test]
    fn compute_answers_again_from_the_cache_only_for_the_same_commit_and_text() {
        let git = |d: &Path, args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(d)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Ada Lovelace")
                .env("GIT_AUTHOR_EMAIL", "ada@example.invalid")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                .output()
                .is_ok_and(|o| o.status.success())
        };
        let tmp = |name: &str| {
            let d = std::env::temp_dir().join(format!("cho-blame-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let dir = tmp("repo");
        let d = dir.as_path();
        if !git(d, &["init", "-q", "-b", "main"]) {
            return; // no git here
        }
        std::fs::write(d.join("a.rs"), "one\n").unwrap();
        assert!(git(d, &["add", "."]) && git(d, &["commit", "-q", "-m", "first"]));
        let cache = Mutex::new(Cache::default());
        let path = d.join("a.rs");
        let a = compute(&cache, &path, None, b"one\n", None).unwrap();
        assert_eq!(a.label(0, 0).unwrap().0, "Ada");
        let again = compute(&cache, &path, None, b"one\n", None).unwrap();
        assert!(Arc::ptr_eq(&a, &again), "same commit and text: cached");
        let edited = compute(&cache, &path, None, b"one\ntwo\n", None).unwrap();
        assert!(!Arc::ptr_eq(&a, &edited));
        assert_eq!(edited.label(1, 0).unwrap().0, UNCOMMITTED);
        // A new commit is a new key, even for the same text.
        std::fs::write(d.join("a.rs"), "one\ntwo\n").unwrap();
        assert!(git(d, &["commit", "-q", "-am", "second"]));
        let committed = compute(&cache, &path, None, b"one\ntwo\n", None).unwrap();
        assert!(!committed.blame.commit_of(1).unwrap().uncommitted());
        assert_eq!(cache.lock().unwrap().len(), 3);
        let outside = tmp("outside");
        let err = compute(&cache, &outside.join("x.rs"), None, b"", None).unwrap_err();
        assert_eq!(err, "not in a git repository");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&outside).ok();
    }
}

//! What an indexer says while it runs, as a step for the status bar.
//!
//! Pure functions over lines of output, so they are tested on samples without running anything.
//! Only the rust-analyzer samples in the tests were captured from the real tool. The
//! scip-typescript and scip-python formats are read tolerantly (a path to a `tsconfig*.json` or
//! `package.json`, an `n/m` or `n of m` counter, a leading "Indexing"); a line that matches none
//! of those is ignored, so an unknown format shows elapsed time only, never a wrong step.

use std::time::Duration;

/// Where an indexer is, as far as its output says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// The package or phase now being worked on, e.g. `packages/core`.
    pub step: Option<String>,
    /// Which step this is: the tool's own counter, or the number of distinct steps seen so far.
    pub n: u32,
    /// The total, only when the tool printed one.
    pub of: Option<u32>,
}

/// One recognised line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub position: Option<(u32, u32)>,
}

/// Reads a stream of output lines into [`Progress`].
#[derive(Debug, Default)]
pub struct Tracker {
    progress: Progress,
}

impl Tracker {
    pub fn feed(&mut self, line: &str) {
        let Some(step) = parse_line(line) else {
            return;
        };
        let p = &mut self.progress;
        match step.position {
            Some((n, of)) => {
                p.n = n;
                p.of = Some(of);
            }
            // A step without a counter after ones with a total never counts past that total.
            None if p.step.as_deref() != Some(step.name.as_str()) => {
                p.n = p.of.map_or(p.n + 1, |of| (p.n + 1).min(of));
            }
            None => {}
        }
        p.step = Some(step.name);
    }

    pub fn snapshot(&self) -> Progress {
        self.progress.clone()
    }
}

/// Removes ANSI escape sequences (colours, cursor moves) that progress bars write.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `n/m` or `n of m` in `s`, with `n <= m` and `m > 0`.
fn counter(s: &str) -> Option<(u32, u32)> {
    let words: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || "()[],:".contains(c))
        .filter(|w| !w.is_empty())
        .collect();
    let num = |w: &str| w.parse::<u32>().ok();
    for (i, w) in words.iter().enumerate() {
        if let Some((a, b)) = w.split_once('/')
            && let (Some(a), Some(b)) = (num(a), num(b))
            && b > 0
            && a <= b
        {
            return Some((a, b));
        }
        if *w == "of"
            && i > 0
            && let (Some(a), Some(b)) = (num(words[i - 1]), words.get(i + 1).and_then(|b| num(b)))
            && b > 0
            && a <= b
        {
            return Some((a, b));
        }
    }
    None
}

/// The directory of a `tsconfig*.json` / `package.json` path in `s`, or `.` for one at the root.
fn project_path(s: &str) -> Option<String> {
    for word in s.split(|c: char| c.is_whitespace() || "()[]\"'`,".contains(c)) {
        let file = word.rsplit('/').next().unwrap_or(word);
        let is_config = file == "package.json"
            || (file.starts_with("tsconfig") && file.ends_with(".json"))
            || file == "jsconfig.json";
        if is_config {
            let dir = word
                .strip_suffix(file)
                .unwrap_or("")
                .trim_end_matches('/')
                .trim_start_matches("./");
            return Some(if dir.is_empty() {
                ".".into()
            } else {
                dir.into()
            });
        }
    }
    None
}

/// The last non-empty segment of a line, since progress bars redraw with `\r`.
fn last_segment(line: &str) -> &str {
    line.rsplit('\r')
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("")
}

/// One line of indexer output as a step, or `None` when it says nothing about progress.
pub fn parse_line(line: &str) -> Option<Step> {
    let line = strip_ansi(last_segment(line));
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // rust-analyzer: `rust-analyzer: Loading cargo metadata: started`, `... Indexing 12/340`.
    if let Some(rest) = line.strip_prefix("rust-analyzer:") {
        let position = counter(rest);
        let mut text = rest.trim();
        for suffix in [": started", ": finished", ": ?"] {
            text = text.strip_suffix(suffix).unwrap_or(text).trim_end();
        }
        let name = match position {
            Some(_) => text
                .rsplit_once(' ')
                .map_or(text, |(head, _)| head)
                .trim_end_matches(':')
                .trim(),
            None => text,
        };
        return (!name.is_empty()).then(|| Step {
            name: name.to_string(),
            position,
        });
    }
    // Tools that name a project by its config file.
    if let Some(name) = project_path(line) {
        return Some(Step {
            name,
            position: counter(line),
        });
    }
    // "Indexing foo" with no path.
    let lower = line.to_ascii_lowercase();
    for prefix in ["indexing ", "indexing: ", "processing "] {
        if lower.starts_with(prefix) {
            let name = line[prefix.len()..].trim();
            let position = counter(name);
            let name = name.split(" [").next().unwrap_or(name).trim();
            if !name.is_empty() {
                return Some(Step {
                    name: name
                        .chars()
                        .take(80)
                        .collect::<String>()
                        .trim_end()
                        .to_string(),
                    position,
                });
            }
        }
    }
    None
}

/// `2m 10s`, `45s`, `1h 02m`.
pub fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// `s` with control characters and bidirectional overrides replaced by spaces: step names come
/// from file paths in the repository, which can hold anything, and the status bar must not
/// let them redraw the line or reorder the text after them.
fn printable(s: &str) -> String {
    s.chars()
        .map(|c| {
            let hides = c.is_control()
                || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
            if hides { ' ' } else { c }
        })
        .collect()
}

/// The status line for a running build: `indexing packages/core (14 of 38) · 2m 10s`. `label`
/// names the project when the indexer has printed no step yet. Elapsed time is always there.
pub fn describe(verb: &str, label: &str, progress: &Progress, elapsed: Duration) -> String {
    let name = printable(progress.step.as_deref().unwrap_or(label));
    let count = match (progress.step.is_some(), progress.n, progress.of) {
        (true, n, Some(of)) => format!(" ({n} of {of})"),
        (true, n, None) if n > 0 => format!(" ({n})"),
        _ => String::new(),
    };
    format!("{verb} {name}{count} \u{b7} {}", format_elapsed(elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by the `progress_lines` fuzz target: steps without a counter after a counted one
    /// ran `n` past the total, so the status line read "4 of 3".
    #[test]
    fn uncounted_steps_never_pass_the_total() {
        let p = run(
            "rust-analyzer: Roots Scanned 3/3\nrust-analyzer: Loading a\nrust-analyzer: Loading b\n",
        );
        assert_eq!((p.n, p.of), (3, Some(3)));
    }

    /// Found by the `progress_lines` fuzz target: whitespace before a stripped `: started`
    /// stayed on the step name.
    #[test]
    fn phase_names_lose_whitespace_before_the_suffix() {
        let step = parse_line("rust-analyzer: Loading \u{b}: started").unwrap();
        assert_eq!(step.name, "Loading");
    }

    /// Found by the `progress_lines` fuzz target: cutting a long name at 80 characters could
    /// leave trailing whitespace on it.
    #[test]
    fn long_step_names_are_trimmed_after_the_cut() {
        let line = format!("indexing {} tail", "a".repeat(79));
        let step = parse_line(&line).unwrap();
        assert_eq!(step.name, "a".repeat(79));
    }

    /// Found by the `progress_lines` fuzz target: a step name taken from a path with control
    /// or bidirectional characters reached the status line unchanged.
    #[test]
    fn describe_hides_control_and_bidi_characters() {
        let mut t = Tracker::default();
        t.feed("indexing a\x07b\u{202e}c\td");
        assert!(t.snapshot().step.is_some());
        let s = describe("indexing", "x\ny", &t.snapshot(), Duration::from_secs(1));
        assert!(
            !s.chars().any(|c| c.is_control() || c == '\u{202e}'),
            "{s:?}"
        );
        let s = describe(
            "indexing",
            "x\ny",
            &Progress::default(),
            Duration::from_secs(1),
        );
        assert!(!s.contains('\n'));
    }

    /// Captured from `rust-analyzer scip` on a one-crate project.
    const RUST_ANALYZER: &str = "Generating SCIP start...
rust-analyzer: Loading discovering sysroot
rust-analyzer: Loading querying project metadata
rust-analyzer: Loading cargo metadata: started
rust-analyzer: Loading cargo metadata: ?
rust-analyzer: Loading cargo metadata: finished
Generating SCIP finished 5.60976798s
";

    fn run(lines: &str) -> Progress {
        let mut t = Tracker::default();
        lines.lines().for_each(|l| t.feed(l));
        t.snapshot()
    }

    #[test]
    fn rust_analyzer_phases_are_steps_without_counts() {
        let p = run(RUST_ANALYZER);
        assert_eq!(p.step.as_deref(), Some("Loading cargo metadata"));
        assert_eq!(p.of, None);
        assert_eq!(parse_line("Generating SCIP start..."), None);
        assert_eq!(
            parse_line("rust-analyzer: Roots Scanned 3/10"),
            Some(Step {
                name: "Roots Scanned".into(),
                position: Some((3, 10))
            })
        );
    }

    // Sample shapes for scip-typescript and scip-python. These are not captured output: they
    // are the forms the parser accepts, kept so a change cannot silently drop them.
    #[test]
    fn config_paths_name_the_package() {
        let s = parse_line("indexing packages/core/tsconfig.json").unwrap();
        assert_eq!((s.name.as_str(), s.position), ("packages/core", None));
        let s = parse_line("  [14/38] ./packages/web/package.json").unwrap();
        assert_eq!(
            (s.name.as_str(), s.position),
            ("packages/web", Some((14, 38)))
        );
        assert_eq!(parse_line("tsconfig.json").unwrap().name, ".");
        assert_eq!(
            parse_line("/home/x/repo/apps/api/tsconfig.build.json")
                .unwrap()
                .name,
            "/home/x/repo/apps/api"
        );
    }

    #[test]
    fn without_a_total_the_steps_seen_are_counted() {
        let p = run(
            "indexing packages/a/tsconfig.json\nnoise\nindexing packages/b/tsconfig.json\nindexing packages/b/tsconfig.json\n",
        );
        assert_eq!(p.step.as_deref(), Some("packages/b"));
        assert_eq!((p.n, p.of), (2, None));
    }

    #[test]
    fn tool_counters_win() {
        let p = run("[3/10] packages/a/package.json\n[4/10] packages/b/package.json\n");
        assert_eq!((p.n, p.of), (4, Some(10)));
    }

    #[test]
    fn progress_bars_and_colours_are_reduced_to_the_last_redraw() {
        let line = "\u{1b}[2K\rIndexing a [=   ] 1/9\r\u{1b}[2K\rIndexing packages/z [==  ] 2/9";
        let s = parse_line(line).unwrap();
        assert_eq!((s.name.as_str(), s.position), ("packages/z", Some((2, 9))));
    }

    #[test]
    fn unknown_lines_and_nonsense_counters_are_ignored() {
        for l in [
            "",
            "   ",
            "warning: something happened",
            "downloaded 5/3 files",
            "0/0",
        ] {
            assert_eq!(parse_line(l), None, "{l:?}");
        }
    }

    #[test]
    fn elapsed_time_is_always_in_the_line() {
        let secs = Duration::from_secs;
        let none = Progress::default();
        assert_eq!(
            describe("indexing", "typescript", &none, secs(0)),
            "indexing typescript \u{b7} 0s"
        );
        let p = Progress {
            step: Some("packages/core".into()),
            n: 14,
            of: Some(38),
        };
        assert_eq!(
            describe("indexing", "typescript", &p, secs(130)),
            "indexing packages/core (14 of 38) \u{b7} 2m 10s"
        );
        let p = Progress { of: None, ..p };
        assert_eq!(
            describe("refreshing", "x", &p, secs(3720)),
            "refreshing packages/core (14) \u{b7} 1h 02m"
        );
    }
}

#![no_main]
//! Indexer and git progress lines: arbitrary text must never panic, counters stay sane and
//! the status line has no control characters.
//!
//! The input is split into lines on `\n` (progress bars redraw with `\r`, which stays inside
//! a line). Each line goes to `cho_semantic::progress::parse_line`, a `Tracker`, and
//! `cho_sources::progress::parse_progress`; the whole text also goes in as one line.
use libfuzzer_sys::fuzz_target;
use cho_semantic::progress::{Progress, Tracker, describe, format_elapsed, parse_line};
use cho_sources::progress::parse_progress;
use std::time::Duration;

fn no_controls(s: &str, what: &str) {
    assert!(
        !s.chars().any(|c| {
            c.is_control()
                || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        }),
        "{what} has a control character: {s:?}"
    );
}

fn check_progress(p: &Progress) {
    if let Some(of) = p.of {
        assert!(of > 0, "total of zero: {p:?}");
        assert!(p.n <= of, "done beyond total: {p:?}");
    }
}

fn check_line(line: &str, tracker: &mut Tracker) {
    if let Some(step) = parse_line(line) {
        assert!(!step.name.is_empty(), "empty step name from {line:?}");
        assert_eq!(step.name.trim(), step.name, "untrimmed name from {line:?}");
        if let Some((n, of)) = step.position {
            assert!(of > 0 && n <= of, "bad position {n}/{of} from {line:?}");
        }
        // Parsing is a pure function of the line.
        assert_eq!(parse_line(line), Some(step));
    }
    let before = tracker.snapshot();
    tracker.feed(line);
    let after = tracker.snapshot();
    check_progress(&after);
    // A known total may be replaced but is never lost.
    if before.of.is_some() {
        assert!(after.of.is_some(), "total lost: {before:?} -> {after:?}");
    }

    if let Some(g) = parse_progress(line) {
        assert!(g.percent <= 100, "percent {} from {line:?}", g.percent);
        assert!(!g.phase.is_empty(), "empty phase from {line:?}");
        assert_eq!(g.phase.trim(), g.phase);
        assert_eq!(parse_progress(line), Some(g));
    }
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut tracker = Tracker::default();
    for line in text.split('\n') {
        check_line(line, &mut tracker);
    }
    check_line(&text, &mut tracker);

    let p = tracker.snapshot();
    let secs = data
        .iter()
        .take(8)
        .fold(0u64, |a, b| a << 8 | u64::from(*b));
    let elapsed = Duration::from_secs(secs);
    let verb = "indexing";
    let s = describe(verb, "project", &p, elapsed);
    no_controls(&s, "describe");
    assert!(s.starts_with(verb));
    assert!(s.contains(&format_elapsed(elapsed)));
    no_controls(&format_elapsed(elapsed), "elapsed");
    let empty = describe(verb, "project", &Progress::default(), elapsed);
    assert!(empty.contains("project"));
});

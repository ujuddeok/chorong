//! Short texts every front end shows the same way: relative times, byte sizes and commit ids.
//! The CLI, the viewer and the settings window all use these, so they never disagree.

/// A relative time: `just now`, `12 min ago`, `2 h ago`, `1 day ago`, `3 days ago` (the wording
/// the user guide shows). `now` and `then` are seconds since the Unix epoch; a `then` in the
/// future (a clock behind) is `just now`.
pub fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "1 day ago".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// A byte count in binary units: `900 B`, `4 KB`, `12 MB`, `3.4 GB`.
pub fn size_text(bytes: u64) -> String {
    const K: u64 = 1024;
    let b = bytes as f64;
    if bytes < K {
        format!("{bytes} B")
    } else if bytes < K * K {
        format!("{} KB", bytes / K)
    } else if bytes < K * K * K {
        format!("{:.0} MB", b / (K * K) as f64)
    } else {
        format!("{:.1} GB", b / (K * K * K) as f64)
    }
}

/// The first seven characters of a commit id (never cuts inside a character, whatever the
/// text is).
pub fn short_commit(commit: &str) -> String {
    commit.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_wording() {
        assert_eq!(ago(100, 100), "just now");
        assert_eq!(ago(159, 100), "just now");
        assert_eq!(ago(160, 100), "1 min ago");
        assert_eq!(ago(100 + 12 * 60, 100), "12 min ago");
        assert_eq!(ago(100 + 2 * 3600 + 59, 100), "2 h ago");
        // Regression: the CLI and the viewer's header band said "1 days ago".
        assert_eq!(ago(90_000, 0), "1 day ago");
        assert_eq!(ago(3 * 86_400, 0), "3 days ago");
        assert_eq!(ago(5, 100), "just now", "a clock behind");
    }

    #[test]
    fn size_wording() {
        assert_eq!(size_text(0), "0 B");
        // Regression: the settings window's peak memory said "0 KB" for 500 bytes.
        assert_eq!(size_text(500), "500 B");
        assert_eq!(size_text(4096), "4 KB");
        assert_eq!(size_text(12 * 1024 * 1024), "12 MB");
        assert_eq!(size_text(3_650_722_201), "3.4 GB");
    }

    #[test]
    fn short_commits_cut_by_character() {
        assert_eq!(short_commit("3f9c1e2a77b0"), "3f9c1e2");
        assert_eq!(short_commit("abc"), "abc");
        assert_eq!(short_commit("ééééééééé"), "ééééééé");
    }
}

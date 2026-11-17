//! Developer timing lines on stderr, turned on by setting `CHORONG_SWITCH_LOG`: side switches,
//! the Changes list, word diffs, graph panes and investigation pages. The verify scripts and
//! `perf-app.sh` read these lines, so their text is part of those checks. Off, nothing is
//! printed and the check is one cached flag read.

/// Whether the timing lines are on (`CHORONG_SWITCH_LOG` is set when Chorong starts).
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CHORONG_SWITCH_LOG").is_some())
}

//! Process and thread CPU time and peak memory, where the platform reports them.
//!
//! Linux reads `/proc`. Other platforms report [`Reading::Unavailable`] with a reason; nothing
//! here is ever guessed. These calls read files, so they belong to background work (a few per
//! job), never to a per-call or per-frame path.

use std::time::Duration;

/// A measurement, or the reason there isn't one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading<T> {
    Available(T),
    Unavailable(&'static str),
}

impl<T> Reading<T> {
    pub fn available(self) -> Option<T> {
        match self {
            Reading::Available(v) => Some(v),
            Reading::Unavailable(_) => None,
        }
    }
}

/// CPU time and peak resident memory of a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessUsage {
    /// User plus system CPU time since the process started.
    pub cpu_time: Duration,
    /// Peak resident set size in bytes (the high-water mark).
    pub peak_rss_bytes: u64,
}

/// Whether this platform reports CPU time and memory at all.
pub const PLATFORM_SUPPORTED: bool = cfg!(target_os = "linux");

#[cfg(not(target_os = "linux"))]
const UNSUPPORTED: &str = "CPU time and peak memory are only reported on Linux so far";

/// Usage of this process.
pub fn current_process() -> Reading<ProcessUsage> {
    process("self")
}

/// CPU time used so far by the calling thread.
#[allow(clippy::disallowed_methods)] // /proc
pub fn current_thread_cpu() -> Reading<Duration> {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/thread-self/stat")
            .ok()
            .and_then(|t| parse_stat_cpu(&t))
        {
            Some(d) => Reading::Available(d),
            None => Reading::Unavailable("could not read /proc/thread-self/stat"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Reading::Unavailable(UNSUPPORTED)
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)] // /proc
fn process(who: &str) -> Reading<ProcessUsage> {
    let stat = std::fs::read_to_string(format!("/proc/{who}/stat"))
        .ok()
        .and_then(|t| parse_stat_cpu(&t));
    let status = std::fs::read_to_string(format!("/proc/{who}/status"))
        .ok()
        .and_then(|t| parse_vm_hwm(&t));
    match (stat, status) {
        (Some(cpu_time), Some(peak_rss_bytes)) => Reading::Available(ProcessUsage {
            cpu_time,
            peak_rss_bytes,
        }),
        _ => Reading::Unavailable("process not found or /proc not readable"),
    }
}

#[cfg(not(target_os = "linux"))]
fn process(_who: &str) -> Reading<ProcessUsage> {
    Reading::Unavailable(UNSUPPORTED)
}

/// `utime + stime` from a `/proc/.../stat` line. The command name is in parentheses and may
/// contain spaces, so fields are counted after the last `)`. Clock ticks are 100 per second on
/// every Linux configuration Chorong targets.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat_cpu(stat: &str) -> Option<Duration> {
    let after = &stat[stat.rfind(')')? + 1..];
    let mut fields = after.split_whitespace();
    // `after` starts at field 3 (state); utime and stime are fields 14 and 15.
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(Duration::from_millis((utime + stime) * 10))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_vm_hwm(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_with_spaces_in_command_name() {
        let line = "42 (my (odd) cmd) S 1 2 3 4 5 6 7 8 9 10 150 50 0 0 20 0 1 0 100";
        // fields after ')': S=3 ... utime=14 -> 150, stime=15 -> 50 -> 200 ticks = 2 s.
        assert_eq!(parse_stat_cpu(line), Some(Duration::from_secs(2)));
    }

    #[test]
    fn status_peak() {
        assert_eq!(
            parse_vm_hwm("Name:\tx\nVmHWM:\t    2048 kB\nVmRSS:\t 1 kB\n"),
            Some(2048 * 1024)
        );
        assert_eq!(parse_vm_hwm("Name:\tx\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_reports_this_process() {
        let u = current_process().available().expect("linux has /proc");
        assert!(u.peak_rss_bytes > 0);
        assert!(current_thread_cpu().available().is_some());
        assert!(process(&u32::MAX.to_string()).available().is_none());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn other_platforms_say_unavailable() {
        assert!(matches!(current_process(), Reading::Unavailable(_)));
    }
}

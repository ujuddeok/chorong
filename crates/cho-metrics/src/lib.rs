//! Always-on cost measurement for Chorong's optional features.
//!
//! One [`Metrics`] holds a fixed set of features, each named by its setting key
//! (`"context.follow_hover"`). Registering happens once, at startup; after that every call is
//! by [`FeatureId`], a plain index.
//!
//! ## Cost of recording
//!
//! Recording a call is a handful of relaxed atomic updates on cache lines owned by that
//! feature: no lock, no allocation, no syscall beyond the two clock reads of the timer. See
//! `examples/bench.rs` for measured numbers.
//!
//! ## What is kept
//!
//! - **Per feature:** call count, total, worst and average duration since start.
//! - **Rolling window:** the last five minutes, in sixty buckets of five seconds, so the
//!   window is accurate to five seconds. A bucket is recycled by whichever thread first
//!   writes to it in a new interval; a sample racing with that reset can be lost. Totals
//!   never are.
//! - **Background work:** CPU time and peak memory where the platform reports them
//!   ([`usage`]).
//! - **Frames:** frames over 8.3 ms and 16.7 ms, with the foreground features that ran during
//!   each such frame ([`Kind::Foreground`]).
//!
//! Nothing leaves the machine; nothing here guesses. Times are measured, or absent.

pub mod usage;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// Frames longer than this miss a 120 Hz display.
pub const FRAME_BUDGET_120HZ: Duration = Duration::from_micros(8_300);
/// Frames longer than this miss a 60 Hz display.
pub const FRAME_BUDGET_60HZ: Duration = Duration::from_micros(16_700);

/// Width of the rolling window.
pub const WINDOW: Duration = Duration::from_secs(300);
const BUCKET_NS: u64 = 5_000_000_000;
const BUCKETS: usize = (WINDOW.as_nanos() as u64 / BUCKET_NS) as usize;

/// Whether a feature's time counts against the frame it ran in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Runs on the UI thread: blamed for slow frames it ran in.
    Foreground,
    /// Runs elsewhere: counted, but never blamed for a frame.
    Background,
}

/// A feature's slot. Cheap to copy; obtain from [`Metrics::id`] once and keep it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FeatureId(u32);

impl FeatureId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Default)]
struct Bucket {
    tick: AtomicU64,
    count: AtomicU64,
    total_ns: AtomicU64,
}

struct Feature {
    key: &'static str,
    kind: Kind,
    count: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    buckets: [Bucket; BUCKETS],
    // Background usage (explicitly reported).
    bg_samples: AtomicU64,
    bg_cpu_ns: AtomicU64,
    bg_peak_rss: AtomicU64,
    // Frame attribution.
    frame_seq: AtomicU64,
    frame_ns: AtomicU64,
    slow_8_3: AtomicU64,
    slow_16_7: AtomicU64,
    slow_ns: AtomicU64,
}

impl Feature {
    fn new(key: &'static str, kind: Kind) -> Self {
        Self {
            key,
            kind,
            count: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| Bucket {
                tick: AtomicU64::new(u64::MAX),
                ..Bucket::default()
            }),
            bg_samples: AtomicU64::new(0),
            bg_cpu_ns: AtomicU64::new(0),
            bg_peak_rss: AtomicU64::new(0),
            frame_seq: AtomicU64::new(0),
            frame_ns: AtomicU64::new(0),
            slow_8_3: AtomicU64::new(0),
            slow_16_7: AtomicU64::new(0),
            slow_ns: AtomicU64::new(0),
        }
    }
}

#[derive(Default)]
struct FrameState {
    /// Sequence number of the current (or last) frame; 0 before the first.
    seq: AtomicU64,
    open: AtomicBool,
    start_ns: AtomicU64,
    frames: AtomicU64,
    total_ns: AtomicU64,
    worst_ns: AtomicU64,
    over_8_3: AtomicU64,
    over_16_7: AtomicU64,
}

/// The measurements for a fixed set of features. Share it as `Arc<Metrics>`.
pub struct Metrics {
    epoch: Instant,
    features: Box<[Feature]>,
    frame: FrameState,
}

fn ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

impl Metrics {
    /// Register the features once. Keys should be the setting keys from `cho-settings`.
    pub fn new(features: &[(&'static str, Kind)]) -> Self {
        Self {
            epoch: Instant::now(),
            features: features
                .iter()
                .map(|&(k, kind)| Feature::new(k, kind))
                .collect(),
            frame: FrameState::default(),
        }
    }

    /// Time since this `Metrics` was created, the clock all `*_at` methods use.
    pub fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    /// Look a feature up by key. Do this once at startup, not per call.
    pub fn id(&self, key: &str) -> Option<FeatureId> {
        self.features
            .iter()
            .position(|f| f.key == key)
            .map(|i| FeatureId(i as u32))
    }

    pub fn keys(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.features.iter().map(|f| f.key)
    }

    /// Start timing; the call is recorded when the guard drops.
    #[must_use = "the call is recorded when the timer is dropped"]
    pub fn start(&self, id: FeatureId) -> Timer<'_> {
        Timer {
            metrics: self,
            id,
            started: Instant::now(),
        }
    }

    /// Time a closure.
    pub fn time<R>(&self, id: FeatureId, f: impl FnOnce() -> R) -> R {
        let _t = self.start(id);
        f()
    }

    /// Record a call that took `elapsed`, finishing now.
    pub fn record(&self, id: FeatureId, elapsed: Duration) {
        self.record_at(id, elapsed, self.now());
    }

    /// Record a call that took `elapsed` and finished `at` after creation. Deterministic;
    /// what the tests and replays use.
    pub fn record_at(&self, id: FeatureId, elapsed: Duration, at: Duration) {
        let d = ns(elapsed);
        let f = &self.features[id.index()];
        f.count.fetch_add(1, Relaxed);
        f.total_ns.fetch_add(d, Relaxed);
        f.max_ns.fetch_max(d, Relaxed);

        let tick = ns(at) / BUCKET_NS;
        let b = &f.buckets[(tick % BUCKETS as u64) as usize];
        let seen = b.tick.load(Relaxed);
        if seen != tick
            && b.tick
                .compare_exchange(seen, tick, Relaxed, Relaxed)
                .is_ok()
        {
            b.count.store(0, Relaxed);
            b.total_ns.store(0, Relaxed);
        }
        b.count.fetch_add(1, Relaxed);
        b.total_ns.fetch_add(d, Relaxed);

        if f.kind == Kind::Foreground && self.frame.open.load(Relaxed) {
            let seq = self.frame.seq.load(Relaxed);
            if f.frame_seq.swap(seq, Relaxed) == seq {
                f.frame_ns.fetch_add(d, Relaxed);
            } else {
                f.frame_ns.store(d, Relaxed);
            }
        }
    }

    /// Report CPU time and peak memory measured for background work (for example a finished
    /// indexer subprocess). CPU time adds up; peak memory keeps the maximum.
    pub fn record_background_usage(&self, id: FeatureId, cpu: Duration, peak_rss_bytes: u64) {
        let f = &self.features[id.index()];
        f.bg_samples.fetch_add(1, Relaxed);
        f.bg_cpu_ns.fetch_add(ns(cpu), Relaxed);
        f.bg_peak_rss.fetch_max(peak_rss_bytes, Relaxed);
    }

    /// Time background work run on the current thread, recording its wall time like a
    /// [`Timer`] and, where the platform reports it, the thread's CPU time. Reads `/proc`
    /// twice, so use it per job, not per call.
    #[must_use = "the work is recorded when the span is dropped"]
    pub fn background_span(&self, id: FeatureId) -> BackgroundSpan<'_> {
        BackgroundSpan {
            timer: self.start(id),
            cpu_start: usage::current_thread_cpu().available(),
        }
    }

    // ---- frames ----

    pub fn frame_begin(&self) {
        self.frame_begin_at(self.now());
    }

    pub fn frame_begin_at(&self, at: Duration) {
        self.frame.seq.fetch_add(1, Relaxed);
        self.frame.start_ns.store(ns(at), Relaxed);
        self.frame.open.store(true, Relaxed);
    }

    /// Close the frame opened by `frame_begin`. `None` if none was open.
    pub fn frame_end(&self) -> Option<FrameOutcome> {
        self.frame_end_at(self.now())
    }

    pub fn frame_end_at(&self, at: Duration) -> Option<FrameOutcome> {
        if !self.frame.open.swap(false, Relaxed) {
            return None;
        }
        let d = ns(at).saturating_sub(self.frame.start_ns.load(Relaxed));
        let fr = &self.frame;
        fr.frames.fetch_add(1, Relaxed);
        fr.total_ns.fetch_add(d, Relaxed);
        fr.worst_ns.fetch_max(d, Relaxed);
        let over_8_3 = d > ns(FRAME_BUDGET_120HZ);
        let over_16_7 = d > ns(FRAME_BUDGET_60HZ);
        if over_8_3 {
            fr.over_8_3.fetch_add(1, Relaxed);
            if over_16_7 {
                fr.over_16_7.fetch_add(1, Relaxed);
            }
            let seq = fr.seq.load(Relaxed);
            for f in self
                .features
                .iter()
                .filter(|f| f.frame_seq.load(Relaxed) == seq)
            {
                f.slow_8_3.fetch_add(1, Relaxed);
                if over_16_7 {
                    f.slow_16_7.fetch_add(1, Relaxed);
                }
                f.slow_ns.fetch_add(f.frame_ns.load(Relaxed), Relaxed);
            }
        }
        Some(FrameOutcome {
            duration: Duration::from_nanos(d),
            over_8_3,
            over_16_7,
        })
    }

    pub fn frames(&self) -> FrameStats {
        let fr = &self.frame;
        FrameStats {
            frames: fr.frames.load(Relaxed),
            total: Duration::from_nanos(fr.total_ns.load(Relaxed)),
            worst: Duration::from_nanos(fr.worst_ns.load(Relaxed)),
            over_8_3: fr.over_8_3.load(Relaxed),
            over_16_7: fr.over_16_7.load(Relaxed),
        }
    }

    // ---- queries ----

    pub fn stats(&self, id: FeatureId) -> FeatureStats {
        self.stats_at(id, self.now())
    }

    pub fn stats_at(&self, id: FeatureId, now: Duration) -> FeatureStats {
        let f = &self.features[id.index()];
        let count = f.count.load(Relaxed);
        let total = f.total_ns.load(Relaxed);
        let now_tick = ns(now) / BUCKET_NS;
        let mut window = WindowStats::default();
        for b in &f.buckets {
            let t = b.tick.load(Relaxed);
            // The newest BUCKETS ticks: now_tick-BUCKETS+1 ..= now_tick.
            if t != u64::MAX && t <= now_tick && now_tick - t < BUCKETS as u64 {
                window.count += b.count.load(Relaxed);
                window.total += Duration::from_nanos(b.total_ns.load(Relaxed));
            }
        }
        let samples = f.bg_samples.load(Relaxed);
        FeatureStats {
            key: f.key,
            id,
            count,
            total: Duration::from_nanos(total),
            average: Duration::from_nanos(total.checked_div(count).unwrap_or(0)),
            worst: Duration::from_nanos(f.max_ns.load(Relaxed)),
            window,
            background: (samples > 0).then(|| BackgroundStats {
                samples,
                cpu_total: Duration::from_nanos(f.bg_cpu_ns.load(Relaxed)),
                peak_rss_bytes: f.bg_peak_rss.load(Relaxed),
            }),
            slow_frames_8_3: f.slow_8_3.load(Relaxed),
            slow_frames_16_7: f.slow_16_7.load(Relaxed),
            slow_frame_time: Duration::from_nanos(f.slow_ns.load(Relaxed)),
        }
    }

    pub fn all(&self) -> Vec<FeatureStats> {
        let now = self.now();
        (0..self.features.len())
            .map(|i| self.stats_at(FeatureId(i as u32), now))
            .collect()
    }

    /// "Costing the most right now": up to `n` features with the highest total time in the
    /// last five minutes, highest first. Features with no time in the window are left out;
    /// ties break by key so the answer is deterministic.
    pub fn top_by_window_total(&self, n: usize) -> Vec<FeatureStats> {
        self.top_at(n, self.now())
    }

    pub fn top_at(&self, n: usize, now: Duration) -> Vec<FeatureStats> {
        let mut all: Vec<FeatureStats> = (0..self.features.len())
            .map(|i| self.stats_at(FeatureId(i as u32), now))
            .filter(|s| !s.window.total.is_zero())
            .collect();
        all.sort_by(|a, b| b.window.total.cmp(&a.window.total).then(a.key.cmp(b.key)));
        all.truncate(n);
        all
    }
}

/// Records one call when dropped.
pub struct Timer<'a> {
    metrics: &'a Metrics,
    id: FeatureId,
    started: Instant,
}

impl Drop for Timer<'_> {
    fn drop(&mut self) {
        let end = Instant::now();
        self.metrics.record_at(
            self.id,
            end.saturating_duration_since(self.started),
            end.saturating_duration_since(self.metrics.epoch),
        );
    }
}

/// Records one background job when dropped: wall time as a normal call, thread CPU time
/// where available.
pub struct BackgroundSpan<'a> {
    timer: Timer<'a>,
    cpu_start: Option<Duration>,
}

impl Drop for BackgroundSpan<'_> {
    fn drop(&mut self) {
        if let (Some(start), Some(end)) = (self.cpu_start, usage::current_thread_cpu().available())
        {
            let m = self.timer.metrics;
            let f = &m.features[self.timer.id.index()];
            f.bg_samples.fetch_add(1, Relaxed);
            f.bg_cpu_ns
                .fetch_add(ns(end.saturating_sub(start)), Relaxed);
        }
        // `timer` drops after this and records the wall time.
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowStats {
    pub count: u64,
    pub total: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackgroundStats {
    pub samples: u64,
    pub cpu_total: Duration,
    pub peak_rss_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureStats {
    pub key: &'static str,
    pub id: FeatureId,
    pub count: u64,
    pub total: Duration,
    pub average: Duration,
    pub worst: Duration,
    /// The last five minutes.
    pub window: WindowStats,
    /// Present once CPU or memory has been reported for this feature.
    pub background: Option<BackgroundStats>,
    /// Slow frames this feature ran in (foreground features only).
    pub slow_frames_8_3: u64,
    pub slow_frames_16_7: u64,
    /// Time the feature spent inside those slow frames.
    pub slow_frame_time: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameOutcome {
    pub duration: Duration,
    pub over_8_3: bool,
    pub over_16_7: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub frames: u64,
    pub total: Duration,
    pub worst: Duration,
    pub over_8_3: u64,
    pub over_16_7: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const MS: Duration = Duration::from_millis(1);
    const S: Duration = Duration::from_secs(1);

    fn m() -> Metrics {
        Metrics::new(&[
            ("a.fg", Kind::Foreground),
            ("b.fg", Kind::Foreground),
            ("c.bg", Kind::Background),
            ("d.idle", Kind::Foreground),
        ])
    }

    #[test]
    fn counts_total_average_worst() {
        let m = m();
        let a = m.id("a.fg").unwrap();
        m.record_at(a, 2 * MS, S);
        m.record_at(a, 6 * MS, S);
        m.record_at(a, 1 * MS, S);
        let s = m.stats_at(a, S);
        assert_eq!(s.count, 3);
        assert_eq!(s.total, 9 * MS);
        assert_eq!(s.average, 3 * MS);
        assert_eq!(s.worst, 6 * MS);
        assert_eq!(m.stats(m.id("d.idle").unwrap()).average, Duration::ZERO);
        assert!(m.id("nope").is_none());
    }

    #[test]
    fn window_keeps_five_minutes_and_forgets_older() {
        let m = m();
        let a = m.id("a.fg").unwrap();
        m.record_at(a, 10 * MS, Duration::from_secs(10));
        m.record_at(a, 20 * MS, Duration::from_secs(200));
        // At t=250 s both are inside the window.
        let w = m.stats_at(a, Duration::from_secs(250)).window;
        assert_eq!((w.count, w.total), (2, 30 * MS));
        // At t=320 s the first (t=10 s) has aged out; the second hasn't.
        let w = m.stats_at(a, Duration::from_secs(320)).window;
        assert_eq!((w.count, w.total), (1, 20 * MS));
        // Lifetime totals are unaffected.
        assert_eq!(m.stats_at(a, Duration::from_secs(320)).total, 30 * MS);
        // At t=600 s nothing is left in the window.
        assert_eq!(
            m.stats_at(a, Duration::from_secs(600)).window,
            WindowStats::default()
        );
    }

    #[test]
    fn bucket_is_recycled_for_a_new_interval() {
        let m = m();
        let a = m.id("a.fg").unwrap();
        // Same bucket slot, 300 s apart.
        m.record_at(a, 5 * MS, Duration::from_secs(1));
        m.record_at(a, 7 * MS, Duration::from_secs(301));
        let w = m.stats_at(a, Duration::from_secs(302)).window;
        assert_eq!((w.count, w.total), (1, 7 * MS));
    }

    #[test]
    fn top_three_by_window_total() {
        let m = Metrics::new(&[
            ("f.one", Kind::Foreground),
            ("f.two", Kind::Foreground),
            ("f.three", Kind::Foreground),
            ("f.four", Kind::Foreground),
            ("f.old", Kind::Foreground),
            ("f.never", Kind::Foreground),
        ]);
        let id = |k| m.id(k).unwrap();
        m.record_at(id("f.one"), 5 * MS, 400 * S);
        m.record_at(id("f.two"), 50 * MS, 400 * S);
        m.record_at(id("f.three"), 20 * MS, 401 * S);
        m.record_at(id("f.four"), 10 * MS, 402 * S);
        // Huge, but long ago: outranks everyone in lifetime total, not in the window.
        m.record_at(id("f.old"), 9000 * MS, 1 * S);
        let top = m.top_at(3, 410 * S);
        let keys: Vec<_> = top.iter().map(|s| s.key).collect();
        assert_eq!(keys, ["f.two", "f.three", "f.four"]);
        // Fewer than n active features: only those with time are returned.
        assert_eq!(m.top_at(10, 410 * S).len(), 4);
        // Ties break by key.
        m.record_at(id("f.one"), 5 * MS, 405 * S);
        m.record_at(id("f.four"), Duration::ZERO, 405 * S);
        let tied: Vec<_> = m.top_at(4, 410 * S).iter().map(|s| s.key).collect();
        assert_eq!(tied, ["f.two", "f.three", "f.four", "f.one"]);
        let m2 = Metrics::new(&[("z", Kind::Foreground), ("y", Kind::Foreground)]);
        m2.record_at(FeatureId(0), MS, S);
        m2.record_at(FeatureId(1), MS, S);
        assert_eq!(m2.top_at(2, S)[0].key, "y");
    }

    #[test]
    fn frames_are_counted_and_blamed() {
        let m = m();
        let (a, b, c) = (
            m.id("a.fg").unwrap(),
            m.id("b.fg").unwrap(),
            m.id("c.bg").unwrap(),
        );

        // Fast frame: 5 ms.
        m.frame_begin_at(S);
        m.record_at(a, 1 * MS, S);
        let o = m.frame_end_at(S + 5 * MS).unwrap();
        assert!(!o.over_8_3 && !o.over_16_7);

        // 10 ms frame: over 8.3 only. a and b ran; the background feature did not count.
        m.frame_begin_at(2 * S);
        m.record_at(a, 4 * MS, 2 * S);
        m.record_at(a, 3 * MS, 2 * S);
        m.record_at(b, 1 * MS, 2 * S);
        m.record_at(c, 50 * MS, 2 * S);
        let o = m.frame_end_at(2 * S + 10 * MS).unwrap();
        assert!(o.over_8_3 && !o.over_16_7);

        // 20 ms frame: over both. Only b ran.
        m.frame_begin_at(3 * S);
        m.record_at(b, 15 * MS, 3 * S);
        assert!(m.frame_end_at(3 * S + 20 * MS).unwrap().over_16_7);

        let f = m.frames();
        assert_eq!((f.frames, f.over_8_3, f.over_16_7), (3, 2, 1));
        assert_eq!(f.worst, 20 * MS);

        let (sa, sb, sc) = (
            m.stats_at(a, 4 * S),
            m.stats_at(b, 4 * S),
            m.stats_at(c, 4 * S),
        );
        assert_eq!((sa.slow_frames_8_3, sa.slow_frames_16_7), (1, 0));
        assert_eq!(sa.slow_frame_time, 7 * MS);
        assert_eq!((sb.slow_frames_8_3, sb.slow_frames_16_7), (2, 1));
        assert_eq!(sb.slow_frame_time, 16 * MS);
        assert_eq!(sc.slow_frames_8_3, 0, "background work is never blamed");
    }

    #[test]
    fn a_feature_that_did_not_run_in_the_slow_frame_is_not_blamed() {
        let m = m();
        let a = m.id("a.fg").unwrap();
        m.frame_begin_at(S);
        m.record_at(a, MS, S);
        m.frame_end_at(S + MS);
        m.frame_begin_at(2 * S);
        m.frame_end_at(2 * S + 30 * MS);
        assert_eq!(m.stats_at(a, 3 * S).slow_frames_8_3, 0);
        assert_eq!(m.frame_end_at(3 * S), None, "no frame open");
    }

    #[test]
    fn background_usage_adds_cpu_and_keeps_peak() {
        let m = m();
        let c = m.id("c.bg").unwrap();
        assert!(m.stats(c).background.is_none());
        m.record_background_usage(c, 2 * S, 100);
        m.record_background_usage(c, 3 * S, 70);
        let b = m.stats(c).background.unwrap();
        assert_eq!((b.samples, b.cpu_total, b.peak_rss_bytes), (2, 5 * S, 100));
    }

    #[test]
    fn timer_records_on_drop() {
        let m = m();
        let a = m.id("a.fg").unwrap();
        m.time(a, || std::thread::sleep(2 * MS));
        {
            let _t = m.start(a);
            std::thread::sleep(MS);
        }
        let s = m.stats(a);
        assert_eq!(s.count, 2);
        assert!(s.total >= 3 * MS);
        assert!(s.window.count == 2 && s.worst >= 2 * MS);
    }

    #[test]
    fn background_span_records_wall_time_and_cpu() {
        let m = m();
        let c = m.id("c.bg").unwrap();
        {
            let _s = m.background_span(c);
            let mut x = 0u64;
            for i in 0..20_000_000u64 {
                x = x.wrapping_add(i ^ (x >> 3));
            }
            std::hint::black_box(x);
        }
        let s = m.stats(c);
        assert_eq!(s.count, 1);
        if usage::PLATFORM_SUPPORTED {
            assert_eq!(s.background.unwrap().samples, 1);
        } else {
            assert!(s.background.is_none());
        }
    }

    #[test]
    fn many_threads_lose_nothing() {
        let m = Arc::new(m());
        let a = m.id("a.fg").unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let m = Arc::clone(&m);
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        m.record_at(a, Duration::from_nanos(10), Duration::from_secs(1));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let s = m.stats_at(a, S);
        assert_eq!(s.count, 40_000);
        assert_eq!(s.total, Duration::from_nanos(400_000));
        assert_eq!(s.window.count, 40_000);
    }
}

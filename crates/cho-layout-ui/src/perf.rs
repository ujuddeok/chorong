//! Frame timing and off-thread work for the shell (in the viewer and the layout lab).
//!
//! A frame is timed from the start of the root view's `render` to the end of painting (a
//! canvas deferred with the highest priority, so it paints last). That covers building the
//! element tree, layout, prepaint and paint on the UI thread; GPU submission and presentation
//! are not included. Frames are kept in a rolling window for the overlay, and optionally
//! written to a CSV file by a background thread, so logging never blocks the UI thread.
//!
//! Layout state is saved off-thread too: the UI thread hands a clone of the workspace (O(panes),
//! microseconds) to a saver thread, which waits until changes stop for [`SAVE_DEBOUNCE`] and
//! then serialises and writes it.

use cho_layout::Workspace;
use cho_metrics::{FeatureId, Metrics};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One frame at 120 Hz, in ms: the same budget `cho-metrics` counts against.
pub const BUDGET_120: f32 = cho_metrics::FRAME_BUDGET_120HZ.as_secs_f32() * 1000.0;
/// One frame at 60 Hz, in ms.
pub const BUDGET_60: f32 = cho_metrics::FRAME_BUDGET_60HZ.as_secs_f32() * 1000.0;
/// Frames kept for the overlay's statistics (5 s at 120 Hz).
pub const WINDOW: usize = 600;
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub last: f32,
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
    pub worst: f32,
    pub over_120: usize,
    pub over_60: usize,
}

/// Percentile by nearest rank over sorted samples.
pub fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f32).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn summarise(samples: impl Iterator<Item = f32>, last: f32) -> Summary {
    let mut v: Vec<f32> = samples.collect();
    v.sort_by(f32::total_cmp);
    Summary {
        frames: v.len(),
        last,
        p50: percentile(&v, 50.0),
        p95: percentile(&v, 95.0),
        p99: percentile(&v, 99.0),
        worst: v.last().copied().unwrap_or(0.0),
        over_120: v.iter().filter(|t| **t > BUDGET_120).count(),
        over_60: v.iter().filter(|t| **t > BUDGET_60).count(),
    }
}

pub struct FrameStats {
    window: VecDeque<f32>,
    last: f32,
    /// When the frame being drawn started.
    pub started: Option<Instant>,
    previous_start: Option<Instant>,
    /// How long the root view's `render` took (building the element tree), in ms.
    build_ms: f32,
    /// How long the last layout operation took, in microseconds.
    pub last_op_us: f32,
    /// When the current pointer-move dispatch started (its capture phase).
    input_started: Option<Instant>,
    /// The longest pointer-move dispatch since the last frame, in microseconds: every
    /// listener GPUI calls for the move, including each drag-move listener of every tab strip
    /// and drop zone, and the model probe when the drop target changes.
    input_us: f32,
    /// The longest dispatch in the rolling window, for the overlay.
    pub worst_input_us: VecDeque<f32>,
    log: Option<Sender<String>>,
    /// Frame-budget tracking and the cost of the frame log (`debug.frame_log`).
    metrics: Option<(Arc<Metrics>, FeatureId)>,
}

impl FrameStats {
    pub fn new(log: Option<PathBuf>) -> Self {
        Self {
            window: VecDeque::with_capacity(WINDOW),
            last: 0.0,
            started: None,
            previous_start: None,
            build_ms: 0.0,
            input_started: None,
            input_us: 0.0,
            worst_input_us: VecDeque::with_capacity(WINDOW),
            last_op_us: 0.0,
            log: log.map(spawn_log),
            metrics: None,
        }
    }

    /// Report every frame to `metrics` (budget tracking), and the time spent preparing a
    /// frame-log line under `log_feature`.
    pub fn with_metrics(mut self, metrics: Arc<Metrics>, log_feature: FeatureId) -> Self {
        self.metrics = Some((metrics, log_feature));
        self
    }

    /// Starts or stops writing the frame log, for a setting that changes while running.
    pub fn set_log(&mut self, path: Option<PathBuf>) {
        if path.is_some() != self.log.is_some() {
            self.log = path.map(spawn_log);
        }
    }

    pub fn begin(&mut self) {
        if let Some((m, _)) = &self.metrics {
            m.frame_begin();
        }
        let now = Instant::now();
        self.previous_start = self.started.replace(now);
    }

    /// A pointer move starts dispatching (capture phase, first listener).
    pub fn input_begin(&mut self) {
        self.input_started = Some(Instant::now());
    }

    /// The same pointer move finished dispatching (bubble phase, last listener).
    pub fn input_end(&mut self) {
        if let Some(t) = self.input_started.take() {
            self.input_us = self.input_us.max(t.elapsed().as_secs_f32() * 1e6);
        }
    }

    /// The longest pointer dispatch over the rolling window, in microseconds.
    pub fn worst_input(&self) -> f32 {
        self.worst_input_us.iter().copied().fold(0.0, f32::max)
    }

    /// Called when the root view has built its element tree.
    pub fn built(&mut self) {
        if let Some(start) = self.started {
            self.build_ms = start.elapsed().as_secs_f32() * 1000.0;
        }
    }

    /// Called when painting ends.
    pub fn end(&mut self) {
        let Some(start) = self.started else { return };
        let ms = start.elapsed().as_secs_f32() * 1000.0;
        let interval = self
            .previous_start
            .map_or(0.0, |p| (start - p).as_secs_f32() * 1000.0);
        self.last = ms;
        if self.window.len() == WINDOW {
            self.window.pop_front();
        }
        self.window.push_back(ms);
        if self.worst_input_us.len() == WINDOW {
            self.worst_input_us.pop_front();
        }
        self.worst_input_us.push_back(self.input_us);
        let input_us = std::mem::take(&mut self.input_us);
        let _log_cost = self
            .metrics
            .as_ref()
            .filter(|_| self.log.is_some())
            .map(|(m, id)| m.start(*id));
        if let Some(tx) = &self.log {
            let epoch_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
                - f64::from(ms);
            let _ = tx.send(format!(
                "{epoch_ms:.1},{ms:.3},{interval:.3},{:.3},{:.1},{input_us:.1}\n",
                self.build_ms, self.last_op_us
            ));
        }
        drop(_log_cost);
        if let Some((m, _)) = &self.metrics {
            m.frame_end();
        }
    }

    pub fn summary(&self) -> Summary {
        summarise(self.window.iter().copied(), self.last)
    }
}

/// Writes frame records on a background thread: `start_epoch_ms,frame_ms,interval_ms,build_ms,op_us,input_us`.
fn spawn_log(path: PathBuf) -> Sender<String> {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("cho: could not write the frame log {}: {e}", path.display());
                return;
            }
        };
        let mut out = std::io::BufWriter::new(file);
        let _ = out.write_all(b"start_epoch_ms,frame_ms,interval_ms,build_ms,op_us,input_us\n");
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    let _ = out.write_all(line.as_bytes());
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = out.flush();
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = out.flush();
    });
    tx
}

/// Saves the layout off the UI thread, debounced. A failure is logged and kept for the shell
/// to show, once until a save works again.
pub struct Saver {
    tx: Sender<Workspace>,
    failure: Arc<Mutex<Failure>>,
}

/// The saver thread's last failure, and whether the shell has shown it.
#[derive(Default)]
struct Failure {
    last: Option<String>,
    unseen: bool,
}

impl Failure {
    /// Records how a save went. Only a new failure is logged and shown.
    fn record(&mut self, result: std::io::Result<PathBuf>) {
        match result {
            Ok(_) => self.last = None,
            Err(e) => {
                let text = e.to_string();
                if self.last.as_ref() != Some(&text) {
                    eprintln!("cho: could not save the layout: {text}");
                    self.last = Some(text);
                    self.unseen = true;
                }
            }
        }
    }

    /// The failure as a sentence for the status bar, if the shell has not shown it yet.
    fn take(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.unseen) {
            return None;
        }
        let e = self.last.as_ref()?;
        Some(format!("Could not save the layout: {e}"))
    }
}

impl Saver {
    pub fn spawn(dir: PathBuf, repo_root: String) -> Self {
        let (tx, rx) = mpsc::channel::<Workspace>();
        let failure = Arc::new(Mutex::new(Failure::default()));
        let failed = failure.clone();
        let save = move |ws: &Workspace| {
            let result = cho_layout::save(&dir, &repo_root, ws);
            if let Ok(mut f) = failed.lock() {
                f.record(result);
            }
        };
        std::thread::spawn(move || {
            let mut pending: Option<Workspace> = None;
            loop {
                match rx.recv_timeout(SAVE_DEBOUNCE) {
                    Ok(ws) => pending = Some(ws),
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(ws) = pending.take() {
                            save(&ws);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        if let Some(ws) = pending.take() {
                            save(&ws);
                        }
                        break;
                    }
                }
            }
        });
        Self { tx, failure }
    }

    pub fn changed(&self, ws: &Workspace) {
        let _ = self.tx.send(ws.clone());
    }

    /// A save failure not shown yet, as a sentence for the status bar.
    pub fn take_failure(&self) -> Option<String> {
        self.failure.lock().ok()?.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_and_budget_counts() {
        let s = summarise((1..=100).map(|i| i as f32 / 4.0), 3.0);
        assert_eq!(s.frames, 100);
        assert_eq!(s.p50, 12.5);
        assert_eq!(s.p95, 23.75);
        assert_eq!(s.p99, 24.75);
        assert_eq!(s.worst, 25.0);
        assert_eq!(s.over_120, 100 - 33);
        assert_eq!(s.over_60, 100 - 66);
        assert_eq!(summarise(std::iter::empty(), 0.0).p99, 0.0);
    }

    #[test]
    fn a_save_failure_is_shown_once_until_a_save_works() {
        let full = || Err(std::io::Error::other("disk full"));
        let mut f = Failure::default();
        assert_eq!(f.take(), None);
        f.record(full());
        assert_eq!(
            f.take().as_deref(),
            Some("Could not save the layout: disk full")
        );
        assert_eq!(f.take(), None);
        // The same failure again is not shown again; after a save works, it is.
        f.record(full());
        assert_eq!(f.take(), None);
        f.record(Ok(PathBuf::new()));
        f.record(full());
        assert!(f.take().is_some());
    }
}

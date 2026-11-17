//! Cost of recording. Run: `cargo run --release -p cho-metrics --example bench`.

use cho_metrics::{Kind, Metrics};
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

const N: u32 = 5_000_000;

fn per_call(label: &str, total: Duration, n: u32) {
    println!(
        "{label:<52} {:>7.1} ns/call",
        total.as_nanos() as f64 / f64::from(n)
    );
}

fn main() {
    let m = Arc::new(Metrics::new(&[
        ("bench.a", Kind::Foreground),
        ("bench.b", Kind::Foreground),
        ("bench.c", Kind::Background),
        ("bench.d", Kind::Foreground),
    ]));
    let a = m.id("bench.a").unwrap();

    // Baseline: two clock reads, nothing recorded.
    let t = Instant::now();
    for _ in 0..N {
        let s = Instant::now();
        black_box(s.elapsed());
    }
    per_call("clock only (Instant::now + elapsed)", t.elapsed(), N);

    let t = Instant::now();
    for _ in 0..N {
        m.record_at(a, Duration::from_nanos(100), Duration::from_secs(1));
    }
    per_call("record_at (atomics only, frame closed)", t.elapsed(), N);

    let t = Instant::now();
    for _ in 0..N {
        let _t = m.start(a);
    }
    per_call("start + drop (full timer, frame closed)", t.elapsed(), N);

    m.frame_begin();
    let t = Instant::now();
    for _ in 0..N {
        let _t = m.start(a);
    }
    per_call("start + drop (full timer, inside a frame)", t.elapsed(), N);
    m.frame_end();

    // Frame begin/end, the per-frame cost.
    let t = Instant::now();
    for _ in 0..N {
        m.frame_begin();
        black_box(m.frame_end());
    }
    per_call("frame_begin + frame_end (fast frame)", t.elapsed(), N);

    // Four threads on one shared feature: worst case for contention.
    let per_thread = N / 4;
    let t = Instant::now();
    let hs: Vec<_> = (0..4)
        .map(|_| {
            let m = Arc::clone(&m);
            std::thread::spawn(move || {
                for _ in 0..per_thread {
                    let _t = m.start(a);
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    per_call(
        "start + drop, 4 threads sharing one feature",
        t.elapsed(),
        per_thread,
    );

    let t = Instant::now();
    for _ in 0..10_000 {
        black_box(m.top_by_window_total(3));
    }
    per_call(
        "top-three query (allocates; UI refresh only)",
        t.elapsed(),
        10_000,
    );
}

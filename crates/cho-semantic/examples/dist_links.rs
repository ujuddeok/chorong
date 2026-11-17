//! Prints what joining built declarations to source found for a repository's cached
//! index, and what it cost: `cargo run -p cho-semantic --example dist_links -- ROOT`.

use cho_semantic::{IndexCache, Semantic};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let cache = IndexCache::default_location().expect("the index cache");
    let started = Instant::now();
    let Some(sem) = Semantic::open(&cache, &root).expect("the index loads") else {
        eprintln!("no index for {}", root.display());
        std::process::exit(1);
    };
    let total = started.elapsed();
    let s = sem.dist_links().stats();
    println!(
        "load {:.2}s total, of which dist links {:.0} ms",
        total.as_secs_f64(),
        s.elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "workspace packages {}, candidates {}, mapped {}",
        s.packages, s.candidates, s.mapped
    );
    for (method, reexports, n) in &s.by_method {
        println!(
            "  {}{}: {n}",
            method.as_str(),
            if *reexports {
                ", through re-exports"
            } else {
                ""
            }
        );
    }
}

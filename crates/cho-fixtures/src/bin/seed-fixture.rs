//! `seed-fixture NAME DEST CACHE`: copies the TypeScript workspace fixture NAME (`dist`,
//! `source-condition`) to DEST, records its committed index in the index cache CACHE, and
//! prints the copy's path. Then `CHORONG_CACHE_DIR=CACHE cho definition … --root PATH` works
//! with no indexer installed. For verification scripts; see `cho_fixtures::TsWorkspace`.

use cho_fixtures::TsWorkspace;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [name, dest, cache] = &args[..] else {
        eprintln!("usage: seed-fixture NAME DEST CACHE (NAME: dist, source-condition)");
        std::process::exit(2);
    };
    let Some(w) = TsWorkspace::from_name(name) else {
        eprintln!("unknown fixture `{name}` (dist, source-condition)");
        std::process::exit(2);
    };
    match w.seed_cache(Path::new(dest), Path::new(cache)) {
        Ok(root) => println!("{}", root.display()),
        Err(e) => {
            eprintln!("seed-fixture: {e}");
            std::process::exit(1);
        }
    }
}

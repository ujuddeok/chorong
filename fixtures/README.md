# Shared test fixtures

Committed inputs that tests in any crate, the verification scripts and the fuzzers can use.
Read them through the `cho-fixtures` crate (`crates/cho-fixtures`, a dev-dependency, never
shipped) so paths stay in one place.

| Fixture | Layout | Rust | Scripts |
| --- | --- | --- | --- |
| [ts-workspaces/dist](ts-workspaces/dist/README.md) | TypeScript packages export built declarations (`dist/*.d.ts`, per file and bundled); uses from other packages reach source through the dist→source join | `TsWorkspace::Dist` | `seed-fixture dist DEST CACHE` |
| [ts-workspaces/source-condition](ts-workspaces/source-condition/README.md) | packages export source through a `development` condition; `tsc` emits `.types/` through project references; one source-only package | `TsWorkspace::SourceCondition` | `seed-fixture source-condition DEST CACHE` |

What `TsWorkspace` gives a test:

- `semantic()`: the committed index loaded in place, with the dist→source join, for query and
  graph tests;
- `checkout(dest)`: a private copy, for tests that write files;
- `seed_cache(dest, cache)`: a copy made a one-commit Git repository, with its index recorded in
  a Chorong index cache, so the real `cho` CLI or viewer runs against it
  (`CHORONG_CACHE_DIR=cache`, `--root` the returned path) with status `current` and no indexer
  installed. The `seed-fixture` binary does the same for shell scripts
  (`cargo build -p cho-fixtures --bin seed-fixture`).

Each fixture's index was generated once with Chorong's pinned indexer; its README says how to
regenerate it. The layouts and how to add one are in `docs/typescript-workspaces.md`. Fixtures
use neutral names (`@fx/*`) and hold no machine paths.

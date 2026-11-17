- Prepare remote worktrees before indexing: `cho prepare show|approve|set|run|auto|clear` and
  `cho open … --prepare`. A recipe comes from the project, the repository's
  `.chorong/prepare.json` or a lockfile suggestion; one approval covers prepare and indexing and
  is asked again when the recipe, a lockfile, a `package.json`, a `build.rs` or a proc-macro
  crate changes. Steps run without a shell, with a timeout, without host tokens, only in
  Chorong's own worktrees. A failed prepare still indexes, and the index reads `partial`.
- `cho index` on your own folder names the install command to run when `node_modules` is
  missing; the new `prepare.auto` setting (off in Lightweight mode) gates automatic prepare.

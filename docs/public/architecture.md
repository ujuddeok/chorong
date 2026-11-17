# Architecture

## Crates

```
crates/cho-core           SourceBuffer (byte-exact positions), chorong:// links, external editors
crates/cho-syntax         Tree-sitter registry, highlighting, declaration outline, symbol lookup
crates/cho-theme          Zed theme loader (schema v0.2.0) + bundled Chorong Dark/Light
crates/cho-investigation  Investigation format: types, validation, anchor checking, EdgeOracle
crates/cho-semantic       SCIP decode and queries, index cache, indexer runners, edge verification
crates/cho-fixtures       Tests only, not shipped: shared fixtures (TypeScript workspace layouts with committed indexes)
crates/cho-search         Repository text search on ripgrep's `ignore`, `grep-searcher`, `grep-regex`
crates/cho-settings       Settings: typed defaults, JSONC file, per-repo overrides, Lightweight mode, change notification (no dependencies)
crates/cho-metrics        Always-on per-feature cost counters, rolling window, frame budgets (no dependencies)
crates/cho-graph          Reference graph: nodes, edges, roll-up, views; JSON, DOT, HTML page
crates/cho-git            Read-only git facts: remote identity, worktrees, diffs, files at a revision, tree compare, line diff, merge base (gix)
crates/cho-changes        Changed files and declarations between two commits (cho-git + cho-syntax)
crates/cho-sources        Remote sources: projects, repository cache, fetch, worktrees, merge base, PR details, tokens
crates/cho-layout         Window layout model: docks, panes, tabs, pins, focus layouts, saved per repository
crates/cho-layout-ui      GPUI shell for cho-layout: docks, code area, focus pages, tray, Layout mode
crates/cho-layout-lab     Prototype GPUI window hosting the shell with placeholder panes
crates/cho-cli            `cho` (package `chorong`): headless CLI, JSON output for agents
app/                       `chorong-app`: GPUI viewer
```

`cho-sources` (no GPUI) opens GitHub and Bitbucket repositories into Chorong's own cache: the
`git` command fetches and adds worktrees, gix reads. See [sources.md](sources.md).

`cho-settings` and `cho-metrics` depend on nothing of ours, and on each other not at all:
the app (and the CLI, for settings) depends on both and registers `cho_settings::keys()`
with `Metrics::new`, so metrics are keyed by setting key without either crate knowing the
other. Library crates that want instrumentation can depend on `cho-metrics` alone.

Dependency direction is strictly downward: `cho-core` depends on nothing of ours; the
library crates never depend on `app` or the CLI (package `chorong`, in `crates/cho-cli`); the CLI
never depends on GPUI.

## One engine, two front ends

```
            ┌──────────── chorong-app (GPUI) ─────────────┐
            │  calls library functions directly,        │
            │  holds Rust structs, renders from memory  │
            └──────────────────┬────────────────────────┘
                               │ in-process function calls
  ┌────────────────────────────┴───────────────────────────────┐
  │ cho-core · cho-syntax · cho-theme · cho-investigation …  │
  └────────────────────────────┬───────────────────────────────┘
                               │ in-process function calls
            ┌──────────────────┴────────────────────────┐
            │  cho-cli: same calls, serialises results  │
            │  to JSON at the very end (for agents)      │
            └────────────────────────────────────────────┘
```

**The viewer does not talk JSON.** It links the library crates and works directly on their
types: highlight spans, symbols, buffers. JSON exists only at the edges, for the CLI,
investigation files, and a future agent API. That keeps the UI path free of serialisation,
IPC and copying, and it means the CLI and the UI always agree, because they run the same code.

When the viewer needs to be driven from outside (an agent asking "show this symbol"), the
plan is for the running app to accept `chorong://` links through the OS URL handler or a
local socket. A link is a small command, not a data channel; the app resolves it with the
same in-process calls.

## Performance rules for the UI

- Never block the UI thread on disk, parsing, indexing or subprocesses. Today a file is
  read and highlighted on open (fine for typical files); this moves to a background task
  with a generation counter before files get large.
- Render only what's visible: the file list and the source view are virtualised
  (`uniform_list`), so a 50,000-line file costs the same per frame as a 50-line one.
- Compute once per file, not per frame: highlights and the outline are built when a file
  loads; rendering just slices them.
- Positions are byte offsets end to end, so no per-frame re-measuring of text to map
  positions.
- Pane management stays within one frame at 120 Hz (8.3 ms) on the UI thread: dragging with
  drop-zone feedback, resizing, minimise and maximise, focus layouts, dock toggles
  (planned layout work).
- Layout operations are O(panes) and run on the model (`cho-layout`), never in render code.
  While dragging, the model is asked whether a drop would be accepted only when the target
  under the pointer changes, not on every move.
- Layout state is saved off the UI thread, debounced: the UI thread hands over a copy of the
  workspace and a background thread serialises it and writes it to disk.
- Draw only what can be seen. Tabs past the end of a strip are not drawn (a `+N` count shows
  them), and nor is the body of a pane squeezed to its header. Pane bodies list their rows
  with `uniform_list`.
- Name an installed UI font. On Linux, a missing `.SystemUIFont` makes GPUI repeat a failed
  font lookup, with a backtrace when `RUST_BACKTRACE` is set, on every text run.
- Measure on the UI thread with the frame-time overlay (Cmd-K F) on real hardware, and use
  `perf-lab.sh` under Xvfb to catch regressions.

## Invariants

- **Byte offsets.** Every stored position is a UTF-8 byte offset. Only `SourceBuffer`
  converts to lines, UTF-8 columns or UTF-16 columns.
- **Deterministic.** Chorong never calls an LLM. Imported claims (investigations, overlays)
  are checked and labelled `verified`, `claimed` or `broken`. See
  [investigation-format.md](investigation-format.md).
- **Read-only.** No code path writes to a repository. Editing happens in another editor
  (see [external-editors.md](external-editors.md)).
- **No guessing.** Symbol lookup is by declaration in the parsed source (later: SCIP
  identity). A name that matches several declarations is reported as ambiguous, never
  silently resolved.
